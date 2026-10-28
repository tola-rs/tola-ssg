//! The compiled sources one revision retains, keyed by the exact unsaved sources they came from.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use tola_build::cancellation::{BuildCancellation, BuildCancelled};
use tola_build::check::{SourceDiagnosticSession, SourceRevision};
use tola_build::config::ResolvedSiteConfig;
use tola_typst::typst::World;

/// The compiled sources one revision retains, keyed by the exact unsaved sources they were
/// produced from.
///
/// Every request reads a revision through the unsaved sources it needs — the source it asks about
/// pinned to the text its editor holds, or a query copy with one broken construct blanked. The
/// same sources compile to the same revision however often they are asked for, so the compilation
/// outlives the request that produced it and answers the next one. A newer revision or a new
/// configuration drops every compilation held.
pub(crate) struct RevisionCompilations {
    configuration: Option<Arc<ResolvedSiteConfig>>,
    revision: u64,
    compilations: Vec<CompiledSources>,
}

/// One compilation, with the exact unsaved sources it was produced from.
struct CompiledSources {
    sources: Vec<(PathBuf, Arc<str>)>,
    compiled: Arc<SourceRevision>,
}

/// How many compilations one revision keeps: the unsaved-source sets one request cycle reads —
/// the check's pinned set, then the query's pinned, repaired, and widened ones. Retaining a
/// site's worth of worlds would be retained state out of proportion with the edits that produced
/// it, and one more set of sources is a new revision rather than another read of this one.
const RETAINED_COMPILATIONS: usize = 4;

impl RevisionCompilations {
    pub(crate) fn new() -> Self {
        Self {
            configuration: None,
            revision: 0,
            compilations: Vec::new(),
        }
    }

    /// The revision these unsaved sources produce in `revision`, compiled now unless that revision
    /// already compiled exactly them.
    ///
    /// The configuration and revision travel with the read, so no caller can ask for a
    /// compilation without stating which revision's inputs it belongs to.
    ///
    /// A read answered from a compilation held re-observes nothing: it recognizes the
    /// configuration, the revision, and the exact unsaved sources, so an input outside those —
    /// the vendor workspace, a file the client never reported as changed — stays invisible by
    /// design until a source change starts the next revision.
    pub(crate) fn inspect(
        &mut self,
        session: &mut SourceDiagnosticSession,
        configuration: &Arc<ResolvedSiteConfig>,
        revision: u64,
        sources: Vec<(PathBuf, Arc<str>)>,
        cancellation: &BuildCancellation,
    ) -> Result<Arc<SourceRevision>, BuildCancelled> {
        cancellation.ensure_active()?;
        let adopted = self.adopt(configuration, revision);
        if adopted {
            if let Some(compiled) = self
                .compilations
                .iter()
                .find(|compiled| same_sources(&compiled.sources, &sources))
            {
                return Ok(Arc::clone(&compiled.compiled));
            }
            // A request that names the sources this revision already resolved — a query pinning a
            // file to the text the revision read, for instance — reads the revision's compilation
            // instead of deriving the site again. The world resolves every path the request names,
            // so a compilation read from these sources resolves to the same sources.
            if let Some(compiled) = self
                .compilations
                .iter()
                .find(|compiled| resolves_the_same(compiled, configuration, &sources))
            {
                return Ok(Arc::clone(&compiled.compiled));
            }
        }
        let compiled = Arc::new(session.inspect(sources.clone(), cancellation)?);
        if adopted {
            self.retain(sources, &compiled);
        }
        Ok(compiled)
    }

    /// Adopt the configuration and revision this read belongs to, and report whether the
    /// compilations held are the ones it may read.
    ///
    /// A new configuration and a newer revision both supersede what is held. An older revision
    /// reads none of it and retains none of its own: a check or a repair superseded while it ran
    /// must not drop the compilations the newest revision is answering from, and the next read of
    /// that revision finds them where they were left.
    fn adopt(&mut self, configuration: &Arc<ResolvedSiteConfig>, revision: u64) -> bool {
        let known = self
            .configuration
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, configuration));
        if !known {
            self.configuration = Some(Arc::clone(configuration));
            self.revision = revision;
            self.compilations.clear();
            return true;
        }
        match revision.cmp(&self.revision) {
            Ordering::Equal => true,
            Ordering::Greater => {
                self.revision = revision;
                self.compilations.clear();
                true
            }
            Ordering::Less => false,
        }
    }

    /// Keep one compiled revision, releasing the oldest when the bound is reached.
    fn retain(&mut self, sources: Vec<(PathBuf, Arc<str>)>, compiled: &Arc<SourceRevision>) {
        if self.compilations.len() == RETAINED_COMPILATIONS {
            self.compilations.remove(0);
        }
        self.compilations.push(CompiledSources {
            sources,
            compiled: Arc::clone(compiled),
        });
    }

    /// Let go of every compilation held; the next read re-derives what it needs.
    ///
    /// Only the compilations are released: the configuration pointer and the revision are the keys
    /// the next read compares against, and the session's own prepared resources outlive them.
    pub(crate) fn release(&mut self) {
        self.compilations.clear();
    }
}

/// Whether a held compilation answers the request it is compared against.
///
/// The request must name every unsaved source the compilation read — dropping one asks about
/// another input set — and the world that compilation built must resolve every path the request
/// names to the text the request asserts, so a pin with the text the revision already read
/// reads what is held.
fn resolves_the_same(
    compiled: &CompiledSources,
    configuration: &Arc<ResolvedSiteConfig>,
    sources: &[(PathBuf, Arc<str>)],
) -> bool {
    let Some(checked) = compiled.compiled.checked() else {
        return false;
    };
    let world = checked.world();
    let held = compiled
        .sources
        .iter()
        .map(|(path, text)| (path.as_path(), text))
        .collect::<BTreeMap<_, _>>();
    let requested = sources
        .iter()
        .map(|(path, text)| (path.as_path(), text))
        .collect::<BTreeMap<_, _>>();
    held.keys()
        .chain(requested.keys())
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .all(|path| {
            let Some(requested_text) = requested.get(path) else {
                return false;
            };
            let world_text = crate::identity::path_id(path, configuration.get_root())
                .and_then(|id| world.source(id).ok())
                .map(|source| source.text().to_owned());
            Some(requested_text.to_string()) == world_text
        })
}

/// Whether two unsaved-source sets read the same text under the same paths.
///
/// A pin or a repair rebuilds its set for every request, so the same inputs are equal paths
/// with equal text, never the same allocations.
fn same_sources(left: &[(PathBuf, Arc<str>)], right: &[(PathBuf, Arc<str>)]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|((left_path, left_text), (right_path, right_text))| {
                left_path == right_path && left_text == right_text
            })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tola_build::cancellation::BuildCancellation;
    use tola_build::check::SourceDiagnosticSession;

    use super::RevisionCompilations;
    use crate::compiler::tests::site_configuration;

    /// Retaining a compilation is an optimization, never a second semantics: a read answered from
    /// the cache says exactly what compiling those sources again says.
    #[test]
    fn cached_read_matches_fresh_compile() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir(root.join("content")).unwrap();
        std::fs::write(
            root.join("site.typ"),
            "#document(\"index.html\", format: \"html\")[#include \"content/page.typ\"]",
        )
        .unwrap();
        let document = root.join("content/page.typ");
        std::fs::write(&document, "saved\n").unwrap();
        let configuration = site_configuration(root, "[build]\nentry = \"site.typ\"\n");
        let mut session = SourceDiagnosticSession::new(Arc::clone(&configuration));
        let sources = vec![(document, Arc::<str>::from("#let unused = 1\nunsaved\n"))];

        let mut warm = RevisionCompilations::new();
        let first = warm
            .inspect(
                &mut session,
                &configuration,
                1,
                sources.clone(),
                &BuildCancellation::new(),
            )
            .unwrap();
        let cached = warm
            .inspect(
                &mut session,
                &configuration,
                1,
                sources.clone(),
                &BuildCancellation::new(),
            )
            .unwrap();
        assert!(
            Arc::ptr_eq(&first, &cached),
            "the revision already compiled these exact sources"
        );

        let mut cold = RevisionCompilations::new();
        let fresh = cold
            .inspect(
                &mut session,
                &configuration,
                1,
                sources,
                &BuildCancellation::new(),
            )
            .unwrap();

        assert!(
            !Arc::ptr_eq(&cached, &fresh),
            "a cache that never recompiled would compare a compilation with itself"
        );
        assert_eq!(cached.diagnostics(), fresh.diagnostics());
        assert_eq!(cached.compiled(), fresh.compiled());
        assert_eq!(
            cached
                .checked()
                .map(|checked| checked.file_reads().to_vec()),
            fresh.checked().map(|checked| checked.file_reads().to_vec()),
            "a reused compilation reads what the same sources read"
        );
    }
}
