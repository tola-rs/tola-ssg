//! The natives behind `@tola/icon`: inline SVG, normalized bytes, and a published URL.

use std::collections::BTreeMap;

use crate::{ICON_OBSERVATION_DIRECTORY, ICON_REQUEST_OBSERVATION_DIRECTORY, TolaPackage};
use tola_address::OutputPath;
use tola_typst::ContentDigest;
use typst::World;
use typst::comemo::Tracked;
use typst::diag::{At, FileError, SourceResult, bail};
use typst::engine::Engine;
use typst::foundations::{Args, Bytes, Content, Context, Dict, NativeElement, Str, func};
use typst::introspection::here;
use typst::syntax::{FileId, RootedPath, Span, VirtualPath, VirtualRoot};
use typst::text::TextElem;
use typst_html::{HtmlAttr, HtmlAttrs, HtmlElem, HtmlTag};

/// The configured collection named by one `.tola-icon/<collection>/<name>.svg` path.
///
/// `None` for any other package file, including malformed icon paths.
pub fn collection_of_icon_file(path: &std::path::Path) -> Option<&str> {
    let relative = path
        .strip_prefix(ICON_OBSERVATION_DIRECTORY)
        .ok()?
        .to_str()?;
    let (collection, name) = relative.split_once('/')?;
    (!collection.is_empty() && name.ends_with(".svg")).then_some(collection)
}

/// Whether `path` names an icon observation in the `@tola/icon` package.
///
/// Any path under the observation directory qualifies, so a malformed or absent icon name stays
/// the world's error to report rather than a silent miss.
pub fn is_icon_file(package: &tola_typst::PackageSpec, path: &std::path::Path) -> bool {
    TolaPackage::from_spec(package) == Some(TolaPackage::Icon)
        && path.starts_with(ICON_OBSERVATION_DIRECTORY)
}

/// The normalized bytes of one configured icon.
///
/// `None` for a path that names no icon, including an icon the configured collections do not
/// carry, which leaves the miss to the world's own error.
pub fn icon_file_bytes<'a>(
    collections: &'a tola_icons::IconCollections,
    package: &tola_typst::PackageSpec,
    path: &std::path::Path,
) -> Option<&'a [u8]> {
    if !is_icon_file(package, path) {
        return None;
    }
    let relative = path
        .strip_prefix(ICON_OBSERVATION_DIRECTORY)
        .ok()?
        .to_str()?;
    let (collection, name) = relative.split_once('/')?;
    collections
        .get(collection, name.strip_suffix(".svg")?)
        .map(|icon| icon.svg().as_bytes())
}

/// One icon a document publishes as a file of its own.
///
/// `icon-url` reads this request's virtual file, and a complete build collects those reads, so
/// exactly the icons documents asked for are published.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PublishedIcon {
    identity: tola_icons::IconId,
}

impl PublishedIcon {
    /// Consume an identity already validated by the icon grammar.
    pub fn new(identity: tola_icons::IconId) -> Self {
        Self { identity }
    }

    pub fn namespace(&self) -> &str {
        self.identity.collection()
    }

    pub fn name(&self) -> &str {
        self.identity.name()
    }

    /// The observation file one `icon-url` call reads.
    pub fn observation_id(&self) -> FileId {
        FileId::new(RootedPath::new(
            VirtualRoot::Package(TolaPackage::Icon.spec()),
            VirtualPath::new(format!(
                "{ICON_REQUEST_OBSERVATION_DIRECTORY}{}/{}",
                self.namespace(),
                self.name()
            ))
            .expect("a validated icon identity forms a virtual path"),
        ))
    }

    /// The request one observed read carries, or `None` for every other read.
    pub fn from_read(locator: &tola_typst::ReadLocator) -> Option<Self> {
        let (tola_typst::ReadLocator::Package { package, path }
        | tola_typst::ReadLocator::ProvidedPackage { package, path }) = locator
        else {
            return None;
        };
        published_icon_request(package, path)
    }

    /// The request one observed icon path names, or `None` for every other file.
    pub fn from_path(path: &std::path::Path) -> Option<Self> {
        let relative = path.to_str()?;
        let relative = relative.strip_prefix('/').unwrap_or(relative);
        let relative = relative.strip_prefix(ICON_REQUEST_OBSERVATION_DIRECTORY)?;
        let (namespace, name) = relative.split_once('/')?;
        tola_icons::IconId::new(namespace, name).ok().map(Self::new)
    }
}

/// The published icon request one package path names, or `None` for every other file.
///
/// The engine answers these paths, so a document's request is ordinary tracked input.
pub fn published_icon_request(
    package: &tola_typst::PackageSpec,
    path: &std::path::Path,
) -> Option<PublishedIcon> {
    if TolaPackage::from_spec(package) != Some(TolaPackage::Icon) {
        return None;
    }
    PublishedIcon::from_path(path)
}

/// The output one published icon occupies, named by the bytes it serves: `_tola/icons/<key>.svg`.
pub fn published_icon_path(bytes: &[u8]) -> OutputPath {
    OutputPath::parse(&format!(
        "_tola/icons/{}.svg",
        ContentDigest::of(bytes).to_hex()
    ))
    .expect("published icon paths contain only fixed segments and hexadecimal keys")
}

/// One requested icon: the identity it names and the bytes Tola would serve.
struct ResolvedIcon {
    identity: tola_icons::IconId,
    bytes: Bytes,
}

#[comemo::memoize]
fn icon_file(identity: &str) -> Result<(tola_icons::IconId, FileId), tola_icons::InvalidIconName> {
    let identity = identity.parse::<tola_icons::IconId>()?;
    let file = FileId::new(RootedPath::new(
        VirtualRoot::Package(TolaPackage::Icon.spec()),
        VirtualPath::new(format!(
            "{ICON_OBSERVATION_DIRECTORY}/{}/{}.svg",
            identity.collection(),
            identity.name(),
        ))
        .expect("validated icon names form a virtual path"),
    ));
    Ok((identity, file))
}

fn read_icon(engine: &mut Engine, identity: &str, span: Span) -> SourceResult<ResolvedIcon> {
    let (identity, file) = icon_file(identity)
        .map_err(|error| format!("`{identity}` is not a valid icon id: {error}"))
        .at(span)?;
    let bytes = match engine.world.file(file) {
        Ok(bytes) => bytes,
        Err(FileError::NotFound(_)) => {
            bail!(
                span,
                "the icon `{}:{}` is not in the collections declared by `icons.collections`",
                identity.collection(),
                identity.name(),
            );
        }
        Err(error) => return Err::<ResolvedIcon, String>(error.to_string()).at(span),
    };
    Ok(ResolvedIcon { identity, bytes })
}

#[comemo::memoize]
fn parsed_icon(bytes: &Bytes) -> Result<tola_icons::SvgIcon, tola_icons::InvalidSvg> {
    tola_icons::SvgIcon::parse(bytes.as_slice())
}

#[func]
pub(super) fn tola_icon_bytes(
    engine: &mut Engine,
    span: Span,
    /// The configured icon's identity, such as `"brand:mark"`.
    id: Str,
) -> SourceResult<Bytes> {
    Ok(read_icon(engine, id.as_str(), span)?.bytes)
}

#[func]
pub(super) fn tola_icon_url(
    engine: &mut Engine,
    span: Span,
    /// The configured icon's identity, such as `"brand:mark"`.
    id: Str,
) -> SourceResult<Str> {
    let icon = read_icon(engine, id.as_str(), span)?;
    let published = PublishedIcon::new(icon.identity);
    let url = super::library::browser_output_url(engine, span, &published_icon_path(&icon.bytes))?;
    // This is ordinary tracked World input, not an imperative output queue, so a complete build
    // collects exactly the icons its documents asked for, including through memoized replays.
    engine.world.file(published.observation_id()).at(span)?;
    Ok(Str::from(url))
}

#[func(contextual)]
pub(super) fn tola_icon(
    engine: &mut Engine,
    context: Tracked<Context>,
    args: &mut Args,
    span: Span,
) -> SourceResult<Content> {
    let identity = args.expect::<Str>("icon id")?;
    let label = args.named::<Option<Str>>("label")?.flatten();
    let attributes = args.named::<Dict>("attrs")?.unwrap_or_default();
    args.take().finish()?;
    if label
        .as_ref()
        .is_some_and(|label| label.as_str().trim().is_empty())
    {
        bail!(
            span,
            "`label` must be a nonempty string or `none` for a decorative icon"
        );
    }
    let location = here(context).at(span)?;
    let icon = parsed_icon(&read_icon(engine, &identity, span)?.bytes)
        .map_err(|error| error.to_string())
        .at(span)?;
    let svg = icon
        .svg_with_id_prefix(&format!("tola-icon-{:032x}", location.hash()))
        .map_err(|error| error.to_string())
        .at(span)?;
    let document = roxmltree::Document::parse(&svg)
        .map_err(|error| error.to_string())
        .at(span)?;
    let root = document.root_element();
    let mut root_attributes = xml_attributes(root);
    for namespace in root.namespaces() {
        if let Some(prefix) = namespace.name() {
            root_attributes.insert(format!("xmlns:{prefix}"), namespace.uri().to_owned());
        }
    }
    root_attributes.insert("xmlns".into(), "http://www.w3.org/2000/svg".into());
    root_attributes
        .entry("viewBox".into())
        .or_insert_with(|| icon.view_box().to_string());
    root_attributes.insert("width".into(), format!("{}em", icon.aspect_ratio()));
    root_attributes.insert("height".into(), "1em".into());
    for (name, value) in attributes {
        let name = if name.as_str().eq_ignore_ascii_case("class") {
            Str::from("class")
        } else if name.as_str().eq_ignore_ascii_case("style") {
            Str::from("style")
        } else {
            name
        };
        if label_owns_attribute(name.as_str()) {
            bail!(
                span,
                "set accessibility semantics with `label`, not `{name}`"
            );
        }
        let value = value.cast::<Str>().at(span)?.to_string();
        let value = match (name.as_str(), root_attributes.get(name.as_str())) {
            ("class", Some(source)) => format!("{source} {value}"),
            ("style", Some(source)) => format!("{};{value}", source.trim_end_matches(';')),
            _ => value,
        };
        root_attributes.insert(name.to_string(), value);
    }
    root_attributes.retain(|name, _| !label_owns_attribute(name));
    if let Some(label) = label {
        root_attributes.insert("role".into(), "img".into());
        root_attributes.insert("aria-label".into(), label.to_string());
    } else {
        root_attributes.insert("aria-hidden".into(), "true".into());
    }
    root_attributes.insert("focusable".into(), "false".into());

    xml_content(root, Some(root_attributes), span)
}

fn label_owns_attribute(name: &str) -> bool {
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

fn xml_attributes(node: roxmltree::Node<'_, '_>) -> BTreeMap<String, String> {
    node.attributes()
        .map(|attribute| {
            let name = match attribute.namespace() {
                Some("http://www.w3.org/XML/1998/namespace") => format!("xml:{}", attribute.name()),
                Some("http://www.w3.org/1999/xlink") => format!("xlink:{}", attribute.name()),
                _ => attribute.name().to_owned(),
            };
            (name, attribute.value().to_owned())
        })
        .collect()
}

fn xml_content(
    node: roxmltree::Node<'_, '_>,
    root_attributes: Option<BTreeMap<String, String>>,
    span: Span,
) -> SourceResult<Content> {
    if node.is_text() {
        return Ok(TextElem::packed(node.text().unwrap_or_default()));
    }
    let tag = HtmlTag::intern(node.tag_name().name()).at(span)?;
    let mut attributes = HtmlAttrs::new();
    for (name, value) in root_attributes.unwrap_or_else(|| xml_attributes(node)) {
        attributes.push(HtmlAttr::intern(&name).at(span)?, value);
    }
    let children = node
        .children()
        .filter(|child| child.is_element() || child.is_text())
        .map(|child| xml_content(child, None, span))
        .collect::<SourceResult<Vec<_>>>()?;
    Ok(HtmlElem::new(tag)
        .with_attrs(attributes)
        .with_body(Some(Content::sequence(children)))
        .pack()
        .spanned(span))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn published_requests_round_trip() {
        let request = PublishedIcon::new(tola_icons::IconId::new("brand", "mark").unwrap());
        for path in [
            std::path::PathBuf::from(".tola-icon-request/brand/mark"),
            std::path::PathBuf::from("/.tola-icon-request/brand/mark"),
        ] {
            assert_eq!(PublishedIcon::from_path(&path), Some(request.clone()));
        }
        for other in [
            ".tola-icon/brand/mark.svg",
            ".tola-icon-request/brand",
            ".tola-icon-request/brand/mark/nested",
            ".tola-icon-request/a/b/mark",
            ".tola-icon-request//mark",
        ] {
            assert_eq!(PublishedIcon::from_path(std::path::Path::new(other)), None);
        }
    }

    #[test]
    fn published_paths_follow_their_bytes() {
        let path = published_icon_path(b"<svg/>");
        assert_eq!(path, published_icon_path(b"<svg/>"));
        assert_ne!(path, published_icon_path(b"<svg />"));
        assert!(path.as_str().starts_with("_tola/icons/"));
        assert!(path.as_str().ends_with(".svg"));
    }
}
