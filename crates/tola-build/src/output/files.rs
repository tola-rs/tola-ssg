//! Materialization of validated output revisions.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use rayon::prelude::*;

use super::graph::{OutputFile, OutputGraph};
use super::revision::OutputRevision;
use crate::cancellation::{BuildCancellation, OptionalCancellation};
use crate::filesystem::TemporaryDirectory;
use tola_input::layout;

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
        write_outputs(graph.outputs(), &staged, Some(cancellation))?;
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
        let directory =
            TemporaryDirectory::create(&layout::hook_candidates(site_root), "candidate")?;
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

/// Create the distinct parent directories in path order.
fn write_output_directories(
    outputs: &[OutputFile],
    root: &Path,
    cancellation: Option<&BuildCancellation>,
) -> Result<()> {
    let directories: BTreeSet<_> = outputs
        .iter()
        .filter_map(|output| Path::new(output.path().as_str()).parent())
        .filter(|directory| !directory.as_os_str().is_empty())
        .collect();
    for directory in directories {
        cancellation.ensure_active_if_present()?;
        fs::create_dir_all(root.join(directory)).map_err(|source| {
            StagedOutputError::Directory {
                relative: directory.display().to_string(),
                source,
            }
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
    let mut file = fs::File::create(&path).map_err(|source| StagedOutputError::Create {
        path: output.path().to_string(),
        source,
    })?;
    for bytes in output.bytes().chunks(OUTPUT_IO_CHUNK_SIZE) {
        cancellation.ensure_active_if_present()?;
        file.write_all(bytes)
            .map_err(|source| StagedOutputError::Write {
                path: output.path().to_string(),
                source,
            })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::owner::OutputOwner;
    use crate::output::root::OwnedOutputRoot;
    use crate::output::semantics::OutputDeclaration;
    use crate::output::tests::{output_file, output_graph, site_lock};

    fn output_root(site: &Path, out: &Path) -> OwnedOutputRoot {
        OwnedOutputRoot::resolve(site, out, std::iter::empty::<(&'static str, &Path)>()).unwrap()
    }

    fn output_write<'a>(
        root: &'a OwnedOutputRoot,
        lock: &'a crate::build::SiteBuildLock,
    ) -> crate::output::OutputWrite<'a> {
        root.write(lock, &BuildCancellation::default()).unwrap()
    }

    fn published_stylesheet(graph: &OutputGraph) -> Vec<u8> {
        let site = tempfile::TempDir::new().unwrap();
        let out = site.path().join("public");
        let dst = output_root(site.path(), &out);
        let lock = site_lock(site.path());
        let writer = output_write(&dst, &lock);
        StagedBuildFiles::materialize(graph, &writer, &BuildCancellation::default())
            .unwrap()
            .commit(writer)
            .unwrap();
        fs::read(out.join("assets/app.css")).unwrap()
    }

    fn graph() -> OutputGraph {
        output_graph([output_file(
            "posts/rust/index.html",
            OutputDeclaration::html_document(),
            b"Rust",
        )])
    }

    #[test]
    fn output_readonly_until_dropped() {
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
    fn cancelled_view_cancelled_err() {
        let site = tempfile::tempdir().unwrap();
        let canceller = crate::cancellation::BuildCanceller::default();
        canceller.cancel();
        let err = HookOutputFiles::materialize_revision(
            site.path(),
            &OutputRevision::from_graph(&graph()),
            &canceller.token(),
        )
        .unwrap_err();
        assert!(
            err.downcast_ref::<crate::cancellation::BuildCancelled>()
                .is_some()
        );
    }

    #[test]
    fn staging_atomic_publish() {
        let site = tempfile::TempDir::new().unwrap();
        let out = site.path().join("public");
        let dst = output_root(site.path(), &out);
        let lock = site_lock(site.path());
        let published = output_write(&dst, &lock);
        let prev = published.stage().unwrap();
        fs::write(prev.join("index.html"), "published").unwrap();
        published.publish().unwrap();

        let writer = output_write(&dst, &lock);
        let files = [
            ("posts/rust/index.html", "Rust"),
            ("posts/typst/index.html", "Typst"),
            ("guide/index.html", "Guide"),
        ];
        let graph = output_graph(files.map(|(path, text)| {
            output_file(path, OutputDeclaration::html_document(), text.as_bytes())
        }));
        let staged =
            StagedBuildFiles::materialize(&graph, &writer, &BuildCancellation::default()).unwrap();

        assert_eq!(fs::read(out.join("index.html")).unwrap(), b"published");
        assert!(!out.join("posts").exists());

        staged.commit(writer).unwrap();
        assert!(!out.join("index.html").exists());
        for (path, text) in files {
            assert_eq!(fs::read(out.join(path)).unwrap(), text.as_bytes(), "{path}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn workspace_symlink_aborts_staging() {
        let site = tempfile::tempdir().unwrap();
        let out = site.path().join("public");
        let content = site.path().join("content");
        fs::create_dir(&content).unwrap();
        fs::write(content.join("page.typ"), "source").unwrap();
        let dst =
            OwnedOutputRoot::resolve(site.path(), &out, [("content", content.as_path())]).unwrap();
        let lock = site_lock(site.path());
        let writer = output_write(&dst, &lock);
        let workspace = crate::filesystem::swap_workspace(&out).unwrap();
        std::os::unix::fs::symlink(&content, &workspace).unwrap();

        let staged =
            StagedBuildFiles::materialize(&graph(), &writer, &BuildCancellation::default());
        assert!(staged.is_err());
        assert_eq!(fs::read(content.join("page.typ")).unwrap(), b"source");
        assert_eq!(fs::read_dir(&content).unwrap().count(), 1);
        assert!(!out.exists());
    }

    #[test]
    fn bundle_stylesheet_digest_and_views() {
        let site = tempfile::TempDir::new().unwrap();
        let path = "assets/app.css";
        let src = b"/*! License */\n.a { color: red; }";
        let entry = tola_typst::BundleEntry::new(
            typst::syntax::VirtualPath::new(path).unwrap(),
            tola_typst::BundleEntryKind::Asset,
            src.to_vec(),
        );
        let cancel = BuildCancellation::default();
        let make_graph = |langs| {
            let mut graph = crate::output::graph::OutputGraphBuilder::new();
            crate::compiler::outputs::insert_bundle(&mut graph, [entry.clone()], langs, &cancel)
                .unwrap();
            graph.finish()
        };
        let unminified = make_graph(tola_minify::MinifiedLanguages::default());
        assert_eq!(unminified.outputs()[0].bytes(), src);
        let graph = make_graph(tola_minify::MinifiedLanguages::new(true, false));
        let out = &graph.outputs()[0];
        let bytes = std::str::from_utf8(out.bytes()).unwrap();
        assert!(bytes.starts_with("/*! License */"));
        assert!(bytes.contains(".a{color:red}"));
        assert_eq!(out.digest(), tola_typst::ContentDigest::of(out.bytes()));
        assert_ne!(out.digest(), unminified.outputs()[0].digest());
        assert_eq!(published_stylesheet(&graph), out.bytes());
        let view = HookOutputFiles::materialize_revision(
            site.path(),
            &OutputRevision::from_graph(&graph),
            &cancel,
        )
        .unwrap();
        assert_eq!(fs::read(view.root().join(path)).unwrap(), out.bytes());
    }

    #[test]
    fn non_bundle_stylesheet_verbatim() {
        let src = ".a { color: red; }";
        for owner in [
            OutputOwner::command(0, "stylesheets"),
            OutputOwner::configured_asset("assets/app.css"),
        ] {
            let css = output_file(
                "assets/app.css",
                OutputDeclaration::from_filesystem_source(Path::new("assets/app.css")),
                src.as_bytes(),
            )
            .with_owner(owner);
            assert_eq!(published_stylesheet(&output_graph([css])), src.as_bytes());
        }
    }
}
