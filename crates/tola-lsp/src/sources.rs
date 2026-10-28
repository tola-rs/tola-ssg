//! Immutable parsed editor sources and atomic UTF-16 document changes.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result, bail};
use lsp_server::Notification;
use lsp_types::TextDocumentContentChangeEvent;
use lsp_types::Uri;
use lsp_types::notification::{self, Notification as LspNotification};
use tola_typst::typst::syntax::{FileId, Lines, RootedPath, Source, VirtualPath, VirtualRoot};
use tola_typst::{SourceBoundary, SourceRefusal};

use crate::compiler::SourceOverrides;
use crate::position;

/// Load a named Typst source, with editor text taking precedence over disk.
/// Embedded package sources are immutable; non-Typst files belong to their own services.
pub(super) fn load(
    uri: &Uri,
    root: &Path,
    unsaved: &SourceOverrides,
    boundary: &SourceBoundary,
    package_sources: Option<&Path>,
    parsed: Option<&Source>,
) -> Option<Source> {
    let root = tola_build::filesystem::normalize_existing_prefix(root);
    let id = crate::identity::source_id(uri, &root, package_sources, boundary).ok()??;
    if let Some(text) = crate::identity::embedded_source(id) {
        return Some(Source::new(id, text.into_owned()));
    }
    let path = crate::uri::to_site_path(uri.as_str()).ok()?;
    if !is_typst_source(&path) {
        return None;
    }
    if let Some(source) = parsed.filter(|source| source.id() == id) {
        return Some(source.clone());
    }
    let text = match unsaved.iter().find(|(open, _)| open == &path) {
        Some((_, text)) => text.to_string(),
        None => std::fs::read_to_string(&path).ok()?,
    };
    Some(Source::new(id, text))
}

#[derive(Clone)]
pub(super) struct ParsedSource {
    /// The revision at which this document's text or identity last changed. A reply in flight is
    /// stale only when its own document's revision moves, so an unrelated file change leaves it.
    pub(super) revision: u64,
    pub(super) source: Source,
    names: Arc<OnceLock<Arc<tola_typst_syntax::names::SourceNames>>>,
}

impl ParsedSource {
    pub(super) fn names(&self) -> Arc<tola_typst_syntax::names::SourceNames> {
        Arc::clone(self.names.get_or_init(|| {
            Arc::new(tola_typst_syntax::names::SourceNames::new(
                self.source.clone(),
            ))
        }))
    }
}

#[derive(Clone)]
struct OpenSource {
    path: Option<PathBuf>,
    version: i32,
    text: Arc<str>,
    parsed: Option<ParsedSource>,
}

impl OpenSource {
    fn changed_text(
        &self,
        version: i32,
        changes: Vec<TextDocumentContentChangeEvent>,
    ) -> Result<Option<Lines<String>>> {
        if version <= self.version {
            tracing::debug!(
                version,
                current = self.version,
                "a document change older than the applied version was dropped"
            );
            return Ok(None);
        }
        let mut text = Lines::new(self.text.to_string());
        for change in changes {
            let range = if let Some(range) = change.range {
                let start = position::byte_offset(&text, range.start)?;
                let end = position::byte_offset(&text, range.end)?;
                if start > end {
                    bail!("document change range ends before it starts");
                }
                start..end
            } else {
                0..text.len_bytes()
            };
            text.edit(range, &change.text);
        }
        Ok(Some(text))
    }

    fn replace_text(&mut self, version: i32, text: Lines<String>, revision: u64) {
        // No text, version, or analysis cache changes until the entire batch is valid.
        if let Some(parsed) = &mut self.parsed {
            parsed.source.replace(text.text());
            parsed.revision = revision;
            parsed.names = Arc::default();
        }
        self.version = version;
        self.text = Arc::from(text.text());
    }

    fn refresh_path(&mut self, uri: &str, root: &Path, revision: u64) {
        let Ok(path) = crate::uri::to_site_path(uri) else {
            return;
        };
        let id = is_typst_source(&path)
            .then(|| crate::identity::path_id(&path, root))
            .flatten();
        if self.path.as_ref() == Some(&path)
            && self.parsed.as_ref().map(|parsed| parsed.source.id()) == id
        {
            return;
        }
        self.parsed = id.map(|id| ParsedSource {
            revision,
            source: Source::new(id, self.text.to_string()),
            names: Arc::default(),
        });
        self.path = Some(path);
    }
}

#[derive(Clone, Default)]
pub(super) struct SourceView {
    package_sources: Option<Arc<PathBuf>>,
    sources: Arc<BTreeMap<String, OpenSource>>,
    overrides: SourceOverrides,
}

impl SourceView {
    pub(crate) fn package_sources(&self) -> Option<&Path> {
        self.package_sources.as_deref().map(PathBuf::as_path)
    }

    /// Package mirrors are readable as embedded sources, never as editable site sources.
    pub(crate) fn site_boundary<'a>(
        &self,
        boundary: &'a SourceBoundary,
    ) -> Cow<'a, SourceBoundary> {
        match self.package_sources() {
            Some(directory) => Cow::Owned(boundary.clone().excluding(directory.to_path_buf())),
            None => Cow::Borrowed(boundary),
        }
    }

    pub(super) fn get(&self, uri: &Uri) -> Option<&ParsedSource> {
        self.sources.get(uri.as_str())?.parsed.as_ref()
    }

    pub(super) fn by_id(&self, id: FileId) -> Option<&ParsedSource> {
        self.sources.values().find_map(|open| {
            let parsed = open.parsed.as_ref()?;
            (parsed.source.id() == id).then_some(parsed)
        })
    }

    pub(crate) fn named_source(&self, path: &Path) -> Option<(&str, &str)> {
        self.sources.iter().find_map(|(uri, open)| {
            (open.path.as_deref() == Some(path)).then_some((uri.as_str(), open.text.as_ref()))
        })
    }

    pub(crate) fn overrides(&self) -> &SourceOverrides {
        &self.overrides
    }

    pub(crate) fn versions_for_path<'a>(
        &'a self,
        path: &'a Path,
    ) -> impl Iterator<Item = (&'a str, i32)> {
        self.sources
            .iter()
            .filter(move |(_, source)| source.path.as_deref() == Some(path))
            .map(|(uri, source)| (uri.as_str(), source.version))
    }

    /// Parsed site documents; unnamed buffers and package documents are excluded.
    pub(super) fn iter(&self) -> impl Iterator<Item = &ParsedSource> {
        self.sources
            .values()
            .filter(|open| open.path.is_some())
            .filter_map(|open| open.parsed.as_ref())
    }

    pub(super) fn is_unnamed(&self, id: FileId) -> bool {
        self.sources.values().any(|open| {
            open.path.is_none()
                && open
                    .parsed
                    .as_ref()
                    .is_some_and(|parsed| parsed.source.id() == id)
        })
    }
}

#[derive(Default)]
pub(super) struct OpenSources {
    root: PathBuf,
    /// Tola's own generated state: its state directory and the site's build lock.
    generated: SourceBoundary,
    /// The generated state one resolved configuration adds: the output tree, and the publication
    /// and vendor workspaces.
    resolved_generated: Option<SourceBoundary>,
    revision: u64,
    view: SourceView,
}

impl OpenSources {
    pub(super) fn new(root: &Path) -> Self {
        Self {
            root: tola_build::filesystem::normalize_existing_prefix(root),
            // Containment is a read's business: a file change is refused for generated state
            // alone, so this boundary never has to require the site root.
            generated: generated_state_boundary(root),
            ..Self::default()
        }
    }

    pub(super) fn with_package_sources(mut self, directory: Option<Arc<PathBuf>>) -> Self {
        self.view.package_sources = directory;
        self.generated = self.view.site_boundary(&self.generated).into_owned();
        self
    }

    /// Add the generated state one resolved configuration refuses, which a file change must not
    /// turn into a revision either.
    ///
    /// The generated state this connection always refuses stands: [`base_source_boundary`]'s read
    /// exception for the package view is a read's business, so a rewritten mirror stays Tola's own
    /// writing here.
    pub(super) fn exclude_generated(&mut self, generated: SourceBoundary) {
        self.resolved_generated = Some(generated);
    }

    /// A resolved configuration can select a root before which only the workspace was known.
    pub(super) fn rebase(&mut self, root: &Path) {
        let root = tola_build::filesystem::normalize_existing_prefix(root);
        if self.root == root {
            return;
        }
        self.generated = self
            .view
            .site_boundary(&generated_state_boundary(&root))
            .into_owned();
        self.root = root;
        self.refresh_paths(self.revision);
        self.view.overrides = self.open_source_texts().into();
    }

    /// Whether one changed document path is state Tola generates or publishes.
    ///
    /// The boundaries resolve the path's own spelling, so a caller may pass it as it read it.
    pub(super) fn is_generated_path(&self, path: &Path) -> bool {
        std::iter::once(&self.generated)
            .chain(self.resolved_generated.iter())
            .any(|boundary| {
                matches!(
                    boundary.refusal(path),
                    Ok(Some(SourceRefusal::GeneratedState))
                )
            })
    }

    /// Whether one changed editor URI resolves into state Tola generates or publishes.
    ///
    /// Only a path a boundary positively refuses counts: an unnamed document, a path outside
    /// the site, and a file that no longer exists all stay eligible, because the site's declared
    /// inputs may live anywhere and a deletion still changes what a check reads.
    pub(super) fn is_generated_state(&self, uri: &str) -> bool {
        crate::uri::to_file_path(uri).is_ok_and(|path| self.is_generated_path(&path))
    }

    pub(super) fn view(&self) -> SourceView {
        self.view.clone()
    }

    pub(super) fn overrides(&self) -> &SourceOverrides {
        &self.view.overrides
    }

    pub(super) fn source(&self, uri: &Uri) -> Option<&Source> {
        Some(&self.view.get(uri)?.source)
    }

    pub(super) fn versions_for_path<'a>(
        &'a self,
        path: &'a Path,
    ) -> impl Iterator<Item = (&'a str, i32)> {
        self.view.versions_for_path(path)
    }

    pub(super) fn version(&self, uri: &str) -> Option<i32> {
        self.view.sources.get(uri).map(|source| source.version)
    }

    /// The source revision these documents and their unsaved text belong to.
    ///
    /// Every accepted change to a tracked document, and every file-change notification, starts a
    /// new revision: a compilation, or a name graph, is reused only within one.
    pub(super) fn revision(&self) -> u64 {
        self.revision
    }

    pub(super) fn apply(&mut self, message: Notification) -> Result<bool> {
        let revision = self.revision + 1;
        let document = message
            .params
            .get("textDocument")
            .and_then(|document| document.get("uri"))
            .and_then(serde_json::Value::as_str);
        let unnamed = document.is_some_and(|uri| uri.starts_with("untitled:"));
        // A document that is generated state is not a source: tracking its buffer would cost a
        // revision and a whole-site check that cannot differ, because no read reaches it.
        if document.is_some_and(|uri| self.is_generated_state(uri)) {
            return Ok(false);
        }
        let changed = match message.method.as_str() {
            notification::DidOpenTextDocument::METHOD => self.open_document(message, revision)?,
            notification::DidChangeTextDocument::METHOD => {
                self.change_document(message, revision)?
            }
            notification::DidCloseTextDocument::METHOD => self.close_document(message)?,
            notification::DidSaveTextDocument::METHOD => {
                if unnamed {
                    return Ok(false);
                }
                self.save_document(message, revision)?
            }
            notification::DidChangeWatchedFiles::METHOD => {
                let params = extract::<notification::DidChangeWatchedFiles>(message)?;
                self.refresh_unless_generated(
                    params.changes.iter().map(|change| change.uri.as_str()),
                    revision,
                )
            }
            notification::DidCreateFiles::METHOD => {
                let params = extract::<notification::DidCreateFiles>(message)?;
                self.refresh_unless_generated(
                    params.files.iter().map(|file| file.uri.as_str()),
                    revision,
                )
            }
            notification::DidRenameFiles::METHOD => {
                let params = extract::<notification::DidRenameFiles>(message)?;
                let moved = params
                    .files
                    .iter()
                    .flat_map(|file| [file.old_uri.as_str(), file.new_uri.as_str()]);
                self.refresh_unless_generated(moved, revision)
            }
            notification::DidDeleteFiles::METHOD => {
                let params = extract::<notification::DidDeleteFiles>(message)?;
                self.refresh_unless_generated(
                    params.files.iter().map(|file| file.uri.as_str()),
                    revision,
                )
            }
            _ => return Ok(false),
        };
        if !changed {
            return Ok(false);
        }
        self.revision = revision;
        if !unnamed {
            self.view.overrides = self.open_source_texts().into();
        }
        Ok(true)
    }

    fn open_document(&mut self, message: Notification, revision: u64) -> Result<bool> {
        let params = extract::<notification::DidOpenTextDocument>(message)?;
        let document = params.text_document;
        let Some(path) = document_path(document.uri.as_str()) else {
            return Ok(false);
        };
        let parsed = match &path {
            DocumentPath::Untitled => Some(Source::new(
                FileId::unique(RootedPath::new(
                    VirtualRoot::Project,
                    VirtualPath::new("untitled.typ").expect("valid virtual path"),
                )),
                document.text.clone(),
            )),
            DocumentPath::Site(site) if is_typst_source(site) => {
                crate::identity::file_id(&document.uri, &self.root)
                    .ok()
                    .map(|id| Source::new(id, document.text.clone()))
            }
            DocumentPath::Site(_) => None,
        }
        .map(|source| ParsedSource {
            revision,
            source,
            names: Arc::default(),
        });
        Arc::make_mut(&mut self.view.sources).insert(
            document.uri.as_str().to_owned(),
            OpenSource {
                path: path.site_path(),
                version: document.version,
                text: document.text.into(),
                parsed,
            },
        );
        Ok(true)
    }

    fn change_document(&mut self, message: Notification, revision: u64) -> Result<bool> {
        let params = extract::<notification::DidChangeTextDocument>(message)?;
        let document = params.text_document;
        if document_path(document.uri.as_str()).is_none() {
            return Ok(false);
        }
        let open = self
            .view
            .sources
            .get(document.uri.as_str())
            .with_context(|| {
                format!(
                    "received changes for unopened document `{}`",
                    document.uri.as_str()
                )
            })?;
        let Some(text) = open.changed_text(document.version, params.content_changes)? else {
            return Ok(false);
        };
        Arc::make_mut(&mut self.view.sources)
            .get_mut(document.uri.as_str())
            .expect("the validated open document remains tracked")
            .replace_text(document.version, text, revision);
        Ok(true)
    }

    fn close_document(&mut self, message: Notification) -> Result<bool> {
        let params = extract::<notification::DidCloseTextDocument>(message)?;
        Ok(Arc::make_mut(&mut self.view.sources)
            .remove(params.text_document.uri.as_str())
            .is_some())
    }

    fn save_document(&mut self, message: Notification, revision: u64) -> Result<bool> {
        let params = extract::<notification::DidSaveTextDocument>(message)?;
        let uri = params.text_document.uri;
        if let Some(open) = Arc::make_mut(&mut self.view.sources).get_mut(uri.as_str()) {
            open.refresh_path(uri.as_str(), &self.root, revision);
        }
        // A save costs a revision even when the editor no longer tracks the document: the client
        // wrote those bytes to disk.
        Ok(true)
    }

    fn refresh_paths(&mut self, revision: u64) {
        for (uri, open) in Arc::make_mut(&mut self.view.sources) {
            open.refresh_path(uri, &self.root, revision);
        }
    }

    /// Re-resolve every tracked document's path after a file-level change, unless every path it
    /// names is generated state, and report whether the change costs a revision.
    ///
    /// A change whose every path is generated state costs none: starting a revision for it would
    /// run a whole-site check that cannot differ, because no read reaches those paths.
    fn refresh_unless_generated<'a>(
        &mut self,
        changed: impl IntoIterator<Item = &'a str>,
        revision: u64,
    ) -> bool {
        if changed.into_iter().all(|uri| self.is_generated_state(uri)) {
            return false;
        }
        self.refresh_paths(revision);
        true
    }

    fn open_source_texts(&self) -> Vec<(PathBuf, Arc<str>)> {
        self.view
            .sources
            .values()
            .filter_map(|source| Some((source.path.clone()?, Arc::clone(&source.text))))
            .collect()
    }
}

/// The source boundary a connection starts from: Tola's own disposable directory and the site's
/// build lock are generated state, and `contained` requires a source to resolve inside `root`.
///
/// The package view `tola editor setup` publishes below that directory is the exception: it is a
/// spelling of this site's packages rather than a place of its own, and a definition into a package
/// names those files, so a file-only client reads them as sources.
pub(super) fn base_source_boundary(root: &Path, contained: bool) -> SourceBoundary {
    SourceBoundary::new(root, contained)
        .excluding_except(
            root.join(tola_build::filesystem::INTERNAL_DIR),
            tola_build::filesystem::layout::builtin_packages(root),
        )
        .excluding(root.join(tola_build::filesystem::SITE_BUILD_LOCK_FILE))
}

/// The generated state a file change never turns into a revision: everything below Tola's state
/// directory and the site's build lock, which no read exception covers.
///
/// A read still reaches the package view through [`base_source_boundary`]; a rewrite there is
/// Tola's own writing rather than an author edit.
fn generated_state_boundary(root: &Path) -> SourceBoundary {
    SourceBoundary::new(root, false)
        .excluding(root.join(tola_build::filesystem::INTERNAL_DIR))
        .excluding(root.join(tola_build::filesystem::SITE_BUILD_LOCK_FILE))
}

fn extract<N: LspNotification>(message: Notification) -> Result<N::Params> {
    message.extract(N::METHOD).map_err(Into::into)
}

/// Whether a path names a Typst source, the only file kind this connection parses.
fn is_typst_source(path: &Path) -> bool {
    path.extension() == Some(OsStr::new("typ"))
}

/// Where the editor's document URI resolves: a site path, or a buffer the editor has not named.
enum DocumentPath {
    Untitled,
    Site(PathBuf),
}

impl DocumentPath {
    /// The site path the document answers for, or none for an untitled buffer.
    fn site_path(self) -> Option<PathBuf> {
        match self {
            Self::Untitled => None,
            Self::Site(path) => Some(path),
        }
    }
}

fn document_path(uri: &str) -> Option<DocumentPath> {
    if uri.starts_with("untitled:") {
        return Some(DocumentPath::Untitled);
    }
    Some(DocumentPath::Site(crate::uri::to_site_path(uri).ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{
        DidChangeTextDocumentParams, DidOpenTextDocumentParams, Range,
        TextDocumentContentChangeEvent, TextDocumentItem, Uri, VersionedTextDocumentIdentifier,
    };

    fn opened(uri: Uri, text: &str) -> Notification {
        Notification::new(
            notification::DidOpenTextDocument::METHOD.into(),
            DidOpenTextDocumentParams {
                text_document: TextDocumentItem::new(uri, "typst".into(), 1, text.into()),
            },
        )
    }

    fn changed(
        uri: Uri,
        version: i32,
        changes: Vec<TextDocumentContentChangeEvent>,
    ) -> Notification {
        Notification::new(
            notification::DidChangeTextDocument::METHOD.into(),
            DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier::new(uri, version),
                content_changes: changes,
            },
        )
    }

    fn edit(range: Option<Range>, text: &str) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range,
            range_length: None,
            text: text.into(),
        }
    }

    fn span(text: &str, marker: &str) -> Range {
        let start = text.find(marker).unwrap();
        position::utf16_range(&Lines::new(text), start..start + marker.len()).unwrap()
    }

    /// A package's own file answers as the package document the compiler resolved, wherever the
    /// editor reads it from.
    #[test]
    fn package_files_load_package_document() {
        let site = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let package = cache.path().join("preview/demo/1.0.0");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(
            package.join("typst.toml"),
            "[package]\nname = \"demo\"\nversion = \"1.0.0\"\nentrypoint = \"lib.typ\"\n",
        )
        .unwrap();
        std::fs::write(package.join("lib.typ"), "#let shout(body) = upper(body)\n").unwrap();
        let cached = load(
            &crate::uri::from_file_path(&package.join("lib.typ")).unwrap(),
            site.path(),
            &SourceOverrides::default(),
            &base_source_boundary(site.path(), false),
            None,
            None,
        )
        .expect("the package source loads");
        assert!(
            load(
                &crate::uri::from_file_path(&package.join("lib.typ")).unwrap(),
                site.path(),
                &SourceOverrides::default(),
                &base_source_boundary(site.path(), true),
                None,
                None,
            )
            .is_none()
        );
        assert_eq!(cached.text(), "#let shout(body) = upper(body)\n");
        let id = cached.id();
        let VirtualRoot::Package(spec) = id.root() else {
            panic!("{id:?}");
        };
        assert_eq!(
            (spec.namespace.as_str(), spec.name.as_str()),
            ("preview", "demo")
        );
        assert_eq!(id.vpath().get_without_slash(), "lib.typ");

        // A file below the site's package view, or below the client's own, loads as the package
        // document it mirrors.
        let root = site.path();
        let package_uri: Uri = "tola-package:/tola/source/0.0.0/lib.typ".parse().unwrap();
        let id = crate::identity::file_id(&package_uri, root).expect("a builtin package");
        let source = crate::identity::embedded_source(id).expect("a builtin source");
        let boundary = base_source_boundary(root, true);
        let configured = tempfile::tempdir().unwrap();
        let mirror = tola_build::filesystem::layout::builtin_packages(root)
            .join("tola/source/0.0.0/lib.typ");
        for (path, package_sources) in [
            (mirror.clone(), None),
            (
                configured.path().join("tola/source/0.0.0/lib.typ"),
                Some(configured.path()),
            ),
        ] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, source.as_bytes()).unwrap();
            let loaded = load(
                &crate::uri::from_file_path(&path).unwrap(),
                root,
                &SourceOverrides::default(),
                &boundary,
                package_sources,
                None,
            )
            .expect("the package view loads");
            assert_eq!(loaded.id(), id, "{path:?}");
            assert_eq!(loaded.text(), source);
        }

        // Bytes another Tola wrote are not this implementation's package source.
        std::fs::write(&mirror, "stale\n").unwrap();
        assert!(
            load(
                &crate::uri::from_file_path(&mirror).unwrap(),
                root,
                &SourceOverrides::default(),
                &boundary,
                None,
                None,
            )
            .is_none()
        );
    }
    #[test]
    fn utf16_edits_preserve_prior_sources() {
        let directory = tempfile::tempdir().unwrap();
        let uri = crate::uri::from_file_path(&directory.path().join("document.typ")).unwrap();
        let mut sources = OpenSources::new(directory.path());
        let original = "a🦊b\r\n正文";
        sources.apply(opened(uri.clone(), original)).unwrap();
        let previous = sources.view();
        let after_first = "a猫b\r\n正文";
        let end = span(after_first, "正文").end;
        sources
            .apply(changed(
                uri.clone(),
                2,
                vec![
                    edit(Some(span(original, "🦊")), "猫"),
                    edit(Some(Range::new(end, end)), "！"),
                ],
            ))
            .unwrap();
        assert_eq!(sources.source(&uri).unwrap().text(), "a猫b\r\n正文！");
        assert_eq!(sources.version(uri.as_str()), Some(2));
        assert_eq!(previous.get(&uri).unwrap().source.text(), original);
    }
    /// A retargeted symlink answers as the source it now resolves to, and the view a request
    /// already holds keeps the source it read.
    #[test]
    #[cfg(unix)]
    fn symlink_retarget_follows_the_resolved_source() {
        fn changed_file(uri: &Uri) -> Notification {
            Notification::new(
                notification::DidChangeWatchedFiles::METHOD.into(),
                lsp_types::DidChangeWatchedFilesParams {
                    changes: vec![lsp_types::FileEvent::new(
                        uri.clone(),
                        lsp_types::FileChangeType::CHANGED,
                    )],
                },
            )
        }

        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.typ");
        let second = directory.path().join("second.typ");
        let asset = directory.path().join("asset.txt");
        let external = outside.path().join("external.typ");
        for path in [&first, &second, &asset, &external] {
            std::fs::write(path, "disk").unwrap();
        }
        let alias = directory.path().join("document.typ");
        std::os::unix::fs::symlink(&first, &alias).unwrap();
        let uri: Uri = url::Url::from_file_path(&alias)
            .unwrap()
            .as_str()
            .parse()
            .unwrap();
        let mut open = OpenSources::new(directory.path());
        open.apply(opened(uri.clone(), "buffer")).unwrap();
        let original = open.view();
        let original_id = open.source(&uri).unwrap().id();
        assert_eq!(
            open.open_source_texts(),
            vec![(first.canonicalize().unwrap(), Arc::from("buffer"))]
        );

        let retarget = |target: &Path| {
            std::fs::remove_file(&alias).unwrap();
            std::os::unix::fs::symlink(target, &alias).unwrap();
        };

        // The buffer follows the source the alias resolves to now.
        retarget(&second);
        open.apply(changed_file(&uri)).unwrap();
        assert_eq!(
            open.open_source_texts(),
            vec![(second.canonicalize().unwrap(), Arc::from("buffer"))]
        );
        assert_ne!(open.source(&uri).unwrap().id(), original_id);
        assert_eq!(open.version(uri.as_str()), Some(1));

        // A target that is no Typst source drops the tracked identity.
        for target in [&external, &asset] {
            retarget(target);
            open.apply(changed_file(&uri)).unwrap();
            assert!(open.source(&uri).is_none(), "{target:?}");
            assert_eq!(open.version(uri.as_str()), Some(1));
        }

        // Retargeting the site source back restores its identity and the buffer text.
        retarget(&first);
        open.apply(changed_file(&uri)).unwrap();
        let restored = open.source(&uri).unwrap();
        assert_eq!(restored.id(), original_id);
        assert_eq!(restored.text(), "buffer");
        assert_eq!(open.version(uri.as_str()), Some(1));

        // The view a request already holds keeps what it read.
        assert_eq!(original.get(&uri).unwrap().source.id(), original_id);
        assert_eq!(original.get(&uri).unwrap().source.text(), "buffer");
    }
}
