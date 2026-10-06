//! Import paths resolve against builtin, configured local and published packages
//! without requiring a successful Bundle.

use std::path::Path;

use anyhow::{Context, Result};
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    GotoDefinitionResponse, Location, TextEdit, Uri,
};
use tola_packages::{BuiltinPackage, TOLA_NAMESPACE, builtin_package, builtin_packages};
use tola_typst::typst::syntax::package::{PackageManifest, PackageSpec, PackageVersion};
use tola_typst::typst::syntax::{
    FileId, LinkedNode, RootedPath, Side, Source, SyntaxKind, VirtualPath, VirtualRoot, ast,
};

use crate::nearest;
use crate::packages::{PackageAccess, importable_packages, installed_versions};
use crate::position;
use crate::protocol::{SourceQuery, SourceReply};
use crate::published::PublishedPackage;

struct ImportPath<'a> {
    /// The string the path is written in, or the unfinished token standing in for one.
    node: LinkedNode<'a>,
    /// The range of the path itself, inside the quotation marks.
    path: std::ops::Range<usize>,
    unterminated: bool,
}

fn import_path(source: &Source, cursor: usize) -> Option<ImportPath<'_>> {
    let node = LinkedNode::new(source.root()).leaf_at(cursor, Side::Before)?;
    let unterminated = node.kind() == SyntaxKind::Error
        && node.leaf_text().starts_with('"')
        && matches!(
            node.parent_kind(),
            Some(SyntaxKind::ModuleImport | SyntaxKind::ModuleInclude)
        );
    if !tola_typst_syntax::syntax::is_import_path(&node) && !unterminated {
        return None;
    }
    let start = node.range().start + 1;
    let end = if unterminated {
        // An unclosed string token can consume the rest of the program. A path
        // edit must close this line without deleting the following source.
        source.text()[start..node.range().end]
            .find(['\r', '\n'])
            .map_or(node.range().end, |end| start + end)
    } else {
        node.range().end - 1
    };
    (cursor >= start && cursor <= end).then_some(ImportPath {
        path: start..end,
        unterminated,
        node,
    })
}

pub(super) fn respond(
    package_access: &PackageAccess<'_>,
    source: &Source,
    cursor: usize,
    query: &SourceQuery,
    client_root: &crate::uri::ClientRoot,
) -> Result<Option<SourceReply>> {
    let Some(ImportPath {
        node,
        path,
        unterminated,
    }) = import_path(source, cursor)
    else {
        return Ok(None);
    };
    let literal = node.cast::<ast::Str>();
    let (content_start, content_end) = (path.start, path.end);
    let prefix = &source.text()[content_start..cursor];
    if matches!(query, SourceQuery::Completion(_)) && prefix.contains(':') {
        let version_start = content_start + prefix.rfind(':').expect("a version separator") + 1;
        return Ok(Some(SourceReply::Completion(CompletionResponse::Array(
            version_completions(
                package_access,
                source,
                prefix,
                version_start,
                content_end,
                unterminated,
            ),
        ))));
    }
    // A bare quote names a file the site has, so the completion that answers it leads: the
    // package catalogue joins only once the author writes the `@` a package spec starts with.
    if matches!(query, SourceQuery::Completion(_)) && prefix.starts_with('@') {
        let range = position::utf16_range(source.lines(), content_start..content_end)
            .context("invalid package completion range")?;
        let mut items = Vec::new();
        for (spec, package_description) in importable_packages(package_access) {
            let name = spec.to_string();
            if !name.starts_with(prefix) {
                continue;
            }
            let (description, documentation) = match builtin_package(&spec) {
                Some(package) => {
                    let manifest = manifest(&package)?;
                    let description = description(&package, &manifest)?;
                    let documentation = package_docs(&package, &name, description.as_deref());
                    (
                        description,
                        Some(super::markdown_documentation(documentation)),
                    )
                }
                None => {
                    let description = package_description.map(|text| text.to_string());
                    let documentation = description.as_deref().map(super::markdown_documentation);
                    (description, documentation)
                }
            };
            let new_text = if unterminated {
                format!("{name}\"")
            } else {
                name.clone()
            };
            items.push(CompletionItem {
                kind: Some(CompletionItemKind::MODULE),
                detail: description,
                documentation,
                text_edit: Some(CompletionTextEdit::Edit(TextEdit { range, new_text })),
                label: name,
                ..CompletionItem::default()
            });
        }
        items.sort_unstable_by(|left, right| left.label.cmp(&right.label));
        return Ok(Some(SourceReply::Completion(CompletionResponse::Array(
            items,
        ))));
    }
    let Some(literal) = literal else {
        return Ok(None);
    };
    let Some((package, id, description)) = imported_source(source.id(), &literal.get())? else {
        if matches!(query, SourceQuery::Hover(_))
            && let Ok(spec) = literal
                .get()
                .parse::<tola_typst::typst::syntax::package::PackageSpec>()
        {
            return Ok(Some(SourceReply::Hover(Some(
                crate::protocol::markdown_hover(
                    spec_hover(package_access, &spec),
                    position::utf16_range(source.lines(), node.range()),
                ),
            ))));
        }
        // Immutable package navigation must never fall through to disk/package
        // lookup for a path absent from the builtin registry.
        return Ok((source.id().root() != &VirtualRoot::Project).then(|| super::unavailable(query)));
    };
    match query {
        SourceQuery::Hover(_) => Ok(Some(SourceReply::Hover(Some(
            crate::protocol::markdown_hover(
                package_docs(&package, &literal.get(), description.as_deref()),
                position::utf16_range(source.lines(), node.range()),
            ),
        )))),
        SourceQuery::Definition(_) => {
            let target = Source::new(
                id,
                crate::identity::embedded_source(id)
                    .context("missing builtin source")?
                    .into(),
            );
            let range = position::utf16_range(target.lines(), 0..target.text().len())
                .context("invalid embedded source range")?;
            Ok(Some(SourceReply::Definition(Some(
                GotoDefinitionResponse::Scalar(Location {
                    uri: definition_uri(id, target.text(), client_root)?,
                    range,
                }),
            ))))
        }
        _ => Ok(Some(super::unavailable(query))),
    }
}

/// The editor's address for one definition target.
///
/// A package document has no site path, so a file-only client reaches it through the mirror this
/// site publishes: the definition names that file, addressed in the spelling the client named its
/// root by. A site whose mirror is absent or differs from the source this Tola ships keeps the
/// package URI, which is what a client that knows the scheme opens. A site file is a path, and is
/// addressed in the client's spelling for the same reason.
fn definition_uri(id: FileId, source: &str, client_root: &crate::uri::ClientRoot) -> Result<Uri> {
    let resolved = client_root.resolved();
    if !matches!(id.root(), VirtualRoot::Package(_)) {
        return client_root.address(&resolved.join(id.vpath().get_without_slash()));
    }
    let view = resolved.join(tola_build::filesystem::PACKAGE_MIRROR_DIRECTORY);
    let mirror = crate::identity::package_view_path(id, &view)?;
    match crate::identity::verify_package_view(&mirror, source) {
        Ok(()) => client_root.address(&mirror),
        Err(_) => crate::identity::source_uri(id, resolved),
    }
}

fn version_completions(
    package_access: &PackageAccess<'_>,
    source: &Source,
    prefix: &str,
    version_start: usize,
    content_end: usize,
    unterminated: bool,
) -> Vec<CompletionItem> {
    let Some((namespace, name)) = namespace_and_name(prefix) else {
        return Vec::new();
    };
    let mut versions: Vec<PackageVersion> = installed_versions(package_access, namespace, name);
    if namespace == "preview" {
        versions.extend(
            published_versions(package_access.published, namespace, name)
                .into_iter()
                .flatten(),
        );
    }
    if namespace == TOLA_NAMESPACE {
        versions.extend(
            builtin_packages()
                .filter(|package| {
                    package.spec().namespace.as_str() == namespace
                        && package.spec().name.as_str() == name
                })
                .map(|package| package.spec().version),
        );
    }
    // Versions order by their numbers, not by their spelling: `0.9.0` precedes `0.10.0`.
    versions.sort();
    versions.dedup();
    let typed = prefix
        .split_once(':')
        .map_or("", |(_, version)| version)
        .trim_start();
    let Some(range) = position::utf16_range(source.lines(), version_start..content_end) else {
        return Vec::new();
    };
    versions
        .into_iter()
        .filter(|version| version.to_string().starts_with(typed))
        .map(|version| {
            let version = version.to_string();
            let new_text = if unterminated {
                format!("{version}\"")
            } else {
                version.clone()
            };
            CompletionItem {
                kind: Some(CompletionItemKind::VALUE),
                detail: Some(format!("@{namespace}/{name}")),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit { range, new_text })),
                label: version,
                ..CompletionItem::default()
            }
        })
        .collect()
}

fn namespace_and_name(prefix: &str) -> Option<(&str, &str)> {
    let (written, _) = prefix.split_once(':')?;
    let remainder = written.strip_prefix('@')?;
    let (namespace, name) = remainder.split_once('/')?;
    (!namespace.is_empty() && !name.is_empty()).then_some((namespace, name))
}

/// Only a `@tola/...` path can be a misspelling of a builtin: another namespace names a package
/// Tola does not publish, and a path that already resolves needs no fix.
pub(crate) fn replacement(source: &Source, cursor: usize) -> Option<TextEdit> {
    let ImportPath { path, .. } = import_path(source, cursor)?;
    let spec = source.text()[path.clone()]
        .parse::<tola_typst::typst::syntax::package::PackageSpec>()
        .ok()?;
    if spec.namespace.as_str() != TOLA_NAMESPACE || builtin_package(&spec).is_some() {
        return None;
    }
    // A name Tola publishes at another version is a certain replacement, not a spelling guess:
    // the site cannot resolve the version it wrote, and it resolves exactly one other.
    let package = builtin_packages()
        .find(|package| package.spec().name.as_str() == spec.name.as_str())
        .or_else(|| nearest_builtin(spec.name.as_str()))?;
    Some(TextEdit {
        range: position::utf16_range(source.lines(), path)?,
        new_text: package.spec().to_string(),
    })
}

fn nearest_builtin(name: &str) -> Option<BuiltinPackage> {
    let packages: Vec<(String, BuiltinPackage)> = builtin_packages()
        .map(|package| (package.spec().name.to_string(), package))
        .collect();
    let closest =
        nearest::closest(name, packages.iter().map(|(name, _)| name.as_str()))?.to_owned();
    packages
        .into_iter()
        .find(|(candidate, _)| *candidate == closest)
        .map(|(_, package)| package)
}

fn spec_hover(package_access: &PackageAccess<'_>, spec: &PackageSpec) -> String {
    let mut text = format!("**{spec}**");
    if let Some(package) = builtin_packages().find(|package| {
        package.spec().namespace == spec.namespace && package.spec().name == spec.name
    }) {
        text.push_str(&format!("\n\nTola provides `{}`.", package.spec()));
        return text;
    }
    let installed = installed_versions(package_access, &spec.namespace, &spec.name);
    if !installed.is_empty() {
        let versions: Vec<String> = installed
            .iter()
            .map(|version| version.to_string())
            .collect();
        text.push_str(&format!("\n\nInstalled: {}.", versions.join(", ")));
        return text;
    }
    text.push_str("\n\nNo installed copy");
    match published_versions(package_access.published, &spec.namespace, &spec.name) {
        Some(versions) => {
            let versions: Vec<String> = versions.iter().map(ToString::to_string).collect();
            text.push_str(&format!("; published: {}.", versions.join(", ")));
            if let Some(entry) = published_entry(package_access.published, spec) {
                if let Some(description) = &entry.description {
                    text.push_str(&format!("\n\n{description}"));
                }
                let links: Vec<String> = [
                    ("Repository", entry.repository.as_deref()),
                    ("Homepage", entry.homepage.as_deref()),
                ]
                .into_iter()
                .filter_map(|(label, url)| {
                    let url = url?;
                    Some(format!("[{label}](<{url}>)"))
                })
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

/// The published entry a package specification names, or the newest one the index has.
fn published_entry<'a>(
    published: &'a [PublishedPackage],
    spec: &PackageSpec,
) -> Option<&'a PublishedPackage> {
    if spec.namespace != "preview" {
        return None;
    }
    published
        .iter()
        .filter(|package| package.name == spec.name)
        .filter_map(|package| {
            let version = package.version.parse::<PackageVersion>().ok()?;
            Some((version, package))
        })
        .max_by_key(|(version, _)| *version)
        .map(|(_, package)| package)
}

/// The published versions of one `@preview` package, oldest first.
///
/// A version a package specification cannot name is left out: an import resolves by parsing the
/// specification, so a version the index spells otherwise names no package.
pub(super) fn published_versions(
    published: &[PublishedPackage],
    namespace: &str,
    name: &str,
) -> Option<Vec<PackageVersion>> {
    if namespace != "preview" {
        return None;
    }
    let mut versions: Vec<PackageVersion> = published
        .iter()
        .filter(|published| published.name == name)
        .filter_map(|published| published.version.parse().ok())
        .collect();
    versions.sort();
    versions.dedup();
    (!versions.is_empty()).then_some(versions)
}

fn manifest(package: &BuiltinPackage) -> Result<PackageManifest> {
    let manifest = package
        .files()
        .find(|(path, _)| *path == "typst.toml")
        .context("embedded package manifest is missing")?
        .1;
    let manifest: PackageManifest = toml::from_str(&manifest)?;
    manifest
        .validate(&package.spec())
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    Ok(manifest)
}

fn entrypoint(package: &BuiltinPackage, manifest: &PackageManifest) -> Result<FileId> {
    let id = RootedPath::new(
        VirtualRoot::Package(package.spec().clone()),
        VirtualPath::new(&manifest.package.entrypoint)?,
    )
    .intern();
    crate::identity::embedded_source(id).context("embedded package entrypoint is missing")?;
    Ok(id)
}

fn description(package: &BuiltinPackage, manifest: &PackageManifest) -> Result<Option<String>> {
    if let Some(description) = &manifest.package.description {
        return Ok(Some(description.to_string()));
    }
    let id = entrypoint(package, manifest)?;
    Ok(crate::identity::embedded_source(id).and_then(|source| source_summary(&source)))
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
                .map_or(line, |(_, description)| description)
        })
        .collect::<Vec<_>>();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn package_docs(package: &BuiltinPackage, name: &str, description: Option<&str>) -> String {
    let mut text = format!("**{name}**");
    if let Some(description) = description {
        text.push_str("\n\n");
        text.push_str(description);
    }
    // The import version is fixed, so an author who reads it as a Tola release number is told
    // which release actually supplies these sources.
    text.push_str(&format!(
        "\n\nSupplied by Tola {}; the import version `{}` is fixed, not that release's number.",
        package.host_version(),
        package.spec().version,
    ));
    text
}

fn imported_source(
    id: FileId,
    from: &str,
) -> Result<Option<(BuiltinPackage, FileId, Option<String>)>> {
    if from.starts_with('@') {
        let Ok(spec) = from.parse::<tola_typst::typst::syntax::package::PackageSpec>() else {
            return Ok(None);
        };
        let Some(package) = builtin_package(&spec) else {
            return Ok(None);
        };
        let manifest = manifest(&package)?;
        let id = entrypoint(&package, &manifest)?;
        let description = description(&package, &manifest)?;
        Ok(Some((package, id, description)))
    } else {
        let Some(target) = crate::identity::embedded_relative(id, from) else {
            return Ok(None);
        };
        let VirtualRoot::Package(spec) = target.root() else {
            return Ok(None);
        };
        let package = builtin_package(spec).context("embedded package is missing")?;
        let description =
            crate::identity::embedded_source(target).and_then(|source| source_summary(&source));
        Ok(Some((package, target, description)))
    }
}

/// File-only clients use host-prepared mirrors. Missing or modified mirrors are
/// errors; this projection never writes them.
pub(crate) fn package_source_definition(
    response: &mut GotoDefinitionResponse,
    directory: &Path,
    client_root: &crate::uri::ClientRoot,
    cancellation: &tola_build::cancellation::BuildCancellation,
) -> Result<()> {
    match response {
        GotoDefinitionResponse::Scalar(location) => {
            package_source_uri(&mut location.uri, directory, client_root, cancellation)
        }
        GotoDefinitionResponse::Array(locations) => {
            for location in locations {
                package_source_uri(&mut location.uri, directory, client_root, cancellation)?;
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
    directory: &Path,
    client_root: &crate::uri::ClientRoot,
    cancellation: &tola_build::cancellation::BuildCancellation,
) -> Result<()> {
    if !uri.as_str().starts_with("tola-package:") {
        // A mirror a client reads as a file is answered in the spelling that client named its root
        // by, never in the resolved form the mirror is stored under.
        let Ok(path) = crate::uri::to_site_path(uri.as_str()) else {
            return Ok(());
        };
        if path.starts_with(directory) {
            *uri = client_root.address(&path)?;
        }
        return Ok(());
    }
    cancellation.ensure_active()?;
    // The mirror is read where identity resolves it and answered where the client spelled it: the
    // site's packages live below the resolved root, and the editor named a different spelling of
    // that same root.
    let id = crate::identity::file_id(uri, client_root.resolved())?;
    if !matches!(id.root(), VirtualRoot::Package(_)) {
        return Ok(());
    }
    let expected =
        crate::identity::embedded_source(id).context("unknown embedded package source")?;
    let path = crate::identity::package_view_path(id, directory)?;
    cancellation.ensure_active()?;
    crate::identity::verify_package_view(&path, &expected)?;
    *uri = client_root.address(&path)?;
    Ok(())
}
pub(crate) fn missing_import(
    source: &Source,
    names: &tola_typst_syntax::names::SourceNames,
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
    // Every package that publishes the name offers its own import, so a name two of them export
    // leaves the choice to the author rather than to the order they are listed in.
    builtin_packages()
        .filter(|package| package.exports().any(|export| export == name))
        .map(|package| {
            let path = package.spec().to_string();
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

fn import_position(source: &Source) -> lsp_types::Range {
    let lines = source.lines();
    // A source's root is its markup, and an import statement is a `Hash` node with the import
    // following it, so the walk pairs the two and stops at the first statement of another kind.
    let mut at = 0;
    let mut hashed = false;
    for child in LinkedNode::new(source.root()).children() {
        match child.kind() {
            SyntaxKind::Hash => hashed = true,
            SyntaxKind::ModuleImport if hashed => {
                // The insertion point is the protocol line after the import's own: the protocol
                // breaks lines where the client does, never where the compiler's index does.
                let next = position::utf16_range(lines, child.range().end..child.range().end)
                    .and_then(|end| {
                        position::byte_offset(
                            lines,
                            lsp_types::Position::new(end.start.line + 1, 0),
                        )
                        .ok()
                    });
                at = next.unwrap_or(child.range().end);
                hashed = false;
            }
            SyntaxKind::Space | SyntaxKind::Parbreak => {}
            _ => break,
        }
    }
    position::utf16_range(lines, at..at).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::tests::*;
    use lsp_types::Position;
    use lsp_types::Range;
    use lsp_types::request as lsp_request;
    use lsp_types::request::Request as LspRequest;
    use serde_json::json;
    use tola_build::cancellation::BuildCancellation;
    use tola_build::config::ResolvedSiteConfig;

    fn site_config(root: &Path) -> ResolvedSiteConfig {
        std::fs::write(root.join("tola.toml"), "").unwrap();
        tola_build::config::loading::load_site_config(
            Some(&root.join("tola.toml")),
            tola_typst::PackageLocations::from_absolute_roots(None, None).unwrap(),
            &tola_build::config::loading::BuildOverrides::default(),
        )
        .unwrap()
        .into_config()
    }

    fn site_source(text: &str) -> Source {
        let id = RootedPath::new(
            VirtualRoot::Project,
            VirtualPath::new("content/document.typ").unwrap(),
        )
        .intern();
        Source::new(id, text.into())
    }

    fn completions(
        config: &ResolvedSiteConfig,
        published: &[PublishedPackage],
        marked: &str,
    ) -> Vec<String> {
        let (source, cursor) = marked_source(marked);
        let position = position::utf16_range(source.lines(), cursor..cursor)
            .unwrap()
            .start;
        let query = SourceQuery::decode(lsp_server::Request {
            id: 1.into(),
            method: <lsp_types::request::Completion as lsp_types::request::Request>::METHOD.into(),
            params: serde_json::json!({
                "textDocument": { "uri": "file:///site/content/document.typ" },
                "position": position,
            }),
        })
        .expect("a completion query")
        .1;
        let package_access =
            PackageAccess::new(config, &tola_build::BuildResources::new(), published);
        match respond(
            &package_access,
            &source,
            cursor,
            &query,
            &crate::uri::ClientRoot::new(config.get_root()),
        )
        .expect("a reply")
        {
            Some(SourceReply::Completion(CompletionResponse::Array(items))) => {
                items.into_iter().map(|item| item.label).collect()
            }
            other => panic!("a completion, got {other:?}"),
        }
    }

    /// The import-spec completions one site answers, over a site directory the call owns.
    fn import_completions(published: &[PublishedPackage], marked: &str) -> Vec<String> {
        let root = tempfile::tempdir().unwrap();
        let config = site_config(root.path());
        completions(&config, published, marked)
    }

    fn marked_source(marked: &str) -> (Source, usize) {
        let cursor = marked.find('|').expect("a query cursor");
        (site_source(&marked.replacen('|', "", 1)), cursor)
    }

    /// A definition into a package is answered as a file in the spelling the client named its root
    /// by: both the mirror path the compiler wrote and the package URI a scheme-aware client opens
    /// reach it, and a mirror that is not there is an error rather than an invented path.
    #[test]
    fn package_source_rewrite_keeps_the_client_spelling() {
        let directory = tempfile::tempdir().unwrap();
        let resolved = directory.path().canonicalize().unwrap();
        let spelled = directory.path().join("aliased");
        let spec: PackageSpec = "@tola/site:0.0.0".parse().unwrap();
        let package = builtin_package(&spec).expect("a builtin package");
        let id = entrypoint(&package, &manifest(&package).expect("a manifest")).expect("an entry");
        let source = crate::identity::embedded_source(id).expect("a builtin package source");
        let view = resolved.join(tola_build::filesystem::PACKAGE_MIRROR_DIRECTORY);
        let mirror = crate::identity::package_view_path(id, &view).unwrap();
        std::fs::create_dir_all(mirror.parent().unwrap()).unwrap();
        std::fs::write(&mirror, source.as_bytes()).unwrap();
        let client_root = crate::uri::ClientRoot::with_resolved(&spelled, &resolved);
        let expected =
            crate::uri::from_file_path(&spelled.join(mirror.strip_prefix(&resolved).unwrap()))
                .unwrap();
        for uri in [
            crate::uri::from_file_path(&mirror).unwrap(),
            crate::identity::source_uri(id, &resolved).unwrap(),
        ] {
            let mut response = GotoDefinitionResponse::Scalar(Location {
                uri,
                range: Range::default(),
            });
            package_source_definition(
                &mut response,
                &view,
                &client_root,
                &BuildCancellation::new(),
            )
            .unwrap();
            let GotoDefinitionResponse::Scalar(location) = &response else {
                panic!("one location: {response:?}");
            };
            assert_eq!(location.uri, expected);
            assert_ne!(
                location.uri,
                crate::uri::from_file_path(&mirror).unwrap(),
                "the answer must not be the resolved spelling"
            );
        }
        // A site whose mirror is not there keeps the package URI rather than inventing a path.
        std::fs::remove_file(&mirror).unwrap();
        assert!(
            package_source_definition(
                &mut GotoDefinitionResponse::Scalar(Location {
                    uri: crate::identity::source_uri(id, &resolved).unwrap(),
                    range: Range::default(),
                }),
                &view,
                &client_root,
                &BuildCancellation::new(),
            )
            .is_err(),
            "a missing mirror is an error, not a fabricated path"
        );
    }

    /// A definition into a package is a client URI: the mirror the site publishes, in the spelling
    /// the client named its root by, never the resolved form and never a URI only a scheme-aware
    /// client can open.
    #[test]
    fn package_definition_answers_the_client_spelling() {
        let directory = tempfile::tempdir().unwrap();
        let resolved = directory.path().canonicalize().unwrap();
        let spelled = directory.path().join("aliased");
        let config = site_config(&resolved);
        let spec: PackageSpec = "@tola/site:0.0.0".parse().unwrap();
        let package = builtin_package(&spec).expect("a builtin package");
        let id = entrypoint(&package, &manifest(&package).expect("a manifest")).expect("an entry");
        let source = crate::identity::embedded_source(id).expect("a builtin package source");
        let view = resolved.join(tola_build::filesystem::PACKAGE_MIRROR_DIRECTORY);
        let mirror = crate::identity::package_view_path(id, &view).unwrap();
        std::fs::create_dir_all(mirror.parent().unwrap()).unwrap();
        std::fs::write(&mirror, source.as_bytes()).unwrap();
        let (source, cursor) = marked_source("#import \"@tola/site:0.0.0|\"\n");
        let position = position::utf16_range(source.lines(), cursor..cursor)
            .unwrap()
            .start;
        let query = SourceQuery::decode(lsp_server::Request {
            id: 1.into(),
            method: <lsp_types::request::GotoDefinition as lsp_types::request::Request>::METHOD
                .into(),
            params: serde_json::json!({
                "textDocument": { "uri": "file:///site/content/document.typ" },
                "position": position,
            }),
        })
        .expect("a definition query")
        .1;
        let package_access = PackageAccess::new(&config, &tola_build::BuildResources::new(), &[]);
        let reply = respond(
            &package_access,
            &source,
            cursor,
            &query,
            &crate::uri::ClientRoot::with_resolved(&spelled, &resolved),
        )
        .expect("a reply");
        let Some(SourceReply::Definition(Some(GotoDefinitionResponse::Scalar(location)))) = reply
        else {
            panic!("a definition: {reply:?}");
        };
        let expected =
            crate::uri::from_file_path(&spelled.join(mirror.strip_prefix(&resolved).unwrap()))
                .unwrap();
        assert_eq!(location.uri, expected);
    }

    #[test]
    fn builtin_completes_its_version() {
        assert_eq!(
            import_completions(&[], "#import \"@tola/site:|\"\n"),
            ["0.0.0"]
        );
    }

    #[test]
    fn published_versions_stay_query_local() {
        let published = [PublishedPackage {
            name: "query-local".into(),
            version: "0.2.0".into(),
            description: None,
            homepage: None,
            repository: None,
        }];
        let marked = "#import \"@preview/query-local:|\"\n";
        assert_eq!(import_completions(&published, marked), ["0.2.0"]);
        assert!(import_completions(&[], marked).is_empty());
    }

    #[test]
    fn offline_completes_local_packages() {
        let root = tempfile::tempdir().unwrap();
        let packages = root.path().join("packages");
        std::fs::create_dir_all(packages.join("preview/supplied/1.2.3")).unwrap();
        std::fs::write(root.path().join("tola.toml"), "").unwrap();
        let config = tola_build::config::loading::load_site_config(
            Some(&root.path().join("tola.toml")),
            tola_typst::PackageLocations::from_absolute_roots(None, None)
                .unwrap()
                .with_declared_root(packages)
                .unwrap(),
            &tola_build::config::loading::BuildOverrides::default(),
        )
        .unwrap()
        .into_config();
        assert_eq!(
            completions(&config, &[], "#import \"@preview/sup|\n#unknown"),
            ["@preview/supplied:1.2.3"]
        );
    }

    #[test]
    fn published_names_complete_once_per_package() {
        let published = [
            PublishedPackage {
                name: "cetz".into(),
                version: "0.3.3".into(),
                description: None,
                homepage: None,
                repository: None,
            },
            PublishedPackage {
                name: "cetz".into(),
                version: "0.3.4".into(),
                description: None,
                homepage: None,
                repository: None,
            },
            PublishedPackage {
                name: "cetz-other".into(),
                version: "1.0.0".into(),
                description: None,
                homepage: None,
                repository: None,
            },
        ];
        assert_eq!(
            import_completions(&published, "#import \"@preview/ce|\"\n"),
            ["@preview/cetz-other:1.0.0", "@preview/cetz:0.3.4"],
            "a name position offers one entry per package, at its newest version"
        );
        assert_eq!(
            import_completions(&published, "#import \"@preview/cetz:|\"\n"),
            ["0.3.3", "0.3.4"],
            "a version position still offers every version"
        );
    }

    /// A version position lists versions by their numbers, not by their spelling.
    #[test]
    fn version_completions_order_by_number() {
        let published = [
            PublishedPackage {
                name: "cetz".into(),
                version: "0.10.0".into(),
                description: None,
                homepage: None,
                repository: None,
            },
            PublishedPackage {
                name: "cetz".into(),
                version: "0.9.0".into(),
                description: None,
                homepage: None,
                repository: None,
            },
        ];
        assert_eq!(
            import_completions(&published, "#import \"@preview/cetz:|\"\n"),
            ["0.9.0", "0.10.0"]
        );
    }

    #[test]
    fn unnamed_package_completes_no_version() {
        assert!(import_completions(&[], "#import \":|\"\n").is_empty());
    }

    #[test]
    fn misspelled_builtin_offers_its_name() {
        let (source, cursor) = marked_source("#import \"@tola/s|it:0.0.0\": site\n");
        let edit = replacement(&source, cursor).expect("one fix");
        assert_eq!(edit.new_text, "@tola/site:0.0.0");
        let written = "@tola/sit:0.0.0".len() as u32;
        assert_eq!(
            (edit.range.start.character, edit.range.end.character),
            (9, 9 + written),
            "the fix replaces the spec the author wrote"
        );
    }

    #[test]
    fn wrong_version_offers_the_builtin() {
        let (source, cursor) = marked_source("#import \"@tola/i|con:0.0.1\": icon\n");
        let edit = replacement(&source, cursor).expect("one fix");
        assert_eq!(edit.new_text, "@tola/icon:0.0.0");
    }

    #[test]
    fn resolving_specs_offer_no_fix() {
        for marked in [
            "#import \"@tola/s|ite:0.0.0\": site\n",
            "#import \"@tola/d|ocument:0.0.0\": current-document\n",
            "#import \"@preview/c|etz:0.3.1\": cetz\n",
            "#import \"@tola/z|zzzzzzz:0.0.0\": other\n",
            "#import \"templates/p|age.typ\": page\n",
        ] {
            let (source, cursor) = marked_source(marked);
            let fix = replacement(&source, cursor);
            assert!(fix.is_none(), "{marked:?} offers {fix:?}");
        }
    }

    #[test]
    fn file_only_clients_read_package_sources() {
        let directory = tempfile::tempdir().unwrap();
        let sources = directory.path().join("package-sources");
        let client_root = crate::uri::ClientRoot::new(directory.path());
        let uri: Uri = "tola-package:/tola/document/0.0.0/lib.typ".parse().unwrap();
        let id = crate::identity::file_id(&uri, directory.path()).unwrap();
        let path = sources.join("tola/document/0.0.0/lib.typ");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            crate::identity::embedded_source(id).unwrap().as_bytes(),
        )
        .unwrap();
        let range = Range::new(
            lsp_types::Position::new(1, 2),
            lsp_types::Position::new(3, 4),
        );
        let location = Location {
            uri: uri.clone(),
            range,
        };
        let definitions = [
            GotoDefinitionResponse::Scalar(location.clone()),
            GotoDefinitionResponse::Array(vec![location]),
        ];
        let file_uri: Uri = url::Url::from_file_path(&path)
            .unwrap()
            .as_str()
            .parse()
            .unwrap();
        for definition in &definitions {
            let mut projected = definition.clone();
            package_source_definition(
                &mut projected,
                &sources,
                &client_root,
                &BuildCancellation::new(),
            )
            .unwrap();
            match (projected, definition) {
                (
                    GotoDefinitionResponse::Scalar(projected),
                    GotoDefinitionResponse::Scalar(original),
                ) => {
                    assert_eq!(projected.uri, file_uri);
                    assert_eq!(projected.range, original.range);
                }
                (
                    GotoDefinitionResponse::Array(projected),
                    GotoDefinitionResponse::Array(original),
                ) => {
                    assert_eq!(projected[0].uri, file_uri);
                    assert_eq!(projected[0].range, original[0].range);
                }
                _ => panic!("definition shape changed"),
            }
        }
        std::fs::write(&path, "modified source").unwrap();
        for definition in &definitions {
            assert!(
                package_source_definition(
                    &mut definition.clone(),
                    &sources,
                    &client_root,
                    &BuildCancellation::new(),
                )
                .is_err()
            );
        }
        std::fs::remove_file(path).unwrap();
        for definition in &definitions {
            assert!(
                package_source_definition(
                    &mut definition.clone(),
                    &sources,
                    &client_root,
                    &BuildCancellation::new(),
                )
                .is_err()
            );
        }
    }

    /// A file-only client reads a site's packages as the mirrors editor setup publishes, so the
    /// server itself hands the editor a mirror path where it otherwise hands out a package
    /// document. The mirror answers as the immutable package source it has, whether the client
    /// named the view or the site's own published directory is read, and a path no builtin package
    /// file mirrors — an unknown package, or a copy this Tola did not write — answers nothing.
    #[test]
    fn package_mirror_answers_as_its_package_source() {
        let mut site = QuerySession::new();
        let directory = site.site.path(".tola/builtin-packages");
        let package = "tola-package:/tola/site/0.0.0/lib.typ".parse().unwrap();
        let id = crate::identity::file_id(&package, site.site.root()).expect("a builtin package");
        let source = crate::identity::embedded_source(id).expect("a builtin source");
        let needle = "@tola/host";
        let at = source
            .find(needle)
            .expect("the package imports the host site value")
            + needle.len();
        let position = position::utf16_range(Source::detached(source.to_string()).lines(), at..at)
            .expect("a position inside the source")
            .start;
        site.site
            .write(".tola/builtin-packages/tola/site/0.0.0/lib.typ", &source);
        site.site
            .write(".tola/builtin-packages/tola/web/0.0.0/lib.typ", "stale\n");
        let mirror =
            crate::uri::from_file_path(&directory.join("tola/site/0.0.0/lib.typ")).unwrap();
        for named in [Some(directory.clone()), None] {
            site.package_sources = named;
            let answer = site
                .hover_at(mirror.as_str(), position)
                .expect("an answerable request");
            assert!(answer.is_some(), "{answer:?}");
            assert_eq!(answer, site.hover_at(package.as_str(), position).unwrap());
            for unmirrored in [
                directory.join("tola/no-such-package/0.0.0/lib.typ"),
                directory.join("tola/web/0.0.0/lib.typ"),
            ] {
                let uri = crate::uri::from_file_path(&unmirrored).unwrap();
                assert!(site.hover_at(uri.as_str(), position).unwrap().is_none());
            }
        }
    }
    /// An embedded package document answers a completion as an ordinary result, never as an
    /// internal error.
    ///
    /// The site check produced no world, and an embedded package has no repair a query copy may
    /// write, so the request must come back answered.
    #[test]
    fn package_document_completes_without_world() {
        let mut site = QuerySession::with_program("#let broken = absent\n");
        let package: lsp_types::Uri = "tola-package:/tola/site/0.0.0/lib.typ".parse().unwrap();
        let id = crate::identity::file_id(&package, site.site.root()).expect("a builtin package");
        let text = crate::identity::embedded_source(id).expect("a builtin source");
        let source = Source::detached(text.to_string());
        let lines = source.lines();
        let import = text
            .lines()
            .position(|line| line.starts_with("#import"))
            .expect("the package's import line");
        let at = position::byte_offset(lines, Position::new(import as u32, 5)).expect("a cursor");
        let position = position::utf16_range(lines, at..at)
            .expect("a position")
            .start;
        let reply = site
            .try_reply_at(
                lsp_request::Completion::METHOD,
                package.as_str(),
                position,
                None,
                serde_json::json!({}),
            )
            .expect("an answered completion");
        // The cursor names no value to wrap, so the package answers an empty completion list.
        assert_eq!(reply, json!([]), "a completion list, not a failed request");
    }
    #[test]
    fn wrong_version_answers_the_builtin() {
        let mut site = QuerySession::new();
        let hover = site
            .hover_text("#import \"@tola/ico|n:0.0.1\": icon\n")
            .expect("a hover for the version");
        assert!(hover.contains("@tola/icon:0.0.0"), "{hover}");
    }

    /// A package the site does not include still answers with the versions it could import.
    #[test]
    fn uninstalled_package_hover_names_versions() {
        let mut site = QuerySession::new();

        let hover = site
            .hover_text("#import \"@preview/abse|nt:0.1.0\": absent\nBody")
            .expect("a hover for the package");
        assert!(hover.contains("@preview/absent"), "{hover}");
        assert!(hover.contains("No installed copy"), "{hover}");
    }
}
