//! Discovery and identity for Typst content written by the site author.
//!
//! Discovery selects the `.typ` inputs of the content tree, nothing more. A file beside a source
//! is an ordinary Typst input the source may read by lexical path, not a resource discovery
//! owns. A source may contribute to several documents, so proximity or an `index.typ` layout
//! cannot choose a resource's public URL or directory owner.

mod slug;
mod sources;

pub use sources::{ContentSource, SourceDescriptorField, SourceDescriptorFieldKind, SourceSet};
pub(crate) use sources::{SourceFileInput, SourceMetadataDeclaration, SourceRecords, SourcesInput};

use anyhow::Result;
use std::collections::{BTreeSet, HashSet};
use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::filesystem::normalize_path;

/// Complete content-root-relative filename identity for a content source.
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
        f.write_str(&crate::filesystem::render_relative(self.as_path()))
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
    config: &crate::config::ResolvedSiteConfig,
    scopes: &[PathBuf],
    units: &[ContentUnit],
) -> Result<Vec<PathBuf>> {
    if scopes.is_empty() {
        return Ok(units.iter().map(|unit| unit.source.clone()).collect());
    }

    let mut files = select_content_units_in_scope_for_root(
        config.get_root(),
        &config.build.content_dir,
        scopes,
        units,
    )?
    .into_iter()
    .map(|unit| unit.source.clone())
    .collect::<Vec<_>>();
    files.sort();
    files.dedup();
    Ok(files)
}

pub(crate) fn discover_content_units_for_config_with_cancellation(
    config: &crate::config::ResolvedSiteConfig,
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<Vec<ContentUnit>> {
    crate::resources::source_boundary(config, crate::InputScope::Online)
        .check(&config.build.content_dir)?;
    discover_from_root(
        &config.build.content_dir,
        config.get_root(),
        &config.build.entry,
        cancellation,
    )
}

/// Whether accepted filesystem changes can alter a published content inventory.
///
/// Known source-file edits preserve the inventory, while deletion or an unknown
/// path beneath the content root requires rediscovery.
pub(crate) fn inventory_may_change(
    config: &crate::config::ResolvedSiteConfig,
    inventory: &[ContentUnit],
    changed_paths: &[PathBuf],
) -> bool {
    if changed_paths.is_empty() {
        return false;
    }
    let content_root = crate::filesystem::normalize_existing_prefix(&config.build.content_dir);
    let known_sources = inventory
        .iter()
        .map(|unit| unit.source.as_path())
        .collect::<std::collections::HashSet<_>>();
    changed_paths.iter().any(|changed| {
        let changed = crate::filesystem::normalize_existing_prefix(changed);
        if !changed.starts_with(&content_root) && !content_root.starts_with(&changed) {
            return false;
        }

        if known_sources.contains(changed.as_path()) && changed.is_file() {
            return false;
        }

        true
    })
}

fn select_content_units_in_scope_for_root<'units>(
    site_root: &Path,
    content_root: &Path,
    scopes: &[PathBuf],
    all: &'units [ContentUnit],
) -> Result<Vec<&'units ContentUnit>> {
    let site_root = normalize_path(site_root);
    let mut selected = Vec::new();
    for scope in scopes {
        let mut candidates = BTreeSet::new();
        if scope.is_absolute() {
            candidates.insert(normalize_path(scope));
        } else {
            if scope.exists() {
                candidates.insert(normalize_path(scope));
            }
            candidates.insert(normalize_path(&site_root.join(scope)));
            candidates.insert(normalize_path(&content_root.join(scope)));
        }

        let selected_before = selected.len();
        for resolved in candidates {
            if resolved.is_file() {
                selected.extend(all.iter().filter(|unit| unit.source == resolved));
                continue;
            }
            if resolved.is_dir() {
                selected.extend(all.iter().filter(|unit| unit.source.starts_with(&resolved)));
                continue;
            }

            let source_candidates = [resolved.with_extension("typ"), resolved.join("index.typ")]
                .map(|candidate| normalize_path(&candidate));
            selected.extend(
                all.iter()
                    .filter(|unit| source_candidates.contains(&unit.source)),
            );
        }

        if selected.len() == selected_before {
            anyhow::bail!(
                "no discovered Typst source matches `{}`; run `tola inspect sources` to list them",
                crate::filesystem::display_path(scope, &site_root)
            );
        }
    }
    selected.sort_by(|a, b| a.id.cmp(&b.id));
    selected.dedup_by(|a, b| a.source == b.source);
    Ok(selected)
}

fn discover_from_root(
    content_root: &Path,
    display_root: &Path,
    entry: &Path,
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<Vec<ContentUnit>> {
    cancellation.ensure_active().map_err(anyhow::Error::new)?;
    if !content_root.is_dir() {
        anyhow::bail!(
            "content root `{}` does not exist; create it or set `build.content-dir` in `tola.toml`",
            crate::filesystem::display_path(content_root, display_root)
        );
    }
    // The walk resolves the root once. Every entry it reaches lies beneath that canonical root and
    // holds no symbolic link — a link is neither descended nor collected — so each walked path is
    // already the canonical spelling the identity rules would resolve it to.
    let root = crate::filesystem::normalize_existing_prefix(content_root);
    let entry = crate::filesystem::normalize_existing_prefix(entry);
    let mut sources = Vec::new();
    collect_typ_sources(&root, display_root, cancellation, &mut sources)?;
    let mut units = Vec::with_capacity(sources.len());
    for source in sources {
        cancellation.ensure_active().map_err(anyhow::Error::new)?;
        if source != entry && has_typ_extension(&source) {
            units.push(unit_for(&root, source));
        }
    }
    units.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(units)
}

/// Collect every `.typ` source beneath `directory`.
///
/// Discovery walks on its calling thread: a directory read costs little beside the work
/// the sources it finds then need, and a walk that borrowed a shared worker pool would
/// make discovery depend on how that pool happens to be occupied.
fn collect_typ_sources(
    directory: &Path,
    display_root: &Path,
    cancellation: &crate::cancellation::BuildCancellation,
    sources: &mut Vec<PathBuf>,
) -> Result<()> {
    cancellation.ensure_active().map_err(anyhow::Error::new)?;
    let entries =
        crate::filesystem::read_sorted_entries(directory, cancellation, display_root, "content")?;
    for entry in entries {
        cancellation.ensure_active().map_err(anyhow::Error::new)?;
        let file_name = entry.file_name();
        if content_name_is_hidden(&file_name) {
            continue;
        }
        let file_type = entry.file_type().map_err(|error| {
            anyhow::anyhow!(
                "cannot inspect content entry in `{}`: {}",
                crate::filesystem::display_path(directory, display_root),
                crate::filesystem::path_failure_reason(&error),
            )
        })?;
        let path = entry.path();
        // A symbolic link is neither descended nor collected: `file_type` does not
        // follow it, so it is neither a directory nor a regular file here.
        if file_type.is_dir() {
            collect_typ_sources(&path, display_root, cancellation, sources)?;
        } else if file_type.is_file() && has_typ_extension(&path) {
            sources.push(path);
        }
    }
    Ok(())
}

/// Whether a content entry is a name the author keeps out of discovery.
///
/// A name that is not valid UTF-8 has no leading dot to match.
fn content_name_is_hidden(file_name: &OsStr) -> bool {
    file_name.to_str().is_some_and(|name| name.starts_with('.'))
}

/// Complete a discovered inventory with the sources an editor holds unsaved.
///
/// The discovered units are already canonical — discovery normalizes the content root once and
/// walks beneath it — so only the opened paths go through the identity rules again. Every check of
/// a site re-reads this, and normalizing a site's whole source list to re-derive units discovery
/// already produced costs one filesystem resolution per source.
pub(crate) fn content_units_with_sources(
    config: &crate::config::ResolvedSiteConfig,
    inventory: &[ContentUnit],
    opened: impl IntoIterator<Item = PathBuf>,
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<Vec<ContentUnit>> {
    let root = crate::filesystem::normalize_existing_prefix(&config.build.content_dir);
    let entry = crate::filesystem::normalize_existing_prefix(&config.build.entry);
    let mut units = inventory.to_vec();
    let mut known = units
        .iter()
        .map(|unit| unit.source.clone())
        .collect::<HashSet<_>>();
    for source in opened {
        cancellation.ensure_active().map_err(anyhow::Error::new)?;
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

/// The content unit one source names, when membership admits it.
///
/// Membership is a normalized source outside the entry, inside the content root, with the
/// extension the walk collects.
fn eligible_unit(root: &Path, entry: &Path, source: &Path) -> Option<ContentUnit> {
    let source = crate::filesystem::normalize_existing_prefix(source);
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

/// Whether a name has the content source extension: exactly lowercase `.typ`.
///
/// The extension alone never decides eligibility. Discovery still requires a path the directory
/// walk reaches under the configured content root, and it skips hidden names, symbolic links, and
/// the entry. Callers use this to tell a name discovery would read from one it would never read;
/// only a build confirms what a configuration actually selects.
pub fn has_typ_extension(path: &Path) -> bool {
    path.extension() == Some(OsStr::new("typ"))
}

fn is_index_source(path: &Path) -> bool {
    path.file_stem().and_then(|stem| stem.to_str()) == Some("index")
}

#[cfg(test)]
mod tests {
    fn content_units(root: &std::path::Path) -> anyhow::Result<Vec<super::ContentUnit>> {
        super::discover_from_root(
            root,
            root,
            &root.join("site.typ"),
            &crate::cancellation::BuildCancellation::default(),
        )
    }

    fn configured_content(
        config: &crate::config::ResolvedSiteConfig,
    ) -> anyhow::Result<Vec<super::ContentUnit>> {
        super::discover_content_units_for_config_with_cancellation(
            config,
            &crate::cancellation::BuildCancellation::default(),
        )
    }

    /// A config whose content root is `content`, declaring one asset tree and one asset file.
    fn asset_config(
        root: &std::path::Path,
        content: &std::path::Path,
        tree: (&std::path::Path, &str),
        file: (&std::path::Path, &str),
    ) -> crate::config::ResolvedSiteConfig {
        use crate::config::section::assets::{
            AssetFileDeclaration, AssetTreeDeclaration, AssetUrl, AssetUrlPrefix,
        };

        let mut config = crate::config::tests::load_test_config(root, "");
        config.build.content_dir = content.to_path_buf();
        config.assets.trees = vec![AssetTreeDeclaration::new(
            tree.0,
            AssetUrlPrefix::parse(tree.1).unwrap(),
        )];
        config.assets.files = vec![AssetFileDeclaration::new(
            file.0,
            AssetUrl::parse(file.1).unwrap(),
        )];
        config
    }

    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn content_identity_keeps_spelling() {
        let path = PathBuf::from("posts").join("Cafe\u{301}.typ");
        assert_eq!(ContentId::new(path).to_string(), "posts/Cafe\u{301}.typ");
    }

    #[test]
    fn only_lowercase_typ_extension_counts() {
        assert!(has_typ_extension(Path::new("post.typ")));
        assert!(!has_typ_extension(Path::new("post.TYP")));
        assert!(!has_typ_extension(Path::new("image.png")));
        assert!(!has_typ_extension(Path::new("no-extension")));
    }

    #[test]
    fn discovery_needs_no_second_pool_worker() {
        // Discovery must not need a worker from the pool that called it: a walk that
        // waited on one would find none free and report no content at all.
        let directory = TempDir::new().unwrap();
        fs::create_dir_all(directory.path().join("posts")).unwrap();
        fs::write(directory.path().join("index.typ"), "").unwrap();
        fs::write(directory.path().join("posts/hello.typ"), "").unwrap();

        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        let discovered = pool.install(|| content_units(directory.path()));

        assert_eq!(
            discovered
                .unwrap()
                .iter()
                .map(|unit| unit.id.to_string())
                .collect::<Vec<_>>(),
            ["index.typ", "posts/hello.typ"]
        );
    }

    #[test]
    fn cancelled_build_skips_discovery() {
        let directory = TempDir::new().unwrap();
        let canceller = crate::cancellation::BuildCanceller::new();
        canceller.cancel();

        let error = discover_from_root(
            directory.path(),
            directory.path(),
            &directory.path().join("site.typ"),
            &canceller.token(),
        )
        .unwrap_err();

        assert!(error.chain().any(|cause| matches!(
            cause.downcast_ref::<crate::cancellation::BuildCancelled>(),
            Some(crate::cancellation::BuildCancelled)
        )));
    }

    #[test]
    fn discovery_finds_nested_sources() {
        let d = TempDir::new().unwrap();
        fs::create_dir_all(d.path().join("posts/rust")).unwrap();
        for source in [
            "posts/a.typ",
            "posts/index.typ",
            "posts/helper.typ",
            "posts/rust/index.typ",
            "posts/rust/helper.typ",
        ] {
            fs::write(d.path().join(source), "").unwrap();
        }

        let units = content_units(d.path()).unwrap();

        assert_eq!(
            units
                .iter()
                .map(|unit| unit.id.to_string())
                .collect::<Vec<_>>(),
            vec![
                "posts/a.typ",
                "posts/helper.typ",
                "posts/index.typ",
                "posts/rust/helper.typ",
                "posts/rust/index.typ"
            ]
        );
        assert!(units.iter().any(|unit| {
            unit.id.as_path() == Path::new("posts/rust/index.typ")
                && unit.layout == ContentSourceLayout::DirectoryIndex
        }));
        assert!(units.iter().any(|unit| {
            unit.id.as_path() == Path::new("posts/rust/helper.typ")
                && unit.layout == ContentSourceLayout::SingleFile
        }));
    }

    #[test]
    fn asset_declarations_keep_every_source() {
        let directory = TempDir::new().unwrap();
        let content = directory.path().join("content");
        fs::create_dir_all(content.join("assets")).unwrap();
        fs::create_dir_all(content.join("downloads")).unwrap();
        fs::write(content.join("index.typ"), "").unwrap();
        fs::write(content.join("visible.typ"), "").unwrap();
        fs::write(content.join("assets/nested.typ"), "").unwrap();
        fs::write(content.join("downloads/tree-member.typ"), "").unwrap();
        fs::write(content.join("exact-file.typ"), "").unwrap();

        let config = asset_config(
            directory.path(),
            &content,
            (&content.join("downloads"), "/downloads"),
            (&content.join("exact-file.typ"), "/exact-file.typ"),
        );

        let units = configured_content(&config).unwrap();

        assert_eq!(
            units
                .iter()
                .map(|unit| unit.id.to_string())
                .collect::<Vec<_>>(),
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
    fn configured_root_scopes_discovery() {
        let directory = TempDir::new().unwrap();
        let content = directory.path().join("content");
        fs::create_dir_all(content.join("posts")).unwrap();
        fs::write(content.join("posts/hello.typ"), "").unwrap();

        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        config.build.content_dir = content.clone();

        let units = configured_content(&config).unwrap();

        assert_eq!(units.len(), 1);
        assert_eq!(units[0].id.as_path(), Path::new("posts/hello.typ"));
        assert_eq!(units[0].root, normalize_path(&content));
    }

    #[test]
    fn missing_content_root_is_reported() {
        let directory = TempDir::new().unwrap();
        let missing = directory.path().join("missing-content");
        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        config.build.content_dir = missing.clone();

        let error = configured_content(&config).unwrap_err();
        let rendered = error.to_string();
        assert!(rendered.contains("does not exist"));
        assert!(rendered.contains("missing-content"));
        assert!(!rendered.contains(directory.path().to_string_lossy().as_ref()));
    }

    #[test]
    fn known_source_edits_keep_the_inventory() {
        let directory = TempDir::new().unwrap();
        let content = directory.path().join("content");
        let document = content.join("document");
        let nested = document.join("assets");
        let tree_source = content.join("shared-assets");
        let exact_source = content.join("download.typ");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(&tree_source).unwrap();
        let index = document.join("index.typ");
        fs::write(&index, "Document").unwrap();
        fs::write(tree_source.join("hidden.typ"), "asset").unwrap();
        fs::write(&exact_source, "asset").unwrap();

        let config = asset_config(
            directory.path(),
            &content,
            (&tree_source, "/shared"),
            (&exact_source, "/download.typ"),
        );
        let inventory = configured_content(&config).unwrap();

        assert_eq!(inventory.len(), 3);
        assert!(!inventory_may_change(
            &config,
            &inventory,
            std::slice::from_ref(&index),
        ));
        assert!(inventory_may_change(
            &config,
            &inventory,
            &[nested.join("new.typ")],
        ));
        assert!(!inventory_may_change(
            &config,
            &inventory,
            &[directory.path().join("outside.typ")],
        ));

        assert!(inventory_may_change(
            &config,
            &inventory,
            &[tree_source.join("new.typ")],
        ));
        fs::remove_file(&exact_source).unwrap();
        assert!(inventory_may_change(&config, &inventory, &[exact_source]));
        fs::remove_file(&index).unwrap();
        assert!(inventory_may_change(&config, &inventory, &[index]));
    }

    #[test]
    fn scopes_resolve_from_the_content_root() {
        let directory = TempDir::new().unwrap();
        let content = directory.path().join("content");
        fs::create_dir_all(content.join("posts")).unwrap();
        fs::create_dir_all(content.join("guides")).unwrap();
        fs::write(content.join("posts/hello.typ"), "").unwrap();
        fs::write(content.join("guides/start.typ"), "").unwrap();

        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        config.build.content_dir = content;

        let discovered = configured_content(&config).unwrap();
        let units = select_content_units_in_scope_for_root(
            config.get_root(),
            &config.build.content_dir,
            &[PathBuf::from("posts/hello"), PathBuf::from("guides")],
            &discovered,
        )
        .unwrap();

        assert_eq!(
            units
                .iter()
                .map(|unit| unit.id.to_string())
                .collect::<Vec<_>>(),
            vec!["guides/start.typ", "posts/hello.typ"]
        );
    }

    #[test]
    fn same_stem_layouts_stay_distinct() {
        let directory = TempDir::new().unwrap();
        fs::create_dir_all(directory.path().join("about")).unwrap();
        for source in ["about.typ", "about/index.typ", "index.typ"] {
            fs::write(directory.path().join(source), "").unwrap();
        }
        let units = content_units(directory.path()).unwrap();
        assert_eq!(
            units
                .iter()
                .map(|unit| unit.id.to_string())
                .collect::<Vec<_>>(),
            ["about/index.typ", "about.typ", "index.typ"]
        );
    }

    #[test]
    fn build_entry_never_joins_the_inventory() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        let content = root.join("content");
        fs::create_dir(&content).unwrap();
        fs::write(content.join("site.typ"), "#document(\"index.html\")[Home]").unwrap();
        fs::write(content.join("document.typ"), "Document").unwrap();
        fs::create_dir(content.join("helpers")).unwrap();
        fs::write(content.join("helpers/card.typ"), "Card").unwrap();
        let config =
            crate::config::tests::load_test_config(root, "[build]\nentry = \"content/site.typ\"");
        let inventory = configured_content(&config).unwrap();
        assert_eq!(
            inventory
                .iter()
                .map(|unit| unit.id.to_string())
                .collect::<Vec<_>>(),
            ["document.typ", "helpers/card.typ"]
        );
        let cancellation = crate::cancellation::BuildCancellation::default();

        fs::remove_file(content.join("site.typ")).unwrap();
        let unopened = content_units_with_sources(
            &config,
            &inventory,
            [content.join("site.typ")],
            &cancellation,
        )
        .unwrap();

        assert_eq!(unopened, inventory);
    }

    #[test]
    fn unsaved_source_joins_the_inventory() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        let content = root.join("content");
        fs::create_dir(&content).unwrap();
        fs::write(content.join("document.typ"), "Document").unwrap();
        let mut config = crate::config::tests::load_test_config(root, "");
        config.build.content_dir = content.clone();
        let inventory = configured_content(&config).unwrap();
        assert_eq!(
            inventory
                .iter()
                .map(|unit| unit.id.to_string())
                .collect::<Vec<_>>(),
            ["document.typ"]
        );

        let opened = content_units_with_sources(
            &config,
            &inventory,
            [content.join("unsaved.typ")],
            &crate::cancellation::BuildCancellation::default(),
        )
        .unwrap();

        assert_eq!(
            opened
                .iter()
                .map(|unit| unit.id.to_string())
                .collect::<Vec<_>>(),
            ["document.typ", "unsaved.typ"]
        );
    }

    #[test]
    fn each_scope_form_selects_its_sources() {
        let directory = TempDir::new().unwrap();
        fs::create_dir_all(directory.path().join("b")).unwrap();
        for source in ["a.typ", "b.typ", "b/helper.typ", "b/index.typ"] {
            fs::write(directory.path().join(source), "").unwrap();
        }
        let discovered = content_units(directory.path()).unwrap();
        for (scope, expected) in [
            ("a", vec!["a.typ"]),
            ("b", vec!["b/helper.typ", "b/index.typ"]),
            ("b.typ", vec!["b.typ"]),
        ] {
            let selected = select_content_units_in_scope_for_root(
                directory.path(),
                directory.path(),
                &[PathBuf::from(scope)],
                &discovered,
            )
            .unwrap();
            assert_eq!(
                selected
                    .iter()
                    .map(|unit| unit.id.to_string())
                    .collect::<Vec<_>>(),
                expected,
                "{scope}"
            );
        }
    }
}
