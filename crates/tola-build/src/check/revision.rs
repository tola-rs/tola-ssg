use std::path::PathBuf;
use std::sync::Arc;

use crate::cancellation::{BuildCancellation, BuildCancelled};
use crate::compiler::SiteHost;
use crate::config::SiteConfig;
use tola_diagnostic::Diagnostic;

#[derive(Default)]
pub(super) struct CheckReads {
    pub(super) disk_reads: Vec<PathBuf>,
    pub(super) missing_disk_reads: Vec<PathBuf>,
    pub(super) package_checks: Vec<tola_typst::PackageCheck>,
    pub(super) file_reads: Vec<tola_typst::FileRead>,
}

impl CheckReads {
    pub(super) fn record(&mut self, accessed: &tola_typst::AccessedDeps) {
        self.file_reads.extend(accessed.reads.iter().cloned());
        self.disk_reads.extend(
            accessed
                .disk_reads
                .iter()
                .map(|r| r.as_path().to_path_buf()),
        );
        self.missing_disk_reads.extend(
            accessed
                .disk_reads
                .iter()
                .filter(|r| r.is_missing())
                .map(|r| r.as_path().to_path_buf()),
        );
        self.package_checks
            .extend(accessed.package_checks.iter().cloned());
    }
}

pub(super) enum CheckStop {
    Cancelled(BuildCancelled),
    Diagnostics(Vec<Diagnostic>),
}

pub(super) enum SourceCheckInput<'a> {
    Path(&'a str),
    File(typst::syntax::FileId),
}

pub(super) struct SourceCheck {
    pub(super) cfg: Arc<SiteConfig>,
    pub(super) host: SiteHost,
    pub(super) library: tola_packages::library::SiteLibrary,
}

/// Diagnostics and, when the check reached a world, the prepared compiler revision.
pub struct SourceRevision {
    pub(super) diagnostics: Vec<Diagnostic>,
    pub(super) checked: Option<SourceCompilation>,
    pub(super) accessed: CheckReads,
}

impl SourceRevision {
    pub(super) fn unprepared(diag: Vec<Diagnostic>) -> Self {
        Self {
            diagnostics: diag,
            checked: None,
            accessed: CheckReads::default(),
        }
    }

    /// Successful read-time observations from source analysis and compilation, including
    /// observations retained before a failed check. Different digests remain distinct.
    pub fn file_reads(&self) -> &[tola_typst::FileRead] {
        &self.accessed.file_reads
    }

    /// Physical read attempts made by this check, including failed input reads.
    pub fn disk_reads(&self) -> &[PathBuf] {
        &self.accessed.disk_reads
    }

    /// Physical paths reported missing at read time.
    pub(crate) fn missing_disk_reads(&self) -> &[PathBuf] {
        &self.accessed.missing_disk_reads
    }

    /// Every package candidate checked before this revision completed or failed.
    pub fn package_checks(&self) -> &[tola_typst::PackageCheck] {
        &self.accessed.package_checks
    }

    /// All diagnostics from this source revision.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Consume the revision, retaining its complete diagnostics.
    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }

    /// The checked site, absent when the check failed before it resolved a world.
    pub fn checked(&self) -> Option<&SourceCompilation> {
        self.checked.as_ref()
    }

    /// Take the compilation this revision checked, absent when it reached no world.
    pub fn into_checked(self) -> Option<SourceCompilation> {
        self.checked
    }

    /// Whether this revision compiled: the site's root program, or the one source a check
    /// compiled on its own.
    pub fn compiled(&self) -> bool {
        !self
            .diagnostics
            .iter()
            .any(|d| d.severity == tola_diagnostic::Severity::Error)
            && self
                .checked
                .as_ref()
                .is_some_and(|checked| checked.evaluation.is_some())
    }
}

/// A checked site: the resolved world, and the Bundle when the root program compiled.
pub struct SourceCompilation {
    pub(super) world: Arc<tola_typst::TypstWorld>,
    /// The root program this world attempted, absent for independent source checks.
    pub(super) site_entry: Option<typst::syntax::FileId>,
    /// What compiled, absent when nothing did.
    pub(super) evaluation: Option<Evaluation>,
}

pub(super) enum Evaluation {
    Site(Arc<tola_typst::BundleCompilation>),
    Source {
        document: Box<tola_typst::HtmlDocument>,
        file_reads: Vec<tola_typst::FileRead>,
    },
}

impl SourceCompilation {
    /// The immutable world containing the inputs this check resolved.
    pub fn world(&self) -> &tola_typst::TypstWorld {
        &self.world
    }

    /// The actual root program of this check, including a package driver.
    /// A standalone fallback never declares site-wide metadata, even if it compiles.
    pub fn site_entry(&self) -> Option<typst::syntax::FileId> {
        self.site_entry
    }

    /// The realized Bundle, before file export or publication, absent when the root program did
    /// not compile.
    pub fn bundle(&self) -> Option<&tola_typst::BundleCompilation> {
        match &self.evaluation {
            Some(Evaluation::Site(bundle)) => Some(bundle),
            _ => None,
        }
    }

    /// Typst's introspection of what compiled, absent when nothing did.
    pub fn introspector(&self) -> Option<&dyn tola_typst::typst::introspection::Introspector> {
        match &self.evaluation {
            Some(Evaluation::Site(bundle)) => Some(bundle.introspector()),
            Some(Evaluation::Source { document, .. }) => Some(document.introspector()),
            None => None,
        }
    }

    /// The reads the compilation made, empty when nothing compiled.
    pub fn file_reads(&self) -> &[tola_typst::FileRead] {
        match &self.evaluation {
            Some(Evaluation::Site(bundle)) => bundle.file_reads(),
            Some(Evaluation::Source { file_reads, .. }) => file_reads,
            None => &[],
        }
    }

    /// The addressable HTML documents this compilation realized, in document order.
    pub fn realized_documents(
        &self,
        cancellation: &BuildCancellation,
    ) -> anyhow::Result<Vec<RealizedDocument>> {
        let Some(Evaluation::Site(bundle)) = &self.evaluation else {
            return Ok(Vec::new());
        };
        let cancel = cancellation.bundle_cancellation();
        let mut docs = Vec::new();
        for doc in bundle.documents() {
            cancel.ensure_active()?;
            if doc.html_inventory(&cancel)?.is_none() {
                continue;
            }
            let output = crate::compiler::documents::output_path_for_virtual(doc.path())?;
            docs.push(RealizedDocument {
                permalink: tola_address::route_for_output(&output),
                output,
                sources: doc.source_ids(),
                rendered_sources: doc.rendered_source_ids(&cancel)?,
            });
        }
        Ok(docs)
    }
}

/// One document a compilation realized, with the address it is served at.
#[derive(Debug)]
pub struct RealizedDocument {
    /// The logical output path the document is written to.
    pub output: tola_address::OutputPath,
    /// The site-relative route the document is served at.
    pub permalink: tola_address::UrlPath,
    /// Original body identities: provenance, not ownership.
    pub sources: Vec<typst::syntax::FileId>,
    /// Additional identities from realized HTML; these are contributions, not primary ownership.
    pub rendered_sources: Vec<typst::syntax::FileId>,
}
