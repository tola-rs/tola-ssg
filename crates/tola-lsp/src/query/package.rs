//! Import paths resolve against builtin, configured local, and published packages
//! without requiring a successful Bundle.

mod paths;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    GotoDefinitionResponse, Location, TextEdit, Uri,
};
use tola_build::cancellation::BuildCancellation;
use tola_build::config::SiteConfig;
use tola_packages::{BuiltinPackage, TOLA_NAMESPACE, builtin_package, builtin_packages};
use tola_typst::typst::syntax::package::{PackageManifest, PackageSpec, PackageVersion};
use tola_typst::typst::syntax::{
    FileId, LinkedNode, RootedPath, Source, VirtualPath, VirtualRoot, ast,
};
use tola_typst_syntax::names::SourceNames;

use crate::packages::{PackageAccess, importable_packages, installed_versions};
use crate::position;
use crate::protocol::{SourceQuery, SourceReply};
use crate::published::PublishedPackage;

use paths::{
    ImportPath, import_completions, import_path, import_position, reader_path_completions,
    with_closing_quote,
};

pub(super) struct PackageQuery<'a, 'b> {
    pub(super) config: &'a SiteConfig,
    pub(super) package_access: &'a PackageAccess<'b>,
    pub(super) source: &'a Source,
    pub(super) cursor: usize,
    pub(super) query: &'a SourceQuery,
    pub(super) client_root: &'a crate::uri::ClientRoot,
    pub(super) cancellation: &'a BuildCancellation,
    pub(super) overrides: &'a [(PathBuf, Arc<str>)],
    pub(super) indexed_names: Option<&'a SourceNames>,
}

pub(super) fn respond(inputs: PackageQuery<'_, '_>) -> Result<Option<SourceReply>> {
    let PackageQuery {
        config: cfg,
        package_access: pkgs,
        source: src,
        cursor: pos,
        query,
        client_root,
        cancellation: cancel,
        overrides,
        indexed_names,
    } = inputs;
    let import = import_path(src, pos);
    if matches!(query, SourceQuery::Completion(_)) {
        let offered = match &import {
            Some(import) => import_completions(cfg, pkgs, src, pos, import, cancel, overrides)?,
            None => reader_path_completions(
                cfg,
                src,
                pos,
                indexed_names,
                cancel,
                overrides,
                &pkgs.boundary,
            )?,
        };
        if let Some(items) = offered {
            return Ok(Some(SourceReply::Completion(CompletionResponse::Array(
                items,
            ))));
        }
    }
    let Some(import) = import else {
        return Ok(None);
    };
    if let Some(reply) =
        local_module_answer(cfg, pkgs, src, &import, query, client_root, overrides)?
    {
        return Ok(Some(reply));
    }
    builtin_answer(cfg, pkgs, src, &import, query, client_root)
}

fn local_module_answer(
    cfg: &SiteConfig,
    pkgs: &PackageAccess<'_>,
    src: &Source,
    import: &ImportPath<'_>,
    query: &SourceQuery,
    client_root: &crate::uri::ClientRoot,
    overrides: &[(PathBuf, Arc<str>)],
) -> Result<Option<SourceReply>> {
    let Some(literal) = import.node.cast::<ast::Str>() else {
        return Ok(None);
    };
    let written = literal.get();
    let Some(id) = local_imported_source(src.id(), &written, pkgs) else {
        return Ok(None);
    };
    match query {
        SourceQuery::Definition(_) => {
            if super::context::load_source(id, cfg, overrides, pkgs)?.is_none() {
                return Ok(Some(super::unavailable(query)));
            }
            let Some(path) = crate::identity::package_file_path_with_packages(
                id,
                &pkgs.locations,
                &pkgs.boundary,
                &pkgs.local_packages,
            ) else {
                return Ok(Some(super::unavailable(query)));
            };
            Ok(Some(definition_reply(
                client_root.address(&path)?,
                lsp_types::Range::default(),
            )))
        }
        SourceQuery::Hover(_) => Ok(Some(import_path_hover(
            src,
            &import.node,
            format!("Declared local module source `{written}`."),
        ))),
        _ => Ok(Some(super::unavailable(query))),
    }
}

fn builtin_answer(
    cfg: &SiteConfig,
    pkgs: &PackageAccess<'_>,
    src: &Source,
    import: &ImportPath<'_>,
    query: &SourceQuery,
    client_root: &crate::uri::ClientRoot,
) -> Result<Option<SourceReply>> {
    let Some(literal) = import.node.cast::<ast::Str>() else {
        return Ok(None);
    };
    let written = literal.get();
    let Some((pkg, id, desc)) = imported_source(src.id(), &written)? else {
        if matches!(query, SourceQuery::Hover(_))
            && let Ok(spec) = written.parse::<PackageSpec>()
        {
            return Ok(Some(import_path_hover(
                src,
                &import.node,
                spec_hover(pkgs, &spec),
            )));
        }
        // Immutable package navigation must not fall through to disk lookup for absent paths.
        return Ok(crate::identity::embedded_source(src.id())
            .is_some()
            .then(|| super::unavailable(query)));
    };
    match query {
        SourceQuery::Hover(_) => Ok(Some(import_path_hover(
            src,
            &import.node,
            package_docs(&pkg, &written, desc.as_deref()),
        ))),
        SourceQuery::Definition(_) => {
            let target = Source::new(
                id,
                crate::identity::embedded_source(id)
                    .context("missing builtin source")?
                    .into(),
            );
            let range = position::utf16_range(target.lines(), 0..target.text().len())
                .context("invalid embedded source range")?;
            Ok(Some(definition_reply(
                definition_uri(id, cfg.root(), client_root)?,
                range,
            )))
        }
        _ => Ok(Some(super::unavailable(query))),
    }
}

fn spec_completions(
    pkgs: &PackageAccess<'_>,
    src: &Source,
    path: std::ops::Range<usize>,
    prefix: &str,
    unterminated: bool,
) -> Result<Vec<CompletionItem>> {
    let range =
        position::utf16_range(src.lines(), path).context("invalid package completion range")?;
    let mut items = Vec::new();
    for (spec, pkg_desc) in importable_packages(pkgs) {
        let name = spec.to_string();
        if !name.starts_with(prefix) {
            continue;
        }
        let (detail, documentation) = match builtin_package(&spec) {
            Some(pkg) => {
                let manifest = manifest(&pkg)?;
                let desc = description(&pkg, &manifest)?;
                let docs = package_docs(&pkg, &name, desc.as_deref());
                (desc, Some(super::markdown_documentation(docs)))
            }
            None => {
                let desc = pkg_desc.as_deref().map(str::to_owned);
                let docs = desc.as_deref().map(super::markdown_documentation);
                (desc, docs)
            }
        };
        let new_text = with_closing_quote(name.clone(), unterminated);
        items.push(CompletionItem {
            kind: Some(CompletionItemKind::MODULE),
            detail,
            documentation,
            text_edit: Some(CompletionTextEdit::Edit(TextEdit { range, new_text })),
            label: name,
            ..CompletionItem::default()
        });
    }
    items.sort_unstable_by(|l, r| l.label.cmp(&r.label));
    Ok(items)
}

fn definition_reply(uri: Uri, range: lsp_types::Range) -> SourceReply {
    SourceReply::Definition(Some(GotoDefinitionResponse::Scalar(Location {
        uri,
        range,
    })))
}

fn import_path_hover(src: &Source, node: &LinkedNode<'_>, text: String) -> SourceReply {
    SourceReply::Hover(Some(crate::protocol::markdown_hover(
        text,
        position::utf16_range(src.lines(), node.range()),
    )))
}

fn local_imported_source(from: FileId, written: &str, pkgs: &PackageAccess<'_>) -> Option<FileId> {
    if written.starts_with('@') {
        let spec = written.parse::<PackageSpec>().ok()?;
        return pkgs
            .local_packages
            .package(&spec)
            .map(tola_typst::LocalPackage::entry_id);
    }
    let VirtualRoot::Package(spec) = from.root() else {
        return None;
    };
    pkgs.local_packages.package(spec)?;
    tola_typst::typst::foundations::PathOrStr::Str(written.into())
        .resolve(from)
        .ok()
        .map(|path| path.intern())
}

fn verified_package_view(id: FileId, dir: &Path) -> Option<PathBuf> {
    let src = crate::identity::embedded_source(id)?;
    let path = crate::identity::package_view_path(id, dir)?;
    crate::identity::verify_package_view(&path, &src)
        .ok()
        .map(|()| path)
}

fn definition_uri(
    id: FileId,
    site_root: &Path,
    client_root: &crate::uri::ClientRoot,
) -> Result<Uri> {
    let VirtualRoot::Package(_) = id.root() else {
        return crate::identity::client_uri(id, site_root, client_root)
            .context("a site file has a client address");
    };
    let view = tola_input::layout::builtin_packages(site_root);
    match verified_package_view(id, &view) {
        Some(mirror) => Ok(client_root.address(&mirror)?),
        None => Ok(crate::identity::source_uri(id, site_root)?),
    }
}

fn version_completions(
    pkgs: &PackageAccess<'_>,
    src: &Source,
    prefix: &str,
    ver_start: usize,
    path_end: usize,
    unterminated: bool,
) -> Vec<CompletionItem> {
    let Some((written, typed)) = prefix.split_once(':') else {
        return Vec::new();
    };
    let typed = typed.trim_start();
    let Some((ns, name)) = namespace_and_name(written) else {
        return Vec::new();
    };
    let mut versions: Vec<PackageVersion> = installed_versions(pkgs, ns, name);
    versions.extend(published_versions(pkgs.published, ns, name).unwrap_or_default());
    if ns == TOLA_NAMESPACE {
        versions.extend(
            builtin_packages()
                .filter(|p| p.spec().namespace.as_str() == ns && p.spec().name.as_str() == name)
                .map(|p| p.spec().version),
        );
    }
    versions.sort();
    versions.dedup();
    let Some(range) = position::utf16_range(src.lines(), ver_start..path_end) else {
        return Vec::new();
    };
    versions
        .into_iter()
        .filter(|v| v.to_string().starts_with(typed))
        .map(|v| {
            let ver = v.to_string();
            CompletionItem {
                kind: Some(CompletionItemKind::VALUE),
                detail: Some(format!("@{ns}/{name}")),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range,
                    new_text: with_closing_quote(ver.clone(), unterminated),
                })),
                label: ver,
                ..CompletionItem::default()
            }
        })
        .collect()
}

fn namespace_and_name(written: &str) -> Option<(&str, &str)> {
    let (ns, name) = written.strip_prefix('@')?.split_once('/')?;
    (!ns.is_empty() && !name.is_empty()).then_some((ns, name))
}

/// Only `@tola/...` specs offer builtin spelling/version fixes.
pub(crate) fn replacement(source: &Source, cursor: usize) -> Option<TextEdit> {
    let ImportPath { path, .. } = import_path(source, cursor)?;
    let spec = source.text()[path.clone()].parse::<PackageSpec>().ok()?;
    if spec.namespace.as_str() != TOLA_NAMESPACE || builtin_package(&spec).is_some() {
        return None;
    }
    let pkg = builtin_packages()
        .find(|p| p.spec().name.as_str() == spec.name.as_str())
        .or_else(|| nearest_builtin(spec.name.as_str()))?;
    Some(TextEdit {
        range: position::utf16_range(source.lines(), path)?,
        new_text: pkg.spec().to_string(),
    })
}

fn nearest_builtin(name: &str) -> Option<BuiltinPackage> {
    let pkgs: Vec<BuiltinPackage> = builtin_packages().collect();
    let names: Vec<String> = pkgs.iter().map(|p| p.spec().name.to_string()).collect();
    let closest = tola_typst::spelling::closest(name, names.iter().map(String::as_str))?.to_owned();
    pkgs.into_iter().find(|p| p.spec().name.as_str() == closest)
}

fn spec_hover(pkgs: &PackageAccess<'_>, spec: &PackageSpec) -> String {
    let mut text = format!("**{spec}**");
    if let Some(pkg) = builtin_packages()
        .find(|p| p.spec().namespace == spec.namespace && p.spec().name == spec.name)
    {
        text.push_str(&format!("\n\nTola provides `{}`.", pkg.spec()));
        return text;
    }
    let installed = installed_versions(pkgs, &spec.namespace, &spec.name);
    if !installed.is_empty() {
        let versions: Vec<String> = installed.iter().map(ToString::to_string).collect();
        text.push_str(&format!("\n\nInstalled: {}.", versions.join(", ")));
        return text;
    }
    text.push_str("\n\nNo installed copy");
    match published_versions(pkgs.published, &spec.namespace, &spec.name) {
        Some(versions) => {
            let versions: Vec<String> = versions.iter().map(ToString::to_string).collect();
            text.push_str(&format!("; published: {}.", versions.join(", ")));
            if let Some(entry) = newest_published_entry(pkgs.published, spec.name.as_str()) {
                if let Some(desc) = &entry.description {
                    text.push_str(&format!("\n\n{desc}"));
                }
                let links: Vec<String> = [
                    ("Repository", entry.repository.as_deref()),
                    ("Homepage", entry.homepage.as_deref()),
                ]
                .into_iter()
                .filter_map(|(label, url)| url.map(|url| format!("[{label}](<{url}>)")))
                .collect();
                if !links.is_empty() {
                    text.push_str(&format!("\n\n{}", links.join(" · ")));
                }
            }
        }
        None => text.push_str("; a build resolves it."),
    }
    text
}

fn newest_published_entry<'a>(
    published: &'a [PublishedPackage],
    name: &str,
) -> Option<&'a PublishedPackage> {
    published
        .iter()
        .filter(|pkg| pkg.name == name)
        .filter_map(|pkg| Some((pkg.version.parse::<PackageVersion>().ok()?, pkg)))
        .max_by_key(|(ver, _)| *ver)
        .map(|(_, pkg)| pkg)
}

pub(super) fn published_versions(
    published: &[PublishedPackage],
    ns: &str,
    name: &str,
) -> Option<Vec<PackageVersion>> {
    if ns != "preview" {
        return None;
    }
    let mut versions: Vec<PackageVersion> = published
        .iter()
        .filter(|p| p.name == name)
        .filter_map(|p| p.version.parse().ok())
        .collect();
    versions.sort();
    versions.dedup();
    (!versions.is_empty()).then_some(versions)
}

fn manifest(pkg: &BuiltinPackage) -> Result<PackageManifest> {
    let raw = pkg
        .files()
        .find(|(path, _)| *path == "typst.toml")
        .context("embedded package manifest is missing")?
        .1;
    let manifest: PackageManifest = toml::from_str(&raw)?;
    manifest
        .validate(&pkg.spec())
        .map_err(|err| anyhow::anyhow!("{err}"))?;
    Ok(manifest)
}

fn entrypoint(pkg: &BuiltinPackage, manifest: &PackageManifest) -> Result<FileId> {
    let id = RootedPath::new(
        VirtualRoot::Package(pkg.spec().clone()),
        VirtualPath::new(&manifest.package.entrypoint)?,
    )
    .intern();
    crate::identity::embedded_source(id).context("embedded package entrypoint is missing")?;
    Ok(id)
}

fn description(pkg: &BuiltinPackage, manifest: &PackageManifest) -> Result<Option<String>> {
    if let Some(desc) = &manifest.package.description {
        return Ok(Some(desc.to_string()));
    }
    let id = entrypoint(pkg, manifest)?;
    Ok(crate::identity::embedded_source(id).and_then(|src| source_summary(&src)))
}

fn source_summary(text: &str) -> Option<String> {
    let lines = text
        .lines()
        .map(str::trim)
        .map_while(|line| line.strip_prefix("//"))
        .map(|line| line.trim_start_matches('/').trim())
        .take_while(|line| !line.is_empty())
        .map(|line| {
            line.split_once(" - ")
                .filter(|(identity, _)| identity.starts_with('@'))
                .map_or(line, |(_, desc)| desc)
        })
        .collect::<Vec<_>>();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn package_docs(pkg: &BuiltinPackage, name: &str, desc: Option<&str>) -> String {
    let mut text = format!("**{name}**");
    if let Some(desc) = desc {
        text.push_str("\n\n");
        text.push_str(desc);
    }
    // Import version is fixed, so point authors to the Tola release supplying the source.
    text.push_str(&format!(
        "\n\nSupplied by Tola {}; the import version `{}` is fixed, not that release's number.",
        pkg.host_version(),
        pkg.spec().version,
    ));
    text
}

fn imported_source(
    id: FileId,
    from: &str,
) -> Result<Option<(BuiltinPackage, FileId, Option<String>)>> {
    if from.starts_with('@') {
        let Ok(spec) = from.parse::<PackageSpec>() else {
            return Ok(None);
        };
        let Some(pkg) = builtin_package(&spec) else {
            return Ok(None);
        };
        let manifest = manifest(&pkg)?;
        let id = entrypoint(&pkg, &manifest)?;
        let desc = description(&pkg, &manifest)?;
        Ok(Some((pkg, id, desc)))
    } else {
        let Some(target) = crate::identity::embedded_relative(id, from) else {
            return Ok(None);
        };
        let VirtualRoot::Package(spec) = target.root() else {
            return Ok(None);
        };
        let pkg = builtin_package(spec).context("embedded package is missing")?;
        let desc = crate::identity::embedded_source(target).and_then(|src| source_summary(&src));
        Ok(Some((pkg, target, desc)))
    }
}

pub(crate) fn package_source_definition(
    response: &mut GotoDefinitionResponse,
    directory: &Path,
    client_root: &crate::uri::ClientRoot,
    cancellation: &BuildCancellation,
) -> Result<()> {
    match response {
        GotoDefinitionResponse::Scalar(loc) => {
            package_source_uri(&mut loc.uri, directory, client_root, cancellation)
        }
        GotoDefinitionResponse::Array(locs) => {
            for loc in locs {
                package_source_uri(&mut loc.uri, directory, client_root, cancellation)?;
            }
            Ok(())
        }
        GotoDefinitionResponse::Link(links) => {
            for link in links {
                package_source_uri(&mut link.target_uri, directory, client_root, cancellation)?;
            }
            Ok(())
        }
    }
}

fn package_source_uri(
    uri: &mut Uri,
    dir: &Path,
    client_root: &crate::uri::ClientRoot,
    cancel: &BuildCancellation,
) -> Result<()> {
    if !uri.as_str().starts_with("tola-package:") {
        let Ok(path) = crate::uri::to_site_path(uri.as_str()) else {
            return Ok(());
        };
        if path.starts_with(dir) {
            *uri = client_root.address(&path)?;
        }
        return Ok(());
    }
    cancel.ensure_active()?;
    let id = crate::identity::file_id(uri, client_root.resolved())?;
    if !matches!(id.root(), VirtualRoot::Package(_)) {
        return Ok(());
    }
    let mirror = verified_package_view(id, dir);
    cancel.ensure_active()?;
    if let Some(path) = mirror {
        *uri = client_root.address(&path)?;
    }
    Ok(())
}

pub(crate) fn missing_import(
    source: &Source,
    names: &SourceNames,
    at: usize,
) -> Vec<(String, TextEdit)> {
    if names.declared_at(at).is_some() {
        return Vec::new();
    }
    let Some(name) =
        tola_typst_syntax::syntax::name(source, at).and_then(|range| source.text().get(range))
    else {
        return Vec::new();
    };
    builtin_packages()
        .filter(|pkg| pkg.exports().any(|export| export == name))
        .map(|pkg| {
            let path = pkg.spec().to_string();
            (
                format!("import `{name}` from `{path}`"),
                TextEdit {
                    range: import_position(source),
                    new_text: format!("#import \"{path}\": {name}\n"),
                },
            )
        })
        .collect()
}
