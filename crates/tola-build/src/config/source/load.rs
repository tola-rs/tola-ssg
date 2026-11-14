//! Snapshot and apply configuration files in include order.

use super::{ConfigFieldOrigins, ConfigSourceError, merge};
use crate::cancellation::{BuildCancellation, BuildCancelled};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io,
    ops::ControlFlow,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};
use tola_config::ValueStep;
use tola_diagnostic::{
    Diagnostic, DiagnosticError, RelatedInformation, Severity, SourcePositions, SourceRange,
};
use tola_input::InputScope;

/// One logical configuration file and its immutable parsed snapshot.
#[derive(Debug, Clone)]
pub struct ConfigFile {
    path: PathBuf,
    physical: PathBuf,
    text: Arc<str>,
    document: Arc<toml_edit::Document<Arc<str>>>,
    value: Arc<toml::Value>,
    hash: blake3::Hash,
    positions: SourcePositions,
    includes: Vec<Include>,
}

#[derive(Debug, Clone)]
struct Include {
    path: PathBuf,
    optional: bool,
    pointer: String,
}

impl ConfigFile {
    pub(super) fn parse(
        path: PathBuf,
        physical: PathBuf,
        text: Arc<str>,
        root: &Path,
    ) -> Result<Self, ConfigSourceError> {
        let document = toml_edit::Document::parse(Arc::clone(&text)).map_err(|error| {
            let diagnostic = super::super::diagnostic::syntax_error_at_root(
                root,
                &path,
                &text,
                error.message(),
                error.span(),
            );
            ConfigSourceError::Syntax(DiagnosticError::attach(error.into(), vec![diagnostic]))
        })?;
        let mut value = toml::Value::deserialize(toml_edit::de::Deserializer::from(
            document.clone(),
        ))
        .map_err(|error| {
            let diagnostic = super::super::diagnostic::syntax_error_at_root(
                root,
                &path,
                &text,
                error.message(),
                error.span(),
            );
            ConfigSourceError::Schema(DiagnosticError::attach(error.into(), vec![diagnostic]))
        })?;
        let raw_positions = SourcePositions::of(&document, Arc::clone(&text));
        let mut positions = SourcePositions::in_root(root.to_owned());
        positions.copy_subtree(&raw_positions, "", "", &path);
        let mut includes = Vec::new();
        if let Some(include) = value
            .as_table_mut()
            .expect("TOML root is a table")
            .remove("include")
        {
            let array = include.as_array().ok_or_else(|| {
                invalid(&path, &positions, "/include", "include must be an array")
            })?;
            for (index, entry) in array.iter().enumerate() {
                let pointer = format!("/include/{index}");
                let (name, optional) = if let Some(name) = entry.as_str() {
                    (name, false)
                } else if let Some(table) = entry.as_table() {
                    if table.keys().any(|key| key != "path" && key != "optional") {
                        return Err(invalid(
                            &path,
                            &positions,
                            &pointer,
                            "include entries accept only `path` and `optional`",
                        ));
                    }
                    let name =
                        table
                            .get("path")
                            .and_then(toml::Value::as_str)
                            .ok_or_else(|| {
                                invalid(
                                    &path,
                                    &positions,
                                    &pointer,
                                    "include entry requires a string `path`",
                                )
                            })?;
                    let optional = match table.get("optional") {
                        None => false,
                        Some(value) => value.as_bool().ok_or_else(|| {
                            invalid(
                                &path,
                                &positions,
                                &pointer,
                                "include `optional` must be a boolean",
                            )
                        })?,
                    };
                    (name, optional)
                } else {
                    return Err(invalid(
                        &path,
                        &positions,
                        &pointer,
                        "include entry must be a path string or table",
                    ));
                };
                if name.is_empty() || name.contains('\0') {
                    return Err(invalid(
                        &path,
                        &positions,
                        &pointer,
                        "include path must be a nonempty local file path",
                    ));
                }
                includes.push(Include {
                    path: PathBuf::from(name),
                    optional,
                    pointer,
                });
            }
        }
        Ok(Self {
            path,
            physical,
            hash: blake3::hash(text.as_bytes()),
            text,
            document: Arc::new(document),
            value: Arc::new(value),
            positions,
            includes,
        })
    }
    fn at_path(&self, path: PathBuf, root: &Path) -> Self {
        let raw = SourcePositions::of(&self.document, Arc::clone(&self.text));
        let mut positions = SourcePositions::in_root(root.to_owned());
        positions.copy_subtree(&raw, "", "", &path);
        Self {
            path,
            positions,
            ..self.clone()
        }
    }
    /// The logical path used by this reference.
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Digest of this file's raw text.
    pub fn hash(&self) -> blake3::Hash {
        self.hash
    }
    pub(super) fn positions(&self) -> &SourcePositions {
        &self.positions
    }
    pub(super) fn value(&self) -> &toml::Value {
        &self.value
    }
}

/// One occurrence of a file in the include graph.
#[derive(Debug, Clone)]
pub struct ConfigApplication {
    pub id: usize,
    pub path: PathBuf,
    pub apply_order: Option<usize>,
    pub included_by: Option<ConfigInclude>,
    pub state: ConfigFileState,
}
/// The result of visiting one include.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFileState {
    Loaded,
    OptionalMissing,
}
/// The include entry that caused an application.
#[derive(Debug, Clone)]
pub struct ConfigInclude {
    pub source_id: usize,
    pub line: usize,
    pub column: usize,
    pub range: Option<SourceRange>,
}

#[derive(Debug)]
pub(super) struct Loaded {
    pub files: Vec<ConfigFile>,
    pub applications: Vec<ConfigApplication>,
    pub application_files: Vec<Option<usize>>,
    pub value: toml::Value,
    pub positions: Arc<SourcePositions>,
    pub origins: BTreeMap<Vec<ValueStep>, ConfigFieldOrigins>,
    pub paths: Vec<PathBuf>,
    pub hash: blake3::Hash,
}

struct Frame {
    file: usize,
    application: usize,
    next: usize,
}

pub(super) fn load(
    path: PathBuf,
    root: PathBuf,
    scope: InputScope,
    overlays: &[(PathBuf, Arc<str>)],
    cancellation: Option<&BuildCancellation>,
) -> Result<Loaded, ConfigSourceError> {
    let display_root = path.parent().unwrap_or(&path).to_owned();
    let mut loader = Loader {
        root: &root,
        display_root: &display_root,
        scope,
        overlays,
        cancellation,
        files: Vec::new(),
        file_indices: HashMap::new(),
        physical_indices: HashMap::new(),
        applications: Vec::new(),
        application_files: Vec::new(),
        paths: vec![path.clone()],
        cache: HashMap::new(),
        missing: HashSet::new(),
        observations: Vec::new(),
        value: toml::Value::Table(Default::default()),
        positions: SourcePositions::in_root(display_root.clone()),
        origins: BTreeMap::new(),
        hash: blake3::Hasher::new(),
    };
    let result = loader.run(path);
    match result {
        Ok(()) => Ok(Loaded {
            files: loader.files,
            applications: loader.applications,
            application_files: loader.application_files,
            value: loader.value,
            positions: Arc::new(loader.positions),
            origins: loader.origins,
            paths: loader.paths,
            hash: loader.hash.finalize(),
        }),
        Err(error) => Err(error.with_paths(loader.paths)),
    }
}

struct Loader<'a> {
    root: &'a Path,
    display_root: &'a Path,
    scope: InputScope,
    overlays: &'a [(PathBuf, Arc<str>)],
    cancellation: Option<&'a BuildCancellation>,
    files: Vec<ConfigFile>,
    file_indices: HashMap<PathBuf, usize>,
    physical_indices: HashMap<PathBuf, usize>,
    applications: Vec<ConfigApplication>,
    application_files: Vec<Option<usize>>,
    paths: Vec<PathBuf>,
    cache: HashMap<PathBuf, Arc<str>>,
    missing: HashSet<PathBuf>,
    observations: Vec<(PathBuf, PathBuf, Option<std::fs::Metadata>)>,
    value: toml::Value,
    positions: SourcePositions,
    origins: BTreeMap<Vec<ValueStep>, ConfigFieldOrigins>,
    hash: blake3::Hasher,
}
impl Loader<'_> {
    fn run(&mut self, path: PathBuf) -> Result<(), ConfigSourceError> {
        let (id, file) = self.visit(path, false, None)?;
        let mut stack = vec![Frame {
            file: file.expect("entry exists"),
            application: id,
            next: 0,
        }];
        while let Some(frame) = stack.last_mut() {
            if let Some(cancellation) = self.cancellation {
                cancellation.ensure_active()?;
            }
            let file = &self.files[frame.file];
            if let Some(include) = file.includes.get(frame.next).cloned() {
                frame.next += 1;
                let next = file.path.parent().unwrap_or(&file.path).join(&include.path);
                if !self.paths.contains(&next) {
                    self.paths.push(next.clone());
                }
                let written = file.positions.at_pointer(&include.pointer);
                let excerpt = written.and_then(|position| position.value().excerpt());
                let parent = ConfigInclude {
                    source_id: frame.application,
                    line: excerpt.as_ref().map_or(1, |e| e.line),
                    column: excerpt.as_ref().map_or(1, |e| e.column),
                    range: written.map(|position| position.value().range()),
                };
                let (id, child) = self
                    .visit(next.clone(), include.optional, Some(parent))
                    .map_err(|error| self.include_error(error, &stack, &include.pointer))?;
                if let Some(child) = child {
                    if stack
                        .iter()
                        .any(|frame| self.files[frame.file].physical == self.files[child].physical)
                    {
                        return Err(self.include_error(
                            self.failure(&next, "configuration include cycle"),
                            &stack,
                            &include.pointer,
                        ));
                    }
                    stack.push(Frame {
                        file: child,
                        application: id,
                        next: 0,
                    });
                }
            } else {
                let frame = stack.pop().unwrap();
                let file = &self.files[frame.file];
                let order = self
                    .applications
                    .iter()
                    .filter(|a| a.apply_order.is_some())
                    .count();
                self.applications[frame.application].apply_order = Some(order);
                merge::apply(
                    &mut self.value,
                    file.value(),
                    &mut self.positions,
                    file,
                    frame.application,
                    &mut self.origins,
                );
            }
        }
        for (path, physical, before) in &self.observations {
            super::check_source_path(self.root, path, self.scope, None)?;
            let after = metadata(path).map_err(|error| self.io_error(path, error))?;
            let current = physical_path(path).map_err(|error| self.io_error(path, error))?;
            let unchanged = match (before, &after) {
                (None, None) => true,
                (Some(a), Some(b)) => same_metadata(a, b),
                _ => false,
            };
            if current != *physical || !unchanged {
                return Err(self.failure(path, "configuration file changed while being loaded"));
            }
        }
        Ok(())
    }
    fn visit(
        &mut self,
        path: PathBuf,
        optional: bool,
        included_by: Option<ConfigInclude>,
    ) -> Result<(usize, Option<usize>), ConfigSourceError> {
        if let Some(cancellation) = self.cancellation {
            cancellation.ensure_active()?;
        }
        super::check_source_path(self.root, &path, self.scope, None)?;
        let physical = physical_path(&path).map_err(|error| self.io_error(&path, error))?;
        if !self.paths.contains(&physical) {
            self.paths.push(physical.clone());
        }
        super::check_source_path(self.root, &physical, self.scope, None)?;
        let observed = metadata(&path).map_err(|error| self.io_error(&path, error))?;
        if observed
            .as_ref()
            .is_some_and(|metadata| !metadata.is_file())
        {
            return Err(file_failure(
                self.display_root,
                &path,
                tola_diagnostic::codes::config::IO,
                "configuration path is not a regular file",
            ));
        }
        if self.observations.iter().any(|(_, previous, metadata)| {
            previous == &physical
                && match (metadata, &observed) {
                    (None, None) => false,
                    (Some(a), Some(b)) => !same_metadata(a, b),
                    _ => true,
                }
        }) {
            return Err(self.failure(&path, "configuration file changed while being loaded"));
        }
        self.observations
            .push((path.clone(), physical.clone(), observed.clone()));
        let mut overlay = None;
        for (candidate, text) in self.overlays {
            let same = candidate == &path
                || physical_path(candidate).is_ok_and(|candidate| candidate == physical);
            if same {
                if overlay
                    .as_ref()
                    .is_some_and(|previous: &Arc<str>| previous.as_ref() != text.as_ref())
                {
                    return Err(self.failure(
                        &path,
                        "conflicting editor contents for the same configuration file",
                    ));
                }
                overlay = Some(Arc::clone(text));
            }
        }
        let from_overlay = overlay.is_some();
        let text = if self.missing.contains(&physical) {
            if optional {
                None
            } else {
                return Err(self.io_error(&path, io::Error::from(io::ErrorKind::NotFound)));
            }
        } else if let Some(text) = self.cache.get(&physical) {
            Some(Arc::clone(text))
        } else if let Some(text) = overlay {
            self.cache.insert(physical.clone(), Arc::clone(&text));
            Some(text)
        } else {
            match self.read(&path, &physical, observed.as_ref()) {
                Ok(text) => {
                    self.cache.insert(physical.clone(), Arc::clone(&text));
                    Some(text)
                }
                Err(ReadFailure::Missing) if optional => {
                    self.missing.insert(physical.clone());
                    None
                }
                Err(ReadFailure::Missing) => {
                    return Err(self.io_error(&path, io::Error::from(io::ErrorKind::NotFound)));
                }
                Err(ReadFailure::Error(error)) => return Err(error),
            }
        };
        let id = self.applications.len();
        let file = match text {
            Some(text) => {
                if let Some(index) = self.file_indices.get(&path) {
                    Some(*index)
                } else {
                    let file = if let Some(index) = self.physical_indices.get(&physical) {
                        self.files[*index].at_path(path.clone(), self.display_root)
                    } else {
                        ConfigFile::parse(path.clone(), physical.clone(), text, self.display_root)?
                    };
                    for include in &file.includes {
                        let pending = path.parent().unwrap_or(&path).join(&include.path);
                        if !self.paths.contains(&pending) {
                            self.paths.push(pending);
                        }
                    }
                    let index = self.files.len();
                    self.files.push(file);
                    self.file_indices.insert(path.clone(), index);
                    self.physical_indices
                        .entry(physical.clone())
                        .or_insert(index);
                    Some(index)
                }
            }
            None => None,
        };
        let logical_bytes = path.as_os_str().as_encoded_bytes();
        let physical_bytes = physical.as_os_str().as_encoded_bytes();
        self.hash
            .update(&(logical_bytes.len() as u64).to_le_bytes());
        self.hash.update(logical_bytes);
        self.hash
            .update(&(physical_bytes.len() as u64).to_le_bytes());
        self.hash.update(physical_bytes);
        self.hash.update(&[u8::from(file.is_some())]);
        // Equal bytes from a buffer and disk have different freshness obligations.
        self.hash.update(&[u8::from(from_overlay)]);
        self.hash.update(
            &(included_by
                .as_ref()
                .map_or(usize::MAX, |parent| parent.source_id) as u64)
                .to_le_bytes(),
        );
        if let Some(index) = file {
            self.hash.update(self.files[index].hash.as_bytes());
        }
        self.applications.push(ConfigApplication {
            id,
            path,
            apply_order: None,
            included_by,
            state: if file.is_some() {
                ConfigFileState::Loaded
            } else {
                ConfigFileState::OptionalMissing
            },
        });
        self.application_files.push(file);
        Ok((id, file))
    }
    fn read(
        &mut self,
        path: &Path,
        physical: &Path,
        observed: Option<&std::fs::Metadata>,
    ) -> Result<Arc<str>, ReadFailure> {
        let mut file = match tola_fs::open_regular_file(path, true) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound && observed.is_none() => {
                return Err(ReadFailure::Missing);
            }
            Err(error) => return Err(ReadFailure::Error(self.io_error(path, error))),
        };
        let before = file
            .metadata()
            .map_err(|e| ReadFailure::Error(self.io_error(path, e)))?;
        let canonical =
            std::fs::canonicalize(path).map_err(|e| ReadFailure::Error(self.io_error(path, e)))?;
        if canonical != physical
            || !observed.is_some_and(|metadata| same_metadata(metadata, &before))
        {
            return Err(ReadFailure::Error(
                self.failure(path, "configuration file changed before being read"),
            ));
        }
        let idle = AtomicBool::new(false);
        let flag = self.cancellation.map_or(&idle, BuildCancellation::flag);
        let mut bytes = Vec::new();
        let _ = tola_input::read_chunks(&mut file, flag, |chunk| {
            bytes.extend_from_slice(chunk);
            ControlFlow::Continue(())
        })
        .map_err(|error| match error {
            tola_input::FileReadError::Cancelled(_) => ReadFailure::Error(BuildCancelled.into()),
            tola_input::FileReadError::Io(error) => ReadFailure::Error(self.io_error(path, error)),
        })?;
        let after = file
            .metadata()
            .map_err(|e| ReadFailure::Error(self.io_error(path, e)))?;
        let current =
            std::fs::metadata(path).map_err(|e| ReadFailure::Error(self.io_error(path, e)))?;
        let canonical =
            std::fs::canonicalize(path).map_err(|e| ReadFailure::Error(self.io_error(path, e)))?;
        if canonical != physical
            || !same_metadata(&before, &after)
            || !same_metadata(&after, &current)
        {
            return Err(ReadFailure::Error(
                self.failure(path, "configuration file changed while being read"),
            ));
        }
        super::check_source_path(self.root, path, self.scope, None).map_err(ReadFailure::Error)?;
        String::from_utf8(bytes).map(Arc::from).map_err(|_| {
            ReadFailure::Error(self.failure(path, "configuration file is not valid UTF-8"))
        })
    }
    fn failure(&self, path: &Path, message: &str) -> ConfigSourceError {
        file_failure(
            self.display_root,
            path,
            tola_diagnostic::codes::config::INVALID,
            message,
        )
    }
    fn io_error(&self, path: &Path, error: io::Error) -> ConfigSourceError {
        let message = format!("cannot read configuration file: {error}");
        let diagnostic = Diagnostic::at_path(
            tola_diagnostic::codes::config::IO,
            Severity::Error,
            tola_diagnostic::display_path(path, self.display_root),
            &message,
        );
        ConfigSourceError::Read(DiagnosticError::attach(error.into(), vec![diagnostic]))
    }
    fn include_error(
        &self,
        error: ConfigSourceError,
        stack: &[Frame],
        pointer: &str,
    ) -> ConfigSourceError {
        if error.diagnostics().is_empty() {
            return error;
        }
        let mut diagnostics = error.diagnostics().to_vec();
        for (index, frame) in stack.iter().enumerate().rev() {
            let file = &self.files[frame.file];
            let pointer = if index + 1 == stack.len() {
                pointer
            } else {
                &file.includes[frame.next - 1].pointer
            };
            let written = file
                .positions
                .at_pointer(pointer)
                .map(|position| position.value());
            let excerpt = written.and_then(|position| position.excerpt());
            let location = tola_diagnostic::Location {
                file: Some(file.path().to_owned()),
                path: tola_diagnostic::display_path(file.path(), self.display_root),
                line: excerpt.as_ref().map(|value| value.line),
                column: excerpt.as_ref().map(|value| value.column),
                range: written.map(|position| position.range()),
                source_lines: excerpt
                    .map(|value| {
                        vec![tola_diagnostic::SourceLine::new(
                            value.line,
                            value.text,
                            Some(value.highlight),
                        )]
                    })
                    .unwrap_or_default(),
            };
            for diagnostic in &mut diagnostics {
                diagnostic.related.push(RelatedInformation {
                    message: "included here".into(),
                    location: Some(location.clone()),
                    source: None,
                });
            }
        }
        ConfigSourceError::Read(DiagnosticError::attach(error.into(), diagnostics))
    }
}
enum ReadFailure {
    Missing,
    Error(ConfigSourceError),
}
fn file_failure(
    root: &Path,
    path: &Path,
    code: tola_diagnostic::DiagnosticCode,
    message: &str,
) -> ConfigSourceError {
    let diagnostic = Diagnostic::at_path(
        code,
        Severity::Error,
        tola_diagnostic::display_path(path, root),
        message,
    );
    ConfigSourceError::Read(DiagnosticError::new(message, vec![diagnostic]))
}

fn invalid(
    path: &Path,
    positions: &SourcePositions,
    pointer: &str,
    reason: &str,
) -> ConfigSourceError {
    let diagnostic =
        super::super::diagnostic::value_error(path, positions, pointer, "include", reason);
    ConfigSourceError::Schema(DiagnosticError::new(reason, vec![diagnostic]))
}
fn physical_path(path: &Path) -> io::Result<PathBuf> {
    let mut path = path.to_owned();
    let mut links = HashSet::new();
    loop {
        let mut prefix = path.as_path();
        let mut suffix = Vec::new();
        loop {
            match std::fs::canonicalize(prefix) {
                Ok(mut physical) => {
                    for component in suffix.into_iter().rev() {
                        physical.push(component);
                    }
                    return Ok(physical);
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    match std::fs::symlink_metadata(prefix) {
                        Ok(metadata) if metadata.file_type().is_symlink() => {
                            let parent = prefix.parent().ok_or(error)?;
                            let name = prefix
                                .file_name()
                                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
                            let identity = std::fs::canonicalize(parent)?.join(name);
                            if !links.insert(identity) {
                                return Err(io::Error::other("symbolic link cycle"));
                            }
                            let target = std::fs::read_link(prefix)?;
                            let mut next = parent.join(target);
                            for component in suffix.into_iter().rev() {
                                next.push(component);
                            }
                            path = next;
                            break;
                        }
                        Ok(_) => {}
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error),
                    }
                    let component = prefix
                        .components()
                        .next_back()
                        .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
                    let parent = prefix.parent().ok_or(error)?;
                    suffix.push(component.as_os_str().to_owned());
                    prefix = parent;
                }
                Err(error) => return Err(error),
            }
        }
    }
}
fn metadata(path: &Path) -> io::Result<Option<std::fs::Metadata>> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}
fn same_metadata(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        a.dev() == b.dev()
            && a.ino() == b.ino()
            && a.len() == b.len()
            && a.mtime() == b.mtime()
            && a.mtime_nsec() == b.mtime_nsec()
            && a.ctime() == b.ctime()
            && a.ctime_nsec() == b.ctime_nsec()
    }
    #[cfg(not(unix))]
    {
        a.len() == b.len()
            && a.modified().ok() == b.modified().ok()
            && a.created().ok() == b.created().ok()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::symlink;

    use tola_input::InputScope;

    use super::load;

    #[test]
    fn optional_dangling_link_tracks_target_and_its_creation() {
        let directory = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        let entry = root.join("tola.toml");
        let link = root.join("optional.toml");
        let target = root.join("missing.toml");
        std::fs::write(
            &entry,
            "include = [{ path = 'optional.toml', optional = true }]\n",
        )
        .unwrap();
        symlink("missing.toml", &link).unwrap();
        let before = load(entry.clone(), root.clone(), InputScope::Pure, &[], None).unwrap();
        assert!(before.paths.contains(&link));
        assert!(before.paths.contains(&target));
        std::fs::write(&target, "[site]\ntitle = 'present'\n").unwrap();
        let after = load(entry, root, InputScope::Pure, &[], None).unwrap();
        assert_ne!(before.hash, after.hash);
    }

    #[test]
    fn pure_scope_rejects_optional_dangling_link_outside_site() {
        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        let entry = root.join("tola.toml");
        std::fs::write(
            &entry,
            "include = [{ path = 'optional.toml', optional = true }]\n",
        )
        .unwrap();
        symlink(
            outside.path().join("missing.toml"),
            root.join("optional.toml"),
        )
        .unwrap();
        assert!(load(entry, root, InputScope::Pure, &[], None).is_err());
    }
}
