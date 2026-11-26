//! Natives for `@tola/icon`: inline SVG, normalized bytes, and published URLs.

use std::collections::BTreeMap;
use std::path::Path;

use crate::{ICON_OBSERVATION_DIRECTORY, ICON_REQUEST_OBSERVATION_DIRECTORY, TolaPackage};
use tola_address::OutputPath;
use tola_icons::{
    IconCollectionKey, IconCollections, IconId, InvalidIconName, InvalidSvg, SvgIcon,
};
use tola_typst::{ContentDigest, PackageSpec, ReadLocator};
use typst::World;
use typst::comemo::Tracked;
use typst::diag::{At, FileError, SourceDiagnostic, SourceResult, bail};
use typst::ecow::eco_vec;
use typst::engine::Engine;
use typst::foundations::{Args, Bytes, Content, Context, Dict, NativeElement, Str, func};
use typst::introspection::here;
use typst::syntax::{FileId, RootedPath, Span, VirtualPath, VirtualRoot};
use typst::text::TextElem;
use typst_html::{HtmlAttr, HtmlAttrs, HtmlElem, HtmlTag};

fn icon_pkg_id(rel: String) -> FileId {
    FileId::new(RootedPath::new(
        VirtualRoot::Package(TolaPackage::Icon.spec()),
        VirtualPath::new(rel).expect("icon path is virtualizable"),
    ))
}

/// Configured collection named by an icon SVG or `.names` observation path.
pub fn collection_of_icon_file(path: &Path) -> Option<&str> {
    let rel = path
        .strip_prefix(ICON_OBSERVATION_DIRECTORY)
        .ok()?
        .to_str()?;
    let (collection, name) = rel.rsplit_once('/')?;
    (!collection.is_empty() && (name.ends_with(".svg") || name == ".names")).then_some(collection)
}

/// Whether `path` names an icon observation in `@tola/icon`.
pub fn is_icon_file(pkg: &PackageSpec, path: &Path) -> bool {
    TolaPackage::from_spec(pkg) == Some(TolaPackage::Icon)
        && path.starts_with(ICON_OBSERVATION_DIRECTORY)
}

/// Normalized SVG bytes for a configured icon.
pub fn icon_file_bytes<'a>(
    collections: &'a IconCollections,
    pkg: &PackageSpec,
    path: &Path,
) -> Option<&'a [u8]> {
    let (collection, name) = obs_parts(pkg, path)?;
    collections
        .get(collection, name.strip_suffix(".svg")?)
        .map(|icon| icon.svg().as_bytes())
}

fn obs_parts<'a>(pkg: &PackageSpec, path: &'a Path) -> Option<(&'a str, &'a str)> {
    if !is_icon_file(pkg, path) {
        return None;
    }
    path.strip_prefix(ICON_OBSERVATION_DIRECTORY)
        .ok()?
        .to_str()?
        .rsplit_once('/')
}

/// Tracked newline-joined icon names for suggestions within a mounted collection.
pub fn icon_collection_names_bytes(
    collections: &IconCollections,
    pkg: &PackageSpec,
    path: &Path,
) -> Option<Vec<u8>> {
    let (collection, name) = obs_parts(pkg, path)?;
    if name != ".names" {
        return None;
    }
    Some(
        collections
            .collection(collection)?
            .names()
            .collect::<Vec<_>>()
            .join("\n")
            .into_bytes(),
    )
}

/// Published icon file requested via `icon-url`.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PublishedIcon {
    id: IconId,
}

impl PublishedIcon {
    pub fn new(id: IconId) -> Self {
        Self { id }
    }

    pub fn namespace(&self) -> &str {
        self.id.collection()
    }

    pub fn name(&self) -> &str {
        self.id.name()
    }

    /// Tracked observation file read by `icon-url`.
    pub fn observation_id(&self) -> FileId {
        icon_pkg_id(format!(
            "{ICON_REQUEST_OBSERVATION_DIRECTORY}{}/{}",
            self.namespace(),
            self.name()
        ))
    }

    pub fn from_read(locator: &ReadLocator) -> Option<Self> {
        let (ReadLocator::Package { package, path }
        | ReadLocator::ProvidedPackage { package, path }) = locator
        else {
            return None;
        };
        published_icon_request(package, path)
    }

    pub fn from_path(path: &Path) -> Option<Self> {
        let rel = path.to_str()?;
        let rel = rel.strip_prefix('/').unwrap_or(rel);
        let rel = rel.strip_prefix(ICON_REQUEST_OBSERVATION_DIRECTORY)?;
        let (ns, name) = rel.rsplit_once('/')?;
        IconId::new(ns, name).ok().map(Self::new)
    }
}

pub fn published_icon_request(pkg: &PackageSpec, path: &Path) -> Option<PublishedIcon> {
    if TolaPackage::from_spec(pkg) != Some(TolaPackage::Icon) {
        return None;
    }
    PublishedIcon::from_path(path)
}

/// Content-addressed output path: `_tola/icons/<key>.svg`.
pub fn published_icon_path(bytes: &[u8]) -> OutputPath {
    OutputPath::parse(&format!(
        "_tola/icons/{}.svg",
        ContentDigest::of(bytes).to_hex()
    ))
    .expect("published icon paths contain only fixed segments and hex")
}

struct LoadedIcon {
    id: IconId,
    bytes: Bytes,
}

#[comemo::memoize]
fn icon_file(id: &str) -> Result<(IconId, FileId), InvalidIconName> {
    let id = id.parse::<IconId>()?;
    let file = icon_pkg_id(format!(
        "{ICON_OBSERVATION_DIRECTORY}/{}/{}.svg",
        id.collection(),
        id.name()
    ));
    Ok((id, file))
}

fn read_icon(
    engine: &mut Engine,
    requested: &str,
    module: Option<&str>,
    span: Span,
) -> SourceResult<LoadedIcon> {
    if let Some(m) = module
        && !super::library::module_records(engine).is_some_and(|ms| ms.contains(m))
    {
        bail!(span, "module `{m}` is not configured for this site");
    }
    let parsed = (|| {
        let (col, name) = requested.split_once(':').ok_or(InvalidIconName)?;
        let col = IconCollectionKey::new(module, col)?;
        let (id, file) = icon_file(&format!("{col}:{name}"))?;
        Ok::<_, InvalidIconName>((col, id, file))
    })();
    let (col, id, file) = parsed.map_err(|err| {
        let mut diag = crate::diagnostic::package_error(
            span,
            TolaPackage::Icon,
            format!("`{requested}` is not a valid icon id: {err}"),
        );
        diag.hint("Write `collection:name` using a configured collection");
        eco_vec![diag]
    })?;
    match engine.world.file(file) {
        Ok(bytes) => Ok(LoadedIcon { id, bytes }),
        Err(FileError::NotFound(_)) => Err(eco_vec![icon_not_found(
            engine, requested, &col, &id, module, span
        )?]),
        Err(err) => Err::<LoadedIcon, String>(err.to_string()).at(span),
    }
}

fn icon_not_found(
    engine: &Engine,
    requested: &str,
    col: &IconCollectionKey,
    id: &IconId,
    module: Option<&str>,
    span: Span,
) -> SourceResult<SourceDiagnostic> {
    let owner = module.map_or_else(
        || "the site".to_owned(),
        |alias| format!("module `{alias}`"),
    );
    let names_file = icon_pkg_id(format!(
        "{ICON_OBSERVATION_DIRECTORY}/{}/.names",
        id.collection()
    ));
    match engine.world.file(names_file) {
        Ok(names) => {
            let mut diag = crate::diagnostic::package_error(
                span,
                TolaPackage::Icon,
                format!(
                    "the icon `{requested}` is not in collection `{}` of {owner}",
                    col.name()
                ),
            );
            let names = std::str::from_utf8(names.as_slice())
                .expect("collection names are validated ASCII");
            if let Some(closest) = tola_typst::spelling::closest(id.name(), names.lines()) {
                diag.hint(format!("Did you mean `{}:{closest}`?", col.name()));
            }
            let selector = module.map_or_else(String::new, |alias| format!(" --module {alias}"));
            diag.hint(format!(
                "Run `tola inspect icons {}{selector}` to list names in this collection",
                col.name()
            ));
            Ok(diag)
        }
        Err(FileError::NotFound(_)) => {
            let mut diag = crate::diagnostic::package_error(
                span,
                TolaPackage::Icon,
                format!(
                    "icon collection `{}` is not declared in `icons.collections` of {owner}",
                    col.name()
                ),
            );
            let cfg = if module.is_some() {
                "the module's tola-module.toml"
            } else {
                "tola.toml"
            };
            diag.hint(format!(
                "Declare `[icons.collections.{}]` in {cfg}, or use a configured collection",
                col.name()
            ));
            Ok(diag)
        }
        Err(err) => Err::<SourceDiagnostic, String>(err.to_string()).at(span),
    }
}

#[comemo::memoize]
fn parsed_icon(bytes: &Bytes) -> Result<SvgIcon, InvalidSvg> {
    SvgIcon::parse(bytes.as_slice())
}

#[func]
pub(super) fn tola_icon_bytes(
    engine: &mut Engine,
    span: Span,
    /// The configured icon's identity, such as `"brand:mark"`.
    id: Str,
    /// Module alias owning this collection. Omit to use the site's collections.
    #[named]
    #[default]
    module: Option<Str>,
) -> SourceResult<Bytes> {
    Ok(read_icon(engine, id.as_str(), module.as_deref(), span)?.bytes)
}

#[func]
pub(super) fn tola_icon_url(
    engine: &mut Engine,
    span: Span,
    /// The configured icon's identity, such as `"brand:mark"`.
    id: Str,
    /// Module alias owning this collection. Omit to use the site's collections.
    #[named]
    #[default]
    module: Option<Str>,
) -> SourceResult<Str> {
    let icon = read_icon(engine, id.as_str(), module.as_deref(), span)?;
    let published = PublishedIcon::new(icon.id);
    let url = super::library::browser_output_url(engine, span, &published_icon_path(&icon.bytes))?;
    // Tracked read lets a full build publish only requested icons, even across memoized replays.
    engine.world.file(published.observation_id()).at(span)?;
    Ok(Str::from(url))
}

#[func(contextual)]
pub(super) fn tola_icon(
    context: Tracked<Context>,
    args: &mut Args,
    span: Span,
) -> SourceResult<Content> {
    let bytes = args.expect::<Bytes>("icon bytes")?;
    let label = args.named::<Option<Str>>("label")?.flatten();
    let attrs = args.named::<Dict>("attrs")?.unwrap_or_default();
    args.take().finish()?;
    if label.as_ref().is_some_and(|l| l.as_str().trim().is_empty()) {
        bail!(
            span,
            "`label` must be a nonempty string or `none` for a decorative icon"
        );
    }
    let loc = here(context).at(span)?;
    let icon = parsed_icon(&bytes).map_err(|e| e.to_string()).at(span)?;
    let svg = icon
        .svg_with_id_prefix(&format!("tola-icon-{:032x}", loc.hash()))
        .map_err(|e| e.to_string())
        .at(span)?;
    let doc = roxmltree::Document::parse(&svg)
        .map_err(|e| e.to_string())
        .at(span)?;
    let root = doc.root_element();
    let mut root_attrs = xml_attrs(root);
    for ns in root.namespaces() {
        if let Some(prefix) = ns.name() {
            root_attrs.insert(format!("xmlns:{prefix}"), ns.uri().to_owned());
        }
    }
    root_attrs.insert("xmlns".into(), "http://www.w3.org/2000/svg".into());
    root_attrs
        .entry("viewBox".into())
        .or_insert_with(|| icon.view_box().to_string());
    root_attrs.insert("width".into(), format!("{}em", icon.aspect_ratio()));
    root_attrs.insert("height".into(), "1em".into());
    for (name, val) in attrs {
        let name = crate::attr_key(&name);
        if label_owns_attr(name) {
            bail!(
                span,
                "set accessibility semantics with `label`, not `{name}`"
            );
        }
        let val = val.cast::<Str>().at(span)?;
        crate::merge_attr(&mut root_attrs, name, val.as_str());
    }
    root_attrs.retain(|name, _| !label_owns_attr(name));
    if let Some(label) = label {
        root_attrs.insert("role".into(), "img".into());
        root_attrs.insert("aria-label".into(), label.to_string());
    } else {
        root_attrs.insert("aria-hidden".into(), "true".into());
    }
    root_attrs.insert("focusable".into(), "false".into());

    xml_content(root, Some(root_attrs), span)
}

fn label_owns_attr(name: &str) -> bool {
    [
        "aria-label",
        "aria-labelledby",
        "aria-hidden",
        "role",
        "focusable",
    ]
    .iter()
    .any(|owned| name.eq_ignore_ascii_case(owned))
}

fn xml_attrs(node: roxmltree::Node<'_, '_>) -> BTreeMap<String, String> {
    node.attributes()
        .map(|attr| {
            let name = match attr.namespace() {
                Some("http://www.w3.org/XML/1998/namespace") => format!("xml:{}", attr.name()),
                Some("http://www.w3.org/1999/xlink") => format!("xlink:{}", attr.name()),
                _ => attr.name().to_owned(),
            };
            (name, attr.value().to_owned())
        })
        .collect()
}

fn xml_content(
    node: roxmltree::Node<'_, '_>,
    root_attrs: Option<BTreeMap<String, String>>,
    span: Span,
) -> SourceResult<Content> {
    if node.is_text() {
        return Ok(TextElem::packed(node.text().unwrap_or_default()));
    }
    let tag = HtmlTag::intern(node.tag_name().name()).at(span)?;
    let mut attrs = HtmlAttrs::new();
    for (name, val) in root_attrs.unwrap_or_else(|| xml_attrs(node)) {
        attrs.push(HtmlAttr::intern(&name).at(span)?, val);
    }
    let children = node
        .children()
        .filter(|c| c.is_element() || c.is_text())
        .map(|c| xml_content(c, None, span))
        .collect::<SourceResult<Vec<_>>>()?;
    Ok(HtmlElem::new(tag)
        .with_attrs(attrs)
        .with_body(Some(Content::sequence(children)))
        .pack()
        .spanned(span))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_suggestions_scoped_to_collection() {
        let col = tola_icons::IconCollection::from_iconify(
            br#"{"prefix":"upstream","icons":{"palette":{"body":"<path/>"}},"aliases":{"paints":{"parent":"palette"}},"chars":{"e001":"palette"}}"#,
        ).unwrap();
        let mut cols = IconCollections::new();
        cols.mount("lucide", col).unwrap();
        let pkg = TolaPackage::Icon.spec();
        let path = Path::new(".tola-icon/lucide/.names");
        let names = icon_collection_names_bytes(&cols, &pkg, path).unwrap();
        let names = std::str::from_utf8(&names).unwrap();
        assert_eq!(
            tola_typst::spelling::closest("paette", names.lines()),
            Some("palette")
        );
        assert_eq!(
            tola_typst::spelling::closest("paint", names.lines()),
            Some("paints")
        );
        assert_eq!(
            tola_typst::spelling::closest("unrelated", names.lines()),
            None
        );
        assert_eq!(collection_of_icon_file(path), Some("lucide"));
        assert!(
            icon_collection_names_bytes(&cols, &pkg, Path::new(".tola-icon/upstream/.names"))
                .is_none()
        );
        assert!(icon_file_bytes(&cols, &pkg, Path::new(".tola-icon/lucide/e001.svg")).is_some());
        assert!(!names.contains("e001"));
    }
}
