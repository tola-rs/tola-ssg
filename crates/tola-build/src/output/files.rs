//! Materialization of validated output snapshots.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use rayon::prelude::*;

use crate::cancellation::{BuildCancellation, OptionalCancellation};
use crate::filesystem::TemporaryDirectory;

use super::graph::{OutputFile, OutputGraph};
use super::revision::OutputRevision;
use super::root::STAGING_DIRECTORY_PREFIX;

// Bound work between cancellation checks while writing one staged output.
const OUTPUT_IO_CHUNK_SIZE: usize = 64 * 1024;

/// A shared read-only filesystem view of one complete set of output bytes.
///
/// Clones retain the same directory. The last owner restores writable permissions
/// and removes it. Read-only permissions prevent accidental writes; consumers
/// are trusted to leave the view unchanged.
#[derive(Debug, Clone)]
pub struct HookOutputFiles {
    directory: Arc<TemporaryDirectory>,
}

/// A complete production tree staged in the site's output workspace.
///
/// Dropping this value before commit removes the private tree without
/// changing the currently published output.
pub(crate) struct StagedBuildFiles {
    directory: tempfile::TempDir,
    destination: PathBuf,
}

impl StagedBuildFiles {
    pub(crate) fn materialize(
        graph: &OutputGraph,
        destination: &super::OutputWrite<'_>,
        cancellation: &BuildCancellation,
    ) -> Result<Self> {
        cancellation.ensure_active()?;
        let destination_path = destination.path();
        let workspace = destination.prepare_workspace_for_staging()?;
        let directory = tempfile::Builder::new()
            .prefix(STAGING_DIRECTORY_PREFIX)
            .tempdir_in(workspace)
            .with_context(|| "Tola could not create the output directory; check that the site directory can be written")?;
        write_outputs(graph.outputs(), directory.path(), Some(cancellation))?;
        Ok(Self {
            directory,
            destination: destination_path.to_path_buf(),
        })
    }

    pub(crate) fn commit(self, destination: super::OutputWrite<'_>) -> Result<()> {
        anyhow::ensure!(
            destination.path() == self.destination,
            "Tola could not publish the build to a different output directory; run the build again"
        );
        destination.replace_with(self.directory)?;
        Ok(())
    }
}

impl HookOutputFiles {
    pub(crate) fn materialize_candidate(
        site_root: &Path,
        graph: &OutputGraph,
        cancellation: Option<&BuildCancellation>,
    ) -> Result<Self> {
        Self::materialize_outputs(site_root, graph.outputs(), cancellation)
    }

    /// Materialize a committed revision for a filesystem-based consumer.
    pub fn materialize_revision(
        site_root: &Path,
        revision: &OutputRevision,
        cancellation: &BuildCancellation,
    ) -> Result<Self> {
        Self::materialize_outputs(site_root, revision.outputs(), Some(cancellation))
    }

    pub(crate) fn materialize_outputs(
        site_root: &Path,
        outputs: &[OutputFile],
        cancellation: Option<&BuildCancellation>,
    ) -> Result<Self> {
        cancellation.ensure_active_if_present()?;
        let directory = TemporaryDirectory::create(
            &site_root
                .join(crate::filesystem::INTERNAL_DIR)
                .join("hook-candidates"),
            "candidate",
        )?;
        write_outputs(outputs, directory.path(), cancellation)?;
        directory.make_read_only(cancellation)?;
        cancellation.ensure_active_if_present()?;
        Ok(Self {
            directory: Arc::new(directory),
        })
    }

    /// Borrow the temporary root while at least one owner remains alive.
    pub fn root(&self) -> &Path {
        self.directory.path()
    }
}

fn write_outputs(
    outputs: &[OutputFile],
    root: &Path,
    cancellation: Option<&BuildCancellation>,
) -> Result<()> {
    cancellation.ensure_active_if_present()?;
    write_output_directories(outputs, root, cancellation)?;
    // Every output is independent: it names its own path and owns its own bytes, so the writes
    // run on the CPU pool instead of waiting on one file at a time. Staging is private until it
    // is committed, so a partially written tree is discarded rather than published. The first
    // failing output in output order decides the reported error, whatever order they finish in.
    let written = outputs
        .par_iter()
        .map(|output| write_output(output, root, cancellation))
        .collect::<Vec<_>>();
    for result in written {
        result?;
    }
    cancellation.ensure_active_if_present()?;
    Ok(())
}

/// Create every directory the staged outputs need, exactly once, in path order.
fn write_output_directories(
    outputs: &[OutputFile],
    root: &Path,
    cancellation: Option<&BuildCancellation>,
) -> Result<()> {
    let mut directories = BTreeSet::new();
    for output in outputs {
        if let Some(parent) = root.join(output.path().as_str()).parent() {
            directories.insert(parent.to_path_buf());
        }
    }
    for directory in directories {
        cancellation.ensure_active_if_present()?;
        fs::create_dir_all(&directory).with_context(|| {
            let relative = directory.strip_prefix(root).unwrap_or(&directory).display();
            format!("could not create the directory for `{relative}`")
        })?;
    }
    Ok(())
}

/// Write one output below `root`.
fn write_output(
    output: &OutputFile,
    root: &Path,
    cancellation: Option<&BuildCancellation>,
) -> Result<()> {
    cancellation.ensure_active_if_present()?;
    let path = root.join(output.path().as_str());
    let mut file = fs::File::create(&path).with_context(|| {
        format!(
            "could not create `{}` in the output directory",
            output.path()
        )
    })?;
    for bytes in output.bytes().chunks(OUTPUT_IO_CHUNK_SIZE) {
        cancellation.ensure_active_if_present()?;
        file.write_all(bytes)
            .with_context(|| format!("could not write `{}`", output.path()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::output::graph::OutputGraphBuilder;
    use crate::output::root::OwnedOutputRoot;
    use crate::output::semantics::OutputDeclaration;
    use crate::output::tests::{insert_output, output_file, output_graph, site_lock};

    /// A publication root with no protected inputs.
    fn output_root(site: &Path, output: &Path) -> OwnedOutputRoot {
        OwnedOutputRoot::resolve(site, output, std::iter::empty::<(&'static str, &Path)>()).unwrap()
    }

    fn output_write<'a>(
        root: &'a OwnedOutputRoot,
        lock: &'a crate::build::SiteBuildLock,
    ) -> crate::output::OutputWrite<'a> {
        root.write(lock, &BuildCancellation::default()).unwrap()
    }

    fn graph() -> OutputGraph {
        output_graph([output_file(
            "posts/rust/index.html",
            OutputDeclaration::html_document(),
            b"Rust",
        )])
    }

    #[test]
    fn output_view_is_read_only_until_dropped() {
        let site = tempfile::tempdir().unwrap();
        let view = HookOutputFiles::materialize_candidate(site.path(), &graph(), None).unwrap();
        let root = view.root().to_path_buf();
        assert_eq!(
            fs::read(root.join("posts/rust/index.html")).unwrap(),
            b"Rust"
        );
        assert!(
            fs::metadata(root.join("posts/rust/index.html"))
                .unwrap()
                .permissions()
                .readonly()
        );

        let retained = view.clone();
        assert_eq!(view.root(), retained.root());
        drop(view);
        assert!(root.is_dir());
        drop(retained);
        assert!(!root.exists());
    }

    #[test]
    fn cancelled_view_reports_cancellation() {
        let site = tempfile::tempdir().unwrap();

        let canceller = crate::cancellation::BuildCanceller::default();
        canceller.cancel();
        let error = HookOutputFiles::materialize_revision(
            site.path(),
            &OutputRevision::from_graph(&graph()),
            &canceller.token(),
        )
        .unwrap_err();
        assert!(
            error
                .downcast_ref::<crate::cancellation::BuildCancelled>()
                .is_some()
        );
    }

    #[test]
    fn staging_is_invisible_until_commit() {
        let site = tempfile::TempDir::new().unwrap();
        let output = site.path().join("public");
        let destination = output_root(site.path(), &output);
        let lock = site_lock(site.path());
        let staging = tempfile::Builder::new()
            .prefix("published-")
            .tempdir_in(site.path())
            .unwrap();
        fs::write(staging.path().join("index.html"), "published").unwrap();
        fs::write(staging.path().join("stale.txt"), "stale").unwrap();
        destination
            .write(&lock, &BuildCancellation::default())
            .unwrap()
            .replace_with(staging)
            .unwrap();

        let writer = output_write(&destination, &lock);
        let staged =
            StagedBuildFiles::materialize(&graph(), &writer, &BuildCancellation::default())
                .unwrap();

        assert_eq!(
            fs::read_to_string(output.join("index.html")).unwrap(),
            "published"
        );
        assert_eq!(
            fs::read(staged.directory.path().join("posts/rust/index.html")).unwrap(),
            b"Rust"
        );

        staged.commit(writer).unwrap();
        assert!(!output.join("index.html").exists());
        assert!(!output.join("stale.txt").exists());
        assert_eq!(
            fs::read(output.join("posts/rust/index.html")).unwrap(),
            b"Rust"
        );
        assert!(
            !destination
                .output_workspace()
                .unwrap()
                .join("previous")
                .exists()
        );
    }

    #[cfg(unix)]
    #[test]
    fn workspace_symlink_aborts_staging() {
        let site = tempfile::tempdir().unwrap();
        let output = site.path().join("public");
        let content = site.path().join("content");
        fs::create_dir(&content).unwrap();
        fs::write(content.join("page.typ"), "source").unwrap();
        let destination =
            OwnedOutputRoot::resolve(site.path(), &output, [("content", content.as_path())])
                .unwrap();
        let lock = site_lock(site.path());
        let writer = output_write(&destination, &lock);
        let workspace = crate::filesystem::publication_workspace(&output).unwrap();
        std::os::unix::fs::symlink(&content, &workspace).unwrap();

        let staged =
            StagedBuildFiles::materialize(&graph(), &writer, &BuildCancellation::default());
        assert!(staged.is_err());
        assert_eq!(fs::read(content.join("page.typ")).unwrap(), b"source");
        assert_eq!(fs::read_dir(&content).unwrap().count(), 1);
        assert!(!output.exists());
    }

    #[test]
    fn dropped_staging_keeps_published_output() {
        let site = tempfile::TempDir::new().unwrap();
        let output = site.path().join("public");
        let destination = output_root(site.path(), &output);
        fs::create_dir(&output).unwrap();
        let lock = site_lock(site.path());
        let writer = output_write(&destination, &lock);
        fs::write(output.join("index.html"), "published").unwrap();

        let staged =
            StagedBuildFiles::materialize(&graph(), &writer, &BuildCancellation::default())
                .unwrap();
        let staged_root = staged.directory.path().to_path_buf();
        drop(staged);

        assert!(!staged_root.exists());
        assert_eq!(
            fs::read_to_string(output.join("index.html")).unwrap(),
            "published"
        );
    }

    fn staged_tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
        let mut files = BTreeMap::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(&directory).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    let relative = path.strip_prefix(root).unwrap().to_owned();
                    files.insert(
                        relative.to_string_lossy().replace('\\', "/"),
                        fs::read(&path).unwrap(),
                    );
                }
            }
        }
        files
    }

    #[test]
    fn staging_writes_every_output() {
        let site = tempfile::TempDir::new().unwrap();
        let output = site.path().join("public");
        let destination = output_root(site.path(), &output);
        let lock = site_lock(site.path());
        let writer = output_write(&destination, &lock);

        let mut candidate = OutputGraphBuilder::new();
        for (path, bytes) in [
            ("index.html", "home"),
            ("docs/one/index.html", "one"),
            ("docs/two/index.html", "two changed"),
            ("guides/three/index.html", "three"),
        ] {
            insert_output(
                &mut candidate,
                path,
                OutputDeclaration::html_document(),
                bytes.as_bytes(),
            );
        }

        let staged = StagedBuildFiles::materialize(
            &candidate.finish(),
            &writer,
            &BuildCancellation::default(),
        )
        .unwrap();

        let expected = [
            ("docs/one/index.html", "one"),
            ("docs/two/index.html", "two changed"),
            ("guides/three/index.html", "three"),
            ("index.html", "home"),
        ]
        .into_iter()
        .map(|(path, bytes)| (path.to_owned(), bytes.as_bytes().to_vec()))
        .collect::<BTreeMap<_, _>>();
        assert_eq!(staged_tree(staged.directory.path()), expected);
    }
}
