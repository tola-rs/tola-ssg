//! Materialization of validated output revisions.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use rayon::prelude::*;

use crate::cancellation::{BuildCancellation, OptionalCancellation};
use crate::filesystem::TemporaryDirectory;
use crate::filesystem::layout;

/// Prose for one staged output operation, rendered exactly as the site author reads it.
#[derive(Debug, thiserror::Error)]
enum StagedOutputError {
    #[error("could not create the directory for `{relative}`")]
    Directory {
        relative: String,
        #[source]
        source: std::io::Error,
    },
    #[error("could not create `{path}` in the output directory")]
    Create {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("could not write `{path}`")]
    Write {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

use super::graph::{OutputFile, OutputGraph};
use super::revision::OutputRevision;

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

/// A complete production tree staged in the site's output swap workspace.
///
/// A tree that is never published stays in the workspace, which the next build's recovery
/// discards, so the currently published output is untouched.
pub(crate) struct StagedBuildFiles {
    destination: PathBuf,
}

impl StagedBuildFiles {
    pub(crate) fn materialize(
        graph: &OutputGraph,
        destination: &super::OutputWrite<'_>,
        cancellation: &BuildCancellation,
    ) -> Result<Self> {
        cancellation.ensure_active()?;
        let staged = destination.stage()?;
        write_outputs(
            graph.outputs(),
            &staged,
            Some(cancellation),
            destination.minify,
        )?;
        Ok(Self {
            destination: destination.path().to_path_buf(),
        })
    }

    pub(crate) fn commit(self, destination: super::OutputWrite<'_>) -> Result<()> {
        anyhow::ensure!(
            destination.path() == self.destination,
            "Tola could not publish the build to a different output directory; run the build again"
        );
        destination.publish()?;
        Ok(())
    }
}

impl HookOutputFiles {
    pub(crate) fn materialize_candidate(
        site_root: &Path,
        graph: &OutputGraph,
        cancellation: Option<&BuildCancellation>,
        languages: tola_minify::MinifiedLanguages,
    ) -> Result<Self> {
        Self::materialize_outputs(site_root, graph.outputs(), cancellation, languages)
    }

    /// Materialize a committed revision for a filesystem-based consumer.
    pub fn materialize_revision(
        site_root: &Path,
        revision: &OutputRevision,
        cancellation: &BuildCancellation,
        languages: tola_minify::MinifiedLanguages,
    ) -> Result<Self> {
        Self::materialize_outputs(site_root, revision.outputs(), Some(cancellation), languages)
    }

    pub(crate) fn materialize_outputs(
        site_root: &Path,
        outputs: &[OutputFile],
        cancellation: Option<&BuildCancellation>,
        languages: tola_minify::MinifiedLanguages,
    ) -> Result<Self> {
        cancellation.ensure_active_if_present()?;
        let directory =
            TemporaryDirectory::create(&layout::hook_candidates(site_root), "candidate")?;
        write_outputs(outputs, directory.path(), cancellation, languages)?;
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
    languages: tola_minify::MinifiedLanguages,
) -> Result<()> {
    cancellation.ensure_active_if_present()?;
    write_output_directories(outputs, root, cancellation)?;
    // Every output is independent: it names its own path and owns its own bytes, so the writes
    // run on the CPU pool instead of waiting on one file at a time. Staging is private until it
    // is committed, so a partially written tree is discarded rather than published. The first
    // failing output in output order decides the reported error, whatever order they finish in.
    let written = outputs
        .par_iter()
        .map(|output| write_output(output, root, cancellation, languages))
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
        fs::create_dir_all(&directory).map_err(|source| StagedOutputError::Directory {
            relative: directory
                .strip_prefix(root)
                .unwrap_or(&directory)
                .display()
                .to_string(),
            source,
        })?;
    }
    Ok(())
}

/// Write one output below `root`.
fn write_output(
    output: &OutputFile,
    root: &Path,
    cancellation: Option<&BuildCancellation>,
    languages: tola_minify::MinifiedLanguages,
) -> Result<()> {
    cancellation.ensure_active_if_present()?;
    let bytes = minified_output_bytes(output, languages);
    let path = root.join(output.path().as_str());
    let mut file = fs::File::create(&path).map_err(|source| StagedOutputError::Create {
        path: output.path().to_string(),
        source,
    })?;
    for bytes in bytes.chunks(OUTPUT_IO_CHUNK_SIZE) {
        cancellation.ensure_active_if_present()?;
        file.write_all(bytes)
            .map_err(|source| StagedOutputError::Write {
                path: output.path().to_string(),
                source,
            })?;
    }
    Ok(())
}

/// The bytes one output publishes: Bundle CSS/JavaScript is minified, everything else is
/// published as produced. A source that cannot be read as UTF-8 or rewritten keeps its bytes.
fn minified_output_bytes<'a>(
    output: &'a OutputFile,
    languages: tola_minify::MinifiedLanguages,
) -> Cow<'a, [u8]> {
    if !matches!(output.owner(), super::owner::OutputOwner::Bundle { .. }) {
        return Cow::Borrowed(output.bytes());
    }
    let Some(request) =
        tola_minify::MinifyRequest::for_source(Path::new(output.path().as_str()), languages)
    else {
        return Cow::Borrowed(output.bytes());
    };
    let Ok(source) = std::str::from_utf8(output.bytes()) else {
        return Cow::Borrowed(output.bytes());
    };
    match request.minify(source) {
        Ok(minified) => Cow::Owned(minified.into_bytes()),
        Err(_) => Cow::Borrowed(output.bytes()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::graph::OutputGraphBuilder;
    use crate::output::owner::OutputOwner;
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

    fn staged_stylesheet(owner: OutputOwner, source: &str) -> Vec<u8> {
        let site = tempfile::TempDir::new().unwrap();
        let output = site.path().join("public");
        let destination = output_root(site.path(), &output);
        let lock = site_lock(site.path());
        let mut writer = output_write(&destination, &lock);
        writer.minify = tola_minify::MinifiedLanguages::new(true, false);
        let stage = writer.stage().unwrap();
        let stylesheet = output_file(
            "assets/app.css",
            OutputDeclaration::from_filesystem_source(Path::new("assets/app.css")),
            source.as_bytes(),
        )
        .with_owner(owner);
        StagedBuildFiles::materialize(
            &output_graph([stylesheet]),
            &writer,
            &BuildCancellation::default(),
        )
        .unwrap();
        fs::read(stage.join("assets/app.css")).unwrap()
    }

    fn graph() -> OutputGraph {
        output_graph([output_file(
            "posts/rust/index.html",
            OutputDeclaration::html_document(),
            b"Rust",
        )])
    }

    #[test]
    fn output_read_only_until_dropped() {
        let site = tempfile::tempdir().unwrap();
        let view = HookOutputFiles::materialize_candidate(
            site.path(),
            &graph(),
            None,
            tola_minify::MinifiedLanguages::default(),
        )
        .unwrap();
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
            tola_minify::MinifiedLanguages::default(),
        )
        .unwrap_err();
        assert!(
            error
                .downcast_ref::<crate::cancellation::BuildCancelled>()
                .is_some()
        );
    }

    /// Writing a candidate into the workspace leaves the published tree as it was.
    #[test]
    fn staging_published_tree_until_commit() {
        let site = tempfile::TempDir::new().unwrap();
        let output = site.path().join("public");
        let destination = output_root(site.path(), &output);
        let lock = site_lock(site.path());
        let published = output_write(&destination, &lock);
        let previous = published.stage().unwrap();
        fs::write(previous.join("index.html"), "published").unwrap();
        published.publish().unwrap();

        let writer = output_write(&destination, &lock);
        let stage = writer.stage().unwrap();
        let staged =
            StagedBuildFiles::materialize(&graph(), &writer, &BuildCancellation::default())
                .unwrap();

        assert_eq!(
            fs::read_to_string(output.join("index.html")).unwrap(),
            "published"
        );
        assert_eq!(
            fs::read(stage.join("posts/rust/index.html")).unwrap(),
            b"Rust"
        );

        staged.commit(writer).unwrap();
        assert!(!output.join("index.html").exists());
        assert_eq!(
            fs::read(output.join("posts/rust/index.html")).unwrap(),
            b"Rust"
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
        let workspace = crate::filesystem::swap_workspace(&output).unwrap();
        std::os::unix::fs::symlink(&content, &workspace).unwrap();

        let staged =
            StagedBuildFiles::materialize(&graph(), &writer, &BuildCancellation::default());
        assert!(staged.is_err());
        assert_eq!(fs::read(content.join("page.typ")).unwrap(), b"source");
        assert_eq!(fs::read_dir(&content).unwrap().count(), 1);
        assert!(!output.exists());
    }

    #[test]
    fn staging_writes_every_output() {
        let site = tempfile::TempDir::new().unwrap();
        let output = site.path().join("public");
        let destination = output_root(site.path(), &output);
        let lock = site_lock(site.path());
        let writer = output_write(&destination, &lock);
        let stage = writer.stage().unwrap();

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

        StagedBuildFiles::materialize(&candidate.finish(), &writer, &BuildCancellation::default())
            .unwrap();

        for (path, bytes) in [
            ("docs/one/index.html", "one"),
            ("docs/two/index.html", "two changed"),
            ("guides/three/index.html", "three"),
            ("index.html", "home"),
        ] {
            assert_eq!(
                fs::read(stage.join(path)).unwrap(),
                bytes.as_bytes(),
                "{path}"
            );
        }
        assert!(!output.exists());
    }

    #[test]
    fn bundle_stylesheet_is_minified() {
        let staged = staged_stylesheet(
            OutputOwner::bundle("site.typ"),
            "/*! License */\n.a { color: red; }",
        );
        let staged = std::str::from_utf8(&staged).unwrap();
        assert!(staged.starts_with("/*! License */"), "{staged:?}");
        assert!(staged.contains(".a{color:red}"), "{staged:?}");
    }

    #[test]
    fn non_bundle_stylesheet_keeps_its_bytes() {
        let source = ".a { color: red; }";
        for owner in [
            OutputOwner::command(0, "stylesheets"),
            OutputOwner::configured_asset("assets/app.css"),
        ] {
            assert_eq!(staged_stylesheet(owner, source), source.as_bytes());
        }
    }
}
