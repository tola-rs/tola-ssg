//! Discovery and identity for the `.typ` sources of the site's content tree.

mod slug;
mod sources;

pub use sources::{ContentSource, SourceDescriptorField, SourceDescriptorFieldKind, SourceSet};
pub(crate) use sources::{SourceFileInput, SourceMetadataDeclaration, SourceRecords, SourcesInput};

use std::collections::{BTreeSet, HashSet};
use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};

use tola_input::normalize_path;

/// Path identifying a source relative to `build.content-dir`, including its extension.
/// Renaming the file or moving it within the content tree changes this ID.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentId(PathBuf);

impl ContentId {
    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl fmt::Display for ContentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&tola_diagnostic::render_relative(self.as_path()))
    }
}

/// Source layout within the content tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentSourceLayout {
    SingleFile,
    DirectoryIndex,
}

impl ContentSourceLayout {
    pub fn from_entry_path(path: &Path) -> Self {
        if is_index_source(path) {
            Self::DirectoryIndex
        } else {
            Self::SingleFile
        }
    }
}

/// A discovered content entry and its canonical source path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentUnit {
    pub root: PathBuf,
    pub id: ContentId,
    pub source: PathBuf,
    pub layout: ContentSourceLayout,
}

/// Select canonical content entry sources for a read-only source inspection.
pub(crate) fn select_content_sources(
    config: &crate::config::SiteConfig,
    scopes: &[PathBuf],
    units: &[ContentUnit],
) -> Result<Vec<PathBuf>, ContentDiscoveryError> {
    if scopes.is_empty() {
        return Ok(units.iter().map(|unit| unit.source.clone()).collect());
    }

    let site_root = normalize_path(config.root());
    let mut selected = BTreeSet::new();
    for scope in scopes {
        let mut matched = false;
        let mut candidates = BTreeSet::new();
        if scope.is_absolute() {
            candidates.insert(normalize_path(scope));
        } else {
            if scope.exists() {
                candidates.insert(normalize_path(scope));
            }
            candidates.insert(normalize_path(&site_root.join(scope)));
            candidates.insert(normalize_path(&config.build.content_dir.join(scope)));
        }

        for resolved in candidates {
            for unit in units_named_by(&resolved, units) {
                matched = true;
                selected.insert(&unit.source);
            }
        }

        if !matched {
            return Err(ContentDiscoveryError::NoSourceMatches {
                scope: tola_diagnostic::display_path(scope, &site_root),
            });
        }
    }
    Ok(selected.into_iter().cloned().collect())
}

pub(crate) fn discover_content_units_for_config_with_cancellation(
    config: &crate::config::SiteConfig,
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<Vec<ContentUnit>, ContentDiscoveryError> {
    crate::resources::source_boundary(config, tola_input::InputScope::Online)
        .check(&config.build.content_dir)?;
    discover_from_root(
        &config.build.content_dir,
        config.root(),
        &config.build.entry,
        cancellation,
    )
}

/// Whether accepted changes may alter the discovered content inventory.
pub(crate) fn inventory_may_change(
    config: &crate::config::SiteConfig,
    inventory: &[ContentUnit],
    changed_paths: &[PathBuf],
) -> bool {
    if changed_paths.is_empty() {
        return false;
    }
    let content_root = tola_input::normalize_existing_prefix(&config.build.content_dir);
    let known_sources = inventory
        .iter()
        .map(|unit| unit.source.as_path())
        .collect::<HashSet<_>>();
    changed_paths.iter().any(|changed| {
        let changed = tola_input::normalize_existing_prefix(changed);
        if !changed.starts_with(&content_root) && !content_root.starts_with(&changed) {
            return false;
        }

        if known_sources.contains(changed.as_path()) && changed.is_file() {
            return false;
        }

        true
    })
}

/// Why a content source inventory could not be built.
#[derive(Debug, thiserror::Error)]
pub enum ContentDiscoveryError {
    #[error(transparent)]
    Cancelled(#[from] crate::cancellation::BuildCancelled),
    #[error(transparent)]
    Boundary(#[from] typst::diag::FileError),
    #[error(
        "no discovered Typst source matches `{scope}`; run `tola inspect sources` to list them"
    )]
    NoSourceMatches { scope: String },
    #[error(
        "content root `{path}` does not exist; create it or set `build.content-dir` in `tola.toml`"
    )]
    MissingContentRoot { path: String },
    #[error("cannot inspect content entry in `{path}`: {reason}")]
    UnreadableEntry { path: String, reason: String },
    #[error("{reason}")]
    UnreadableDirectory { reason: String },
}

/// The units one resolved scope path names: a file, a directory tree, or a stem.
fn units_named_by<'units>(resolved: &Path, all: &'units [ContentUnit]) -> Vec<&'units ContentUnit> {
    if resolved.is_file() {
        return all.iter().filter(|unit| unit.source == resolved).collect();
    }
    if resolved.is_dir() {
        return all
            .iter()
            .filter(|unit| unit.source.starts_with(resolved))
            .collect();
    }
    let source_candidates = [resolved.with_extension("typ"), resolved.join("index.typ")]
        .map(|candidate| normalize_path(&candidate));
    all.iter()
        .filter(|unit| source_candidates.contains(&unit.source))
        .collect()
}

fn discover_from_root(
    content_root: &Path,
    display_root: &Path,
    entry: &Path,
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<Vec<ContentUnit>, ContentDiscoveryError> {
    cancellation.ensure_active()?;
    if !content_root.is_dir() {
        return Err(ContentDiscoveryError::MissingContentRoot {
            path: tola_diagnostic::display_path(content_root, display_root),
        });
    }
    // The walk resolves the root once and neither descends nor collects a symbolic link.
    let root = tola_input::normalize_existing_prefix(content_root);
    let entry = tola_input::normalize_existing_prefix(entry);
    let mut sources = Vec::new();
    collect_typ_sources(&root, display_root, cancellation, &mut sources)?;
    let mut units = Vec::with_capacity(sources.len());
    for source in sources {
        cancellation.ensure_active()?;
        if source != entry {
            units.push(unit_for(&root, source));
        }
    }
    units.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(units)
}

/// Collect every `.typ` source beneath `directory` on the calling thread.
fn collect_typ_sources(
    directory: &Path,
    display_root: &Path,
    cancellation: &crate::cancellation::BuildCancellation,
    sources: &mut Vec<PathBuf>,
) -> Result<(), ContentDiscoveryError> {
    cancellation.ensure_active()?;
    let entries = match crate::filesystem::read_sorted_entries(
        directory,
        cancellation,
        display_root,
        "content",
    ) {
        Ok(entries) => entries,
        Err(error) if crate::cancellation::is_cancelled(&error) => {
            return Err(ContentDiscoveryError::Cancelled(
                crate::cancellation::BuildCancelled,
            ));
        }
        Err(error) => {
            return Err(ContentDiscoveryError::UnreadableDirectory {
                reason: error.to_string(),
            });
        }
    };
    for entry in entries {
        cancellation.ensure_active()?;
        let file_name = entry.file_name();
        if content_name_is_hidden(&file_name) {
            continue;
        }
        let file_type =
            entry
                .file_type()
                .map_err(|error| ContentDiscoveryError::UnreadableEntry {
                    path: tola_diagnostic::display_path(directory, display_root),
                    reason: crate::filesystem::path_failure_reason(&error).to_owned(),
                })?;
        let path = entry.path();
        // A symbolic link is neither descended nor collected: `file_type` does not follow it.
        if file_type.is_dir() {
            collect_typ_sources(&path, display_root, cancellation, sources)?;
        } else if file_type.is_file() && has_typ_extension(&path) {
            sources.push(path);
        }
    }
    Ok(())
}

/// Whether a content entry is a name the author keeps out of discovery.
fn content_name_is_hidden(file_name: &OsStr) -> bool {
    file_name.to_str().is_some_and(|name| name.starts_with('.'))
}

/// Add unsaved editor sources; discovered units are already canonical.
pub(crate) fn content_units_with_sources(
    config: &crate::config::SiteConfig,
    inventory: &[ContentUnit],
    opened: impl IntoIterator<Item = PathBuf>,
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<Vec<ContentUnit>, ContentDiscoveryError> {
    let root = tola_input::normalize_existing_prefix(&config.build.content_dir);
    let entry = tola_input::normalize_existing_prefix(&config.build.entry);
    let mut units = inventory.to_vec();
    let mut known = units
        .iter()
        .map(|unit| unit.source.clone())
        .collect::<HashSet<_>>();
    for source in opened {
        cancellation.ensure_active()?;
        let Some(unit) = eligible_unit(&root, &entry, &source) else {
            continue;
        };
        if known.insert(unit.source.clone()) {
            units.push(unit);
        }
    }
    units.sort_by(|left, right| left.id.cmp(&right.id));
    units.dedup_by(|left, right| left.id == right.id);
    Ok(units)
}

/// The unit a normalized source names, when membership admits it.
fn eligible_unit(root: &Path, entry: &Path, source: &Path) -> Option<ContentUnit> {
    let source = tola_input::normalize_existing_prefix(source);
    if source == *entry || !source.starts_with(root) || !has_typ_extension(&source) {
        return None;
    }
    Some(unit_for(root, source))
}

/// The unit one already-canonical source names beneath `root`.
fn unit_for(root: &Path, source: PathBuf) -> ContentUnit {
    let relative = source
        .strip_prefix(root)
        .expect("content sources are inside the normalized content root");
    ContentUnit {
        root: root.to_path_buf(),
        id: ContentId::new(relative.to_path_buf()),
        layout: ContentSourceLayout::from_entry_path(&source),
        source,
    }
}

/// Content source extension: exactly lowercase `.typ`, never eligibility alone.
pub fn has_typ_extension(path: &Path) -> bool {
    path.extension() == Some(OsStr::new("typ"))
}

fn is_index_source(path: &Path) -> bool {
    path.file_stem().and_then(|stem| stem.to_str()) == Some("index")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use tempfile::TempDir;

    use crate::config::SiteConfig;

    use super::*;

    fn content_units(root: &Path) -> Result<Vec<ContentUnit>, ContentDiscoveryError> {
        discover_from_root(
            root,
            root,
            &root.join("site.typ"),
            &crate::cancellation::BuildCancellation::default(),
        )
    }

    fn configured_content(cfg: &SiteConfig) -> Result<Vec<ContentUnit>, ContentDiscoveryError> {
        discover_content_units_for_config_with_cancellation(
            cfg,
            &crate::cancellation::BuildCancellation::default(),
        )
    }

    fn asset_config(
        root: &Path,
        content: &Path,
        tree: (&Path, &str),
        file: (&Path, &str),
    ) -> SiteConfig {
        use crate::config::section::assets::{
            AssetFileDeclaration, AssetTreeDeclaration, AssetUrl, AssetUrlPrefix,
        };

        let mut cfg = crate::config::tests::load_test_config(root, "");
        cfg.build.content_dir = content.to_path_buf();
        cfg.assets.trees = vec![AssetTreeDeclaration::new(
            tree.0,
            AssetUrlPrefix::parse(tree.1).unwrap(),
        )];
        cfg.assets.files = vec![AssetFileDeclaration::new(
            file.0,
            AssetUrl::parse(file.1).unwrap(),
        )];
        cfg
    }

    #[test]
    fn discovery_no_pool_starvation() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("posts")).unwrap();
        fs::write(dir.path().join("index.typ"), "").unwrap();
        fs::write(dir.path().join("posts/hello.typ"), "").unwrap();

        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        let discovered = pool.install(|| content_units(dir.path()));

        assert_eq!(
            discovered
                .unwrap()
                .iter()
                .map(|u| u.id.to_string())
                .collect::<Vec<_>>(),
            ["index.typ", "posts/hello.typ"]
        );
    }

    #[test]
    fn discovery_cancelled() {
        let dir = TempDir::new().unwrap();
        let canceller = crate::cancellation::BuildCanceller::new();
        canceller.cancel();

        let err = discover_from_root(
            dir.path(),
            dir.path(),
            &dir.path().join("site.typ"),
            &canceller.token(),
        )
        .unwrap_err();

        assert!(matches!(
            err,
            ContentDiscoveryError::Cancelled(crate::cancellation::BuildCancelled)
        ));
    }

    #[test]
    fn discovery_nested_sources() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("posts/rust")).unwrap();
        for src in [
            "posts/a.typ",
            "posts/index.typ",
            "posts/helper.typ",
            "posts/rust/index.typ",
            "posts/rust/helper.typ",
            "posts/rust/ignored.TYP",
        ] {
            fs::write(dir.path().join(src), "").unwrap();
        }

        let units = content_units(dir.path()).unwrap();

        assert_eq!(
            units.iter().map(|u| u.id.to_string()).collect::<Vec<_>>(),
            vec![
                "posts/a.typ",
                "posts/helper.typ",
                "posts/index.typ",
                "posts/rust/helper.typ",
                "posts/rust/index.typ"
            ]
        );
        assert!(units.iter().any(|u| {
            u.id.as_path() == Path::new("posts/rust/index.typ")
                && u.layout == ContentSourceLayout::DirectoryIndex
        }));
        assert!(units.iter().any(|u| {
            u.id.as_path() == Path::new("posts/rust/helper.typ")
                && u.layout == ContentSourceLayout::SingleFile
        }));
    }

    #[test]
    fn asset_declarations_keep_sources() {
        let dir = TempDir::new().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(content.join("assets")).unwrap();
        fs::create_dir_all(content.join("downloads")).unwrap();
        fs::write(content.join("index.typ"), "").unwrap();
        fs::write(content.join("visible.typ"), "").unwrap();
        fs::write(content.join("assets/nested.typ"), "").unwrap();
        fs::write(content.join("downloads/tree-member.typ"), "").unwrap();
        fs::write(content.join("exact-file.typ"), "").unwrap();

        let cfg = asset_config(
            dir.path(),
            &content,
            (&content.join("downloads"), "/downloads"),
            (&content.join("exact-file.typ"), "/exact-file.typ"),
        );

        let units = configured_content(&cfg).unwrap();
        assert_eq!(
            units.iter().map(|u| u.id.to_string()).collect::<Vec<_>>(),
            vec![
                "assets/nested.typ",
                "downloads/tree-member.typ",
                "exact-file.typ",
                "index.typ",
                "visible.typ"
            ]
        );
    }

    #[test]
    fn missing_content_root_reported() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("missing-content");
        let mut cfg = crate::config::tests::load_test_config(dir.path(), "");
        cfg.build.content_dir = missing;

        let err = configured_content(&cfg).unwrap_err();
        assert!(matches!(
            err,
            ContentDiscoveryError::MissingContentRoot { path } if path == "missing-content"
        ));
    }

    #[test]
    fn inventory_change_detection() {
        let dir = TempDir::new().unwrap();
        let content = dir.path().join("content");
        let doc = content.join("document");
        let nested = doc.join("assets");
        let tree_src = content.join("shared-assets");
        let exact_src = content.join("download.typ");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(&tree_src).unwrap();
        let index = doc.join("index.typ");
        fs::write(&index, "Document").unwrap();
        fs::write(tree_src.join("hidden.typ"), "asset").unwrap();
        fs::write(&exact_src, "asset").unwrap();

        let cfg = asset_config(
            dir.path(),
            &content,
            (&tree_src, "/shared"),
            (&exact_src, "/download.typ"),
        );
        let inv = configured_content(&cfg).unwrap();

        assert_eq!(inv.len(), 3);
        assert!(!inventory_may_change(
            &cfg,
            &inv,
            std::slice::from_ref(&index)
        ));
        assert!(inventory_may_change(&cfg, &inv, &[nested.join("new.typ")]));
        assert!(!inventory_may_change(
            &cfg,
            &inv,
            &[dir.path().join("outside.typ")]
        ));
        assert!(inventory_may_change(
            &cfg,
            &inv,
            &[tree_src.join("new.typ")]
        ));
        fs::remove_file(&exact_src).unwrap();
        assert!(inventory_may_change(&cfg, &inv, &[exact_src]));
        fs::remove_file(&index).unwrap();
        assert!(inventory_may_change(&cfg, &inv, &[index]));
    }

    #[test]
    fn scopes_resolve_from_root() {
        let dir = TempDir::new().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(content.join("posts")).unwrap();
        fs::create_dir_all(content.join("guides")).unwrap();
        fs::write(content.join("posts/hello.typ"), "").unwrap();
        fs::write(content.join("guides/start.typ"), "").unwrap();

        let mut cfg = crate::config::tests::load_test_config(dir.path(), "");
        cfg.build.content_dir = content;

        let discovered = configured_content(&cfg).unwrap();
        let scopes = ["posts/hello", "guides", "guides"].map(PathBuf::from);
        let selected = select_content_sources(&cfg, &scopes, &discovered).unwrap();
        let content = normalize_path(&cfg.build.content_dir);
        assert_eq!(
            selected,
            [
                content.join("guides/start.typ"),
                content.join("posts/hello.typ")
            ]
        );
    }

    #[test]
    fn same_stem_layouts_distinct() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("about")).unwrap();
        for src in ["about.typ", "about/index.typ", "index.typ"] {
            fs::write(dir.path().join(src), "").unwrap();
        }
        let units = content_units(dir.path()).unwrap();
        assert_eq!(
            units.iter().map(|u| u.id.to_string()).collect::<Vec<_>>(),
            ["about/index.typ", "about.typ", "index.typ"]
        );
    }

    #[test]
    fn entry_excluded_from_inventory() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        fs::create_dir(&content).unwrap();
        fs::write(content.join("site.typ"), "#document(\"index.html\")[Home]").unwrap();
        fs::write(content.join("document.typ"), "Document").unwrap();
        fs::create_dir(content.join("helpers")).unwrap();
        fs::write(content.join("helpers/card.typ"), "Card").unwrap();
        let cfg =
            crate::config::tests::load_test_config(root, "[build]\nentry = \"content/site.typ\"");
        let inv = configured_content(&cfg).unwrap();
        assert_eq!(
            inv.iter().map(|u| u.id.to_string()).collect::<Vec<_>>(),
            ["document.typ", "helpers/card.typ"]
        );
        let cancel = crate::cancellation::BuildCancellation::default();

        fs::remove_file(content.join("site.typ")).unwrap();
        let unopened =
            content_units_with_sources(&cfg, &inv, [content.join("site.typ")], &cancel).unwrap();

        assert_eq!(unopened, inv);
    }

    #[test]
    fn unsaved_source_joins_inventory() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        fs::create_dir(&content).unwrap();
        fs::write(content.join("document.typ"), "Document").unwrap();
        let mut cfg = crate::config::tests::load_test_config(root, "");
        cfg.build.content_dir = content.clone();
        let inv = configured_content(&cfg).unwrap();
        assert_eq!(
            inv.iter().map(|u| u.id.to_string()).collect::<Vec<_>>(),
            ["document.typ"]
        );

        let opened = content_units_with_sources(
            &cfg,
            &inv,
            [content.join("unsaved.typ")],
            &crate::cancellation::BuildCancellation::default(),
        )
        .unwrap();

        assert_eq!(
            opened.iter().map(|u| u.id.to_string()).collect::<Vec<_>>(),
            ["document.typ", "unsaved.typ"]
        );
    }

    #[test]
    fn scope_selects_matching_sources() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("b")).unwrap();
        for src in ["a.typ", "b.typ", "b/helper.typ", "b/index.typ"] {
            fs::write(dir.path().join(src), "").unwrap();
        }
        let discovered = content_units(dir.path()).unwrap();
        let mut cfg = crate::config::tests::load_test_config(dir.path(), "");
        let content = normalize_path(dir.path());
        cfg.build.content_dir = content.clone();
        for (scope, expected) in [
            ("a", vec!["a.typ"]),
            ("b", vec!["b/helper.typ", "b/index.typ"]),
            ("b.typ", vec!["b.typ"]),
        ] {
            let selected =
                select_content_sources(&cfg, &[PathBuf::from(scope)], &discovered).unwrap();
            assert_eq!(
                selected,
                expected.iter().map(|n| content.join(n)).collect::<Vec<_>>(),
                "{scope}"
            );
        }
    }
}
