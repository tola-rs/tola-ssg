//! Import paths resolve against builtin, configured local and published packages
//! without requiring a successful Bundle.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    GotoDefinitionResponse, Location, TextEdit, Uri,
};
use tola_build::cancellation::BuildCancellation;
use tola_build::config::ResolvedSiteConfig;
use tola_packages::{BuiltinPackage, TOLA_NAMESPACE, builtin_package, builtin_packages};
use tola_typst::typst::foundations::Repr;
use tola_typst::typst::syntax::package::{PackageManifest, PackageSpec, PackageVersion};
use tola_typst::typst::syntax::{
    FileId, LinkedNode, RootedPath, Side, Source, SyntaxKind, VirtualPath, VirtualRoot, ast,
};
use tola_typst_syntax::names::SourceNames;

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

/// One path argument as far as the cursor has written it, in the bytes a completion edits.
struct WrittenPath {
    /// The content bytes a completion replaces.
    range: std::ops::Range<usize>,
    /// Whether the string is still open, so the edit writes its closing quote.
    unterminated: bool,
    /// The file format the call's parameter accepts, when it narrows suggestions.
    extension: Option<&'static str>,
}

fn import_path(source: &Source, cursor: usize) -> Option<ImportPath<'_>> {
    let node = LinkedNode::new(source.root()).leaf_at(cursor, Side::Before)?;
    let unclosed = node.kind() == SyntaxKind::Error
        && node.leaf_text().starts_with('"')
        && matches!(
            node.parent_kind(),
            Some(SyntaxKind::ModuleImport | SyntaxKind::ModuleInclude)
        );
    if !tola_typst_syntax::syntax::is_import_path(&node) && !unclosed {
        return None;
    }
    let start = node.range().start + 1;
    let line_end = string_line_end(source.text(), start, node.range().end);
    let unterminated = unclosed || line_end < node.range().end;
    let end = if unterminated {
        line_end
    } else {
        node.range().end - 1
    };
    (cursor >= start && cursor <= end).then_some(ImportPath {
        path: start..end,
        unterminated,
        node,
    })
}

/// A path is written on one line, so an unfinished string's edit stops at its own line break.
fn string_line_end(text: &str, start: usize, token_end: usize) -> usize {
    text[start..token_end]
        .find(['\r', '\n'])
        .map_or(token_end, |offset| start + offset)
}

/// The edit text one import completion writes: a string the author has not closed takes the
/// closing quote.
fn with_closing_quote(mut content: String, unterminated: bool) -> String {
    if unterminated {
        content.push('"');
    }
    content
}

/// One package-path query: the site, the package access it reads, and the editor's cursor.
pub(super) struct PackageQuery<'a, 'b> {
    pub(super) config: &'a ResolvedSiteConfig,
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
        config,
        package_access,
        source,
        cursor,
        query,
        client_root,
        cancellation,
        overrides,
        indexed_names,
    } = inputs;
    let completing = matches!(query, SourceQuery::Completion(_));
    let import = import_path(source, cursor);
    // A completion answers the spelling being written, before a definition or a hover answers it.
    if completing {
        let offered = match &import {
            Some(import) => import_completions(
                config,
                package_access,
                source,
                cursor,
                import,
                cancellation,
                overrides,
            )?,
            None => reader_path_completions(
                config,
                source,
                cursor,
                indexed_names,
                cancellation,
                overrides,
                &package_access.boundary,
            )?,
        };
        if let Some(items) = offered {
            let reply = SourceReply::Completion(CompletionResponse::Array(items));
            return Ok(Some(reply));
        }
    }
    let Some(import) = import else {
        return Ok(None);
    };
    if let Some(reply) = local_module_answer(
        config,
        package_access,
        source,
        &import,
        query,
        client_root,
        overrides,
    )? {
        return Ok(Some(reply));
    }
    builtin_answer(config, package_access, source, &import, query, client_root)
}

/// The completions a path argument the site's own calls read offers, when the cursor writes one.
fn reader_path_completions(
    config: &ResolvedSiteConfig,
    source: &Source,
    cursor: usize,
    indexed_names: Option<&SourceNames>,
    cancellation: &BuildCancellation,
    overrides: &[(PathBuf, Arc<str>)],
    boundary: &tola_typst::SourceBoundary,
) -> Result<Option<Vec<CompletionItem>>> {
    let Some(written) = reader_path(source, cursor, indexed_names) else {
        return Ok(None);
    };
    site_path_completion(
        config,
        source,
        cursor,
        &written,
        cancellation,
        overrides,
        boundary,
    )
}

/// The completions one import path offers; `None` when the path names a package source.
fn import_completions(
    config: &ResolvedSiteConfig,
    package_access: &PackageAccess<'_>,
    source: &Source,
    cursor: usize,
    import: &ImportPath<'_>,
    cancellation: &BuildCancellation,
    overrides: &[(PathBuf, Arc<str>)],
) -> Result<Option<Vec<CompletionItem>>> {
    let prefix = &source.text()[import.path.start..cursor];
    if prefix.starts_with('@') && prefix.contains(':') {
        let version_start = import.path.start + prefix.rfind(':').expect("a version separator") + 1;
        return Ok(Some(version_completions(
            package_access,
            source,
            prefix,
            version_start,
            import.path.end,
            import.unterminated,
        )));
    }
    if prefix.starts_with('@') {
        return Ok(Some(spec_completions(
            package_access,
            source,
            import.path.clone(),
            prefix,
            import.unterminated,
        )?));
    }
    // A bare quote names a site file: the typed spelling decides the labels and the edit.
    let written = WrittenPath {
        range: import.path.clone(),
        unterminated: import.unterminated,
        extension: Some("typ"),
    };
    site_path_completion(
        config,
        source,
        cursor,
        &written,
        cancellation,
        overrides,
        &package_access.boundary,
    )
}

/// The answer a path naming a locally declared module source gets, when it names one.
fn local_module_answer(
    config: &ResolvedSiteConfig,
    package_access: &PackageAccess<'_>,
    source: &Source,
    import: &ImportPath<'_>,
    query: &SourceQuery,
    client_root: &crate::uri::ClientRoot,
    overrides: &[(PathBuf, Arc<str>)],
) -> Result<Option<SourceReply>> {
    let Some(literal) = import.node.cast::<ast::Str>() else {
        return Ok(None);
    };
    let written = literal.get();
    let Some(id) = local_imported_source(source.id(), &written, package_access) else {
        return Ok(None);
    };
    match query {
        SourceQuery::Definition(_) => {
            if super::context::load_source(id, config, overrides, package_access)?.is_none() {
                return Ok(Some(super::unavailable(query)));
            }
            let Some(path) = crate::identity::package_file_path_with_packages(
                id,
                &package_access.locations,
                &package_access.boundary,
                &package_access.local_packages,
            ) else {
                return Ok(Some(super::unavailable(query)));
            };
            Ok(Some(definition_reply(
                client_root.address(&path)?,
                lsp_types::Range::default(),
            )))
        }
        SourceQuery::Hover(_) => Ok(Some(import_path_hover(
            source,
            &import.node,
            format!("Declared local module source `{written}`."),
        ))),
        _ => Ok(Some(super::unavailable(query))),
    }
}

/// The answer a path naming a builtin package source gets; `None` when it names none.
fn builtin_answer(
    config: &ResolvedSiteConfig,
    package_access: &PackageAccess<'_>,
    source: &Source,
    import: &ImportPath<'_>,
    query: &SourceQuery,
    client_root: &crate::uri::ClientRoot,
) -> Result<Option<SourceReply>> {
    let Some(literal) = import.node.cast::<ast::Str>() else {
        return Ok(None);
    };
    let written = literal.get();
    let Some((package, id, description)) = imported_source(source.id(), &written)? else {
        if matches!(query, SourceQuery::Hover(_))
            && let Ok(spec) = written.parse::<PackageSpec>()
        {
            return Ok(Some(import_path_hover(
                source,
                &import.node,
                spec_hover(package_access, &spec),
            )));
        }
        // Immutable package navigation must never fall through to disk/package
        // lookup for a path absent from the builtin registry.
        return Ok(crate::identity::embedded_source(source.id())
            .is_some()
            .then(|| super::unavailable(query)));
    };
    match query {
        SourceQuery::Hover(_) => Ok(Some(import_path_hover(
            source,
            &import.node,
            package_docs(&package, &written, description.as_deref()),
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
                definition_uri(id, config.get_root(), client_root)?,
                range,
            )))
        }
        _ => Ok(Some(super::unavailable(query))),
    }
}

/// The import-spec completions one site offers for the `@` prefix the author writes.
fn spec_completions(
    package_access: &PackageAccess<'_>,
    source: &Source,
    path: std::ops::Range<usize>,
    prefix: &str,
    unterminated: bool,
) -> Result<Vec<CompletionItem>> {
    let range =
        position::utf16_range(source.lines(), path).context("invalid package completion range")?;
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
        let new_text = with_closing_quote(name.clone(), unterminated);
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
    Ok(items)
}

/// One definition reply: the file the import names and the range the client selects in it.
fn definition_reply(uri: Uri, range: lsp_types::Range) -> SourceReply {
    SourceReply::Definition(Some(GotoDefinitionResponse::Scalar(Location {
        uri,
        range,
    })))
}

fn import_path_hover(source: &Source, node: &LinkedNode<'_>, text: String) -> SourceReply {
    SourceReply::Hover(Some(crate::protocol::markdown_hover(
        text,
        position::utf16_range(source.lines(), node.range()),
    )))
}

fn local_imported_source(
    from: FileId,
    written: &str,
    access: &PackageAccess<'_>,
) -> Option<FileId> {
    if written.starts_with('@') {
        let spec = written.parse::<PackageSpec>().ok()?;
        return access
            .local_packages
            .package(&spec)
            .map(tola_typst::LocalPackage::entry_id);
    }
    let VirtualRoot::Package(spec) = from.root() else {
        return None;
    };
    access.local_packages.package(spec)?;
    tola_typst::typst::foundations::PathOrStr::Str(written.into())
        .resolve(from)
        .ok()
        .map(|path| path.intern())
}

/// The mirror file an editor reads one builtin package source at, when it holds that source.
fn verified_package_view(id: FileId, directory: &Path) -> Option<PathBuf> {
    let source = crate::identity::embedded_source(id)?;
    let path = crate::identity::package_view_path(id, directory)?;
    crate::identity::verify_package_view(&path, &source)
        .ok()
        .map(|()| path)
}

/// The client's address for one definition target; a package with no verified mirror keeps its URI.
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
    package_access: &PackageAccess<'_>,
    source: &Source,
    prefix: &str,
    version_start: usize,
    path_end: usize,
    unterminated: bool,
) -> Vec<CompletionItem> {
    let Some((written, typed)) = prefix.split_once(':') else {
        return Vec::new();
    };
    let typed = typed.trim_start();
    let Some((namespace, name)) = namespace_and_name(written) else {
        return Vec::new();
    };
    let mut versions: Vec<PackageVersion> = installed_versions(package_access, namespace, name);
    versions
        .extend(published_versions(package_access.published, namespace, name).unwrap_or_default());
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
    versions.sort();
    versions.dedup();
    let Some(range) = position::utf16_range(source.lines(), version_start..path_end) else {
        return Vec::new();
    };
    versions
        .into_iter()
        .filter(|version| version.to_string().starts_with(typed))
        .map(|version| {
            let version = version.to_string();
            let new_text = with_closing_quote(version.clone(), unterminated);
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

fn namespace_and_name(written: &str) -> Option<(&str, &str)> {
    let (namespace, name) = written.strip_prefix('@')?.split_once('/')?;
    (!namespace.is_empty() && !name.is_empty()).then_some((namespace, name))
}

/// Only a `@tola/...` path can be a misspelling of a builtin: another namespace names a package
/// Tola does not publish, and a path that already resolves needs no fix.
pub(crate) fn replacement(source: &Source, cursor: usize) -> Option<TextEdit> {
    let ImportPath { path, .. } = import_path(source, cursor)?;
    let spec = source.text()[path.clone()].parse::<PackageSpec>().ok()?;
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
    let packages: Vec<BuiltinPackage> = builtin_packages().collect();
    let names: Vec<String> = packages
        .iter()
        .map(|package| package.spec().name.to_string())
        .collect();
    let closest = tola_typst::spelling::closest(name, names.iter().map(String::as_str))?.to_owned();
    packages
        .into_iter()
        .find(|package| package.spec().name.as_str() == closest)
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
            if let Some(entry) =
                newest_published_entry(package_access.published, spec.name.as_str())
            {
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

/// The newest entry one `@preview` package name has in the published index.
fn newest_published_entry<'a>(
    published: &'a [PublishedPackage],
    name: &str,
) -> Option<&'a PublishedPackage> {
    published
        .iter()
        .filter(|package| package.name == name)
        .filter_map(|package| {
            let version = package.version.parse::<PackageVersion>().ok()?;
            Some((version, package))
        })
        .max_by_key(|(version, _)| *version)
        .map(|(_, package)| package)
}

/// The published versions of one `@preview` package, oldest first.
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

/// The reader path argument the cursor writes; an unfinished string's edit ends on its line.
fn reader_path(
    source: &Source,
    cursor: usize,
    indexed_names: Option<&SourceNames>,
) -> Option<WrittenPath> {
    let text = source.text();
    let argument = tola_typst_syntax::syntax::path_argument(source, cursor)?;
    if argument.base != tola_typst_syntax::syntax::PathBase::File {
        return None;
    }
    if let Some(callee) = &argument.callee {
        let local_names;
        let names = match indexed_names {
            Some(names) => names,
            None => {
                local_names = SourceNames::new(source.clone());
                &local_names
            }
        };
        if names.declared_at(callee.start).is_some() {
            return None;
        }
    }
    let line_end = string_line_end(text, argument.range.start, argument.range.end);
    (cursor <= line_end).then_some(WrittenPath {
        range: argument.range.start..line_end,
        unterminated: text.as_bytes().get(line_end) != Some(&b'"'),
        extension: argument.extension,
    })
}

/// The site files and directories one path argument names, labelled from the root or the writer.
fn site_path_completion(
    config: &ResolvedSiteConfig,
    source: &Source,
    cursor: usize,
    written: &WrittenPath,
    cancellation: &BuildCancellation,
    overrides: &[(PathBuf, Arc<str>)],
    boundary: &tola_typst::SourceBoundary,
) -> Result<Option<Vec<CompletionItem>>> {
    if source.id().root() != &VirtualRoot::Project {
        return Ok(None);
    }
    let raw_prefix = &source.text()[written.range.start..cursor];
    let prefix = tola_typst_syntax::syntax::string_prefix(raw_prefix);
    let rooted = prefix.starts_with('/');
    let Some(directory) = source.id().vpath().parent() else {
        return Ok(Some(Vec::new()));
    };
    let mut directories = std::collections::BTreeSet::new();
    let mut files = Vec::new();
    for id in crate::files::completable_file_ids(config, overrides, boundary, cancellation)? {
        cancellation.ensure_active()?;
        if id == source.id()
            || written
                .extension
                .is_some_and(|extension| id.vpath().extension() != Some(extension))
        {
            continue;
        }
        let label = if rooted {
            id.vpath().get_with_slash().to_owned()
        } else {
            id.vpath().relative_from(&directory).to_string()
        };
        for (index, _) in label.match_indices('/') {
            let directory = &label[..=index];
            if directory != "/" && directory != prefix.as_str() {
                directories.insert(directory.to_owned());
            }
        }
        files.push(label);
    }
    files.sort();
    let mut replacement =
        super::completion::completion_edit(source, cursor, written.range.clone(), String::new())?;
    if written.unterminated
        && let CompletionTextEdit::InsertAndReplace(edit) = &replacement
    {
        replacement = CompletionTextEdit::Edit(TextEdit {
            range: edit.replace,
            new_text: String::new(),
        });
    }
    let mut items = Vec::new();
    let labels = directories
        .into_iter()
        .map(|label| (label, CompletionItemKind::FOLDER))
        .chain(
            files
                .into_iter()
                .map(|label| (label, CompletionItemKind::FILE)),
        );
    for (label, kind) in labels {
        let quoted = label.as_str().repr();
        let content = &quoted[1..quoted.len() - 1];
        let new_text = if written.unterminated {
            quoted[1..].to_owned()
        } else {
            content.to_owned()
        };
        let filter = match label.strip_prefix(prefix.as_str()) {
            Some(suffix) => {
                let suffix = suffix.repr();
                format!("{raw_prefix}{}", &suffix[1..suffix.len() - 1])
            }
            None => content.to_owned(),
        };
        let filter_text = (filter != label).then_some(filter);
        let mut edit = replacement.clone();
        crate::completion::set_edit_text(&mut edit, new_text);
        items.push(CompletionItem {
            label,
            kind: Some(kind),
            filter_text,
            text_edit: Some(edit),
            ..CompletionItem::default()
        });
    }
    crate::completion::preserve_order(&mut items);
    Ok(Some(items))
}
fn imported_source(
    id: FileId,
    from: &str,
) -> Result<Option<(BuiltinPackage, FileId, Option<String>)>> {
    if from.starts_with('@') {
        let Ok(spec) = from.parse::<PackageSpec>() else {
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

/// Rewrite each package URI to the verified mirror file; a missing mirror keeps the URI.
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
    let id = crate::identity::file_id(uri, client_root.resolved())?;
    if !matches!(id.root(), VirtualRoot::Package(_)) {
        return Ok(());
    }
    let mirror = verified_package_view(id, directory);
    cancellation.ensure_active()?;
    // A missing or stale mirror keeps the package URI; `tola editor doctor` reports it.
    if let Some(path) = mirror {
        *uri = client_root.address(&path)?;
    }
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

    /// The configuration of a site whose `tola.toml` is empty.
    fn site_config(root: &Path) -> Arc<ResolvedSiteConfig> {
        std::fs::write(root.join("tola.toml"), "").unwrap();
        load_configuration(root).expect("site configuration")
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
        let package_access = PackageAccess::new(
            config,
            Arc::new(tola_typst::LocalPackages::new([], std::iter::empty::<String>()).unwrap()),
            &tola_build::BuildResources::new(),
            published,
        );
        match respond(PackageQuery {
            config,
            package_access: &package_access,
            source: &source,
            cursor,
            query: &query,
            client_root: &crate::uri::ClientRoot::new(config.get_root()),
            cancellation: &BuildCancellation::new(),
            overrides: &[],
            indexed_names: None,
        })
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

    #[test]
    fn definition_answers_client_spelling() {
        let directory = tempfile::tempdir().unwrap();
        let resolved = directory.path().canonicalize().unwrap();
        let spelled = directory.path().join("aliased");
        let config = site_config(&resolved);
        let spec: PackageSpec = "@tola/site:0.0.0".parse().unwrap();
        let package = builtin_package(&spec).expect("a builtin package");
        let id = entrypoint(&package, &manifest(&package).expect("a manifest")).expect("an entry");
        let embedded = crate::identity::embedded_source(id).expect("a builtin package source");
        let view = tola_input::layout::builtin_packages(&resolved);
        let mirror = crate::identity::package_view_path(id, &view).unwrap();
        std::fs::create_dir_all(mirror.parent().unwrap()).unwrap();
        std::fs::write(&mirror, embedded.as_bytes()).unwrap();
        let client_root = crate::uri::ClientRoot::with_resolved(&spelled, &resolved);
        let expected =
            crate::uri::from_file_path(&spelled.join(mirror.strip_prefix(&resolved).unwrap()))
                .unwrap();

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
        let package_access = PackageAccess::new(
            &config,
            Arc::new(tola_typst::LocalPackages::new([], std::iter::empty::<String>()).unwrap()),
            &tola_build::BuildResources::new(),
            &[],
        );
        let reply = respond(PackageQuery {
            config: &config,
            package_access: &package_access,
            source: &source,
            cursor,
            query: &query,
            client_root: &client_root,
            cancellation: &BuildCancellation::new(),
            overrides: &[],
            indexed_names: None,
        })
        .expect("a reply");
        let Some(SourceReply::Definition(Some(GotoDefinitionResponse::Scalar(location)))) = reply
        else {
            panic!("a definition: {reply:?}");
        };
        assert_eq!(location.uri, expected);

        // A mirror file the client already reads as a file keeps the client's spelling too.
        let mut mirrored = GotoDefinitionResponse::Scalar(Location {
            uri: crate::uri::from_file_path(&mirror).unwrap(),
            range: Range::default(),
        });
        package_source_definition(
            &mut mirrored,
            &view,
            &client_root,
            &BuildCancellation::new(),
        )
        .unwrap();
        assert_eq!(
            mirrored,
            GotoDefinitionResponse::Scalar(Location {
                uri: expected,
                range: Range::default(),
            })
        );
    }

    #[test]
    fn unsaved_paths_complete_once() {
        let mut site = QuerySession::new();
        let root = site.config.get_root().to_path_buf();
        let document = root.join("content/document.typ");
        let draft = root.join("content/draft.typ");
        site.site.write("content/existing.typ", "Existing\n");
        let (text, position) = marked_cursor("#include \"dr|\"\n");
        let query = SourceQuery::decode(lsp_server::Request {
            id: 1.into(),
            method: lsp_request::Completion::METHOD.into(),
            params: json!({
                "textDocument": { "uri": crate::uri::from_file_path(&document).unwrap() },
                "position": position,
            }),
        })
        .unwrap()
        .1;
        let overrides = [
            (document, Arc::from(text)),
            (draft.clone(), Arc::from("Draft\n")),
            (root.join("content/existing.typ"), Arc::from("Unsaved\n")),
            (root.join("public/hidden.typ"), Arc::from("Generated\n")),
        ];
        let reply = crate::query::respond(crate::query::QueryRequest {
            session: &mut site.session,
            compilations: &mut site.compilations,
            disk: &mut crate::analysis::DiskSources::default(),
            graphs: &mut crate::analysis::GraphCache::default(),
            bibliographies: &mut site.bibliographies,
            entity_queries: &mut crate::compiler::EntityQueries::default(),
            config: &site.config,
            source_revision: site.revision,
            overrides: &overrides,
            view: &crate::sources::SourceView::default(),
            published: &[],
            query: &query,
            routes_as_hints: false,
            check_progress: crate::protocol::CheckProgress::Failed,
            cancellation: &BuildCancellation::new(),
            client_root: &crate::uri::ClientRoot::new(&root),
        })
        .unwrap();
        let SourceReply::Completion(CompletionResponse::Array(items)) = reply else {
            panic!("a path completion reply");
        };
        let labels = completion_labels(&items);
        assert!(labels.contains(&"draft.typ"), "{labels:?}");
        assert_eq!(
            labels
                .iter()
                .filter(|label| **label == "existing.typ")
                .count(),
            1
        );
        assert!(!labels.iter().any(|label| label.ends_with("hidden.typ")));
        assert!(!draft.exists());
    }

    #[test]
    fn local_imports_land_at_start() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let config = site_config(&root);
        let package_root = root.join("module");
        std::fs::create_dir(&package_root).unwrap();
        let entry = package_root.join("lib.typ");
        std::fs::write(&entry, "#let value = 1\n#value\n").unwrap();
        let spec = "@local/demo:0.1.0".parse().unwrap();
        let package = tola_typst::LocalPackage::new(
            spec,
            &package_root,
            VirtualPath::new("lib.typ").unwrap(),
        )
        .unwrap();
        let mut access = PackageAccess::new(
            &config,
            Arc::new(tola_typst::LocalPackages::new([], std::iter::empty::<String>()).unwrap()),
            &tola_build::BuildResources::new(),
            &[],
        );
        access.local_packages =
            Arc::new(tola_typst::LocalPackages::new([package], ["local"]).unwrap());
        let (source, cursor) = marked_source("#import \"@local/de|mo:0.1.0\"\n");
        let query = SourceQuery::decode(lsp_server::Request::new(
            1.into(),
            lsp_request::GotoDefinition::METHOD.into(),
            json!({
                "textDocument": {"uri": crate::uri::from_file_path(&root.join("content/document.typ")).unwrap()},
                "position": position::utf16_range(source.lines(), cursor..cursor).unwrap().start,
            }),
        )).unwrap().1;
        let reply = respond(PackageQuery {
            config: &config,
            package_access: &access,
            source: &source,
            cursor,
            query: &query,
            client_root: &crate::uri::ClientRoot::new(&root),
            cancellation: &BuildCancellation::new(),
            overrides: &[],
            indexed_names: None,
        })
        .unwrap();
        let Some(SourceReply::Definition(Some(GotoDefinitionResponse::Scalar(location)))) = reply
        else {
            panic!("a local import names its source file");
        };
        assert_eq!(location.uri, crate::uri::from_file_path(&entry).unwrap());
        assert_eq!(location.range, Range::default());
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
        let index = [published("query-local", "0.2.0")];
        let marked = "#import \"@preview/query-local:|\"\n";
        assert_eq!(import_completions(&index, marked), ["0.2.0"]);
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
    fn published_names_complete_once() {
        let index = [
            published("cetz", "0.3.3"),
            published("cetz", "0.3.4"),
            published("cetz-other", "1.0.0"),
        ];
        assert_eq!(
            import_completions(&index, "#import \"@preview/ce|\"\n"),
            ["@preview/cetz-other:1.0.0", "@preview/cetz:0.3.4"],
            "a name position offers one entry per package, at its newest version"
        );
        assert_eq!(
            import_completions(&index, "#import \"@preview/cetz:|\"\n"),
            ["0.3.3", "0.3.4"],
            "a version position still offers every version"
        );
    }

    #[test]
    fn version_completions_order_by_number() {
        let index = [published("cetz", "0.10.0"), published("cetz", "0.9.0")];
        assert_eq!(
            import_completions(&index, "#import \"@preview/cetz:|\"\n"),
            ["0.9.0", "0.10.0"]
        );
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
    fn file_clients_read_package_sources() {
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
        let original = Location { uri, range };
        for mut definition in [
            GotoDefinitionResponse::Scalar(original.clone()),
            GotoDefinitionResponse::Array(vec![original.clone()]),
        ] {
            package_source_definition(
                &mut definition,
                &sources,
                &client_root,
                &BuildCancellation::new(),
            )
            .unwrap();
            let location = match &definition {
                GotoDefinitionResponse::Scalar(location) => location,
                GotoDefinitionResponse::Array(locations) => &locations[0],
                _ => unreachable!(),
            };
            assert_eq!(location.uri, crate::uri::from_file_path(&path).unwrap());
            assert_eq!(location.range, range);
        }

        std::fs::write(&path, "modified source").unwrap();
        let mut projected = GotoDefinitionResponse::Scalar(original.clone());
        package_source_definition(
            &mut projected,
            &sources,
            &client_root,
            &BuildCancellation::new(),
        )
        .unwrap();
        assert_eq!(projected, GotoDefinitionResponse::Scalar(original));
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
