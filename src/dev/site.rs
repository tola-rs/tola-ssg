//! Installed development revisions, their requested HTML representations, and build-cache handoff.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use arc_swap::ArcSwapOption;
use bytes::Bytes;
use tola_build::build::{BuildSession, CheckedRevision};
use tola_build::site::SiteRevision;

use crate::dev::http::html::HtmlInjection;
use crate::dev::http::response::ResponseBody;

#[derive(Default)]
pub(super) struct RequestedHtml {
    injection: OnceLock<HtmlInjection>,
    slices: OnceLock<[Bytes; 3]>,
}

impl RequestedHtml {
    pub(super) fn response(
        &self,
        method: &hyper::Method,
        content_len: usize,
        injection: impl FnOnce() -> HtmlInjection,
        content: impl FnOnce() -> Bytes,
    ) -> ResponseBody {
        let injection = self.injection.get_or_init(injection);
        if method == hyper::Method::HEAD {
            ResponseBody::head_metadata(injection.byte_len(content_len))
        } else {
            ResponseBody::slices(
                self.slices
                    .get_or_init(|| injection.slices(content()))
                    .clone(),
            )
        }
    }
}

/// Each installed revision owns the representations of its requested pages.
#[derive(Default)]
pub(super) struct HtmlResponses {
    pages: Mutex<BTreeMap<String, Arc<RequestedHtml>>>,
    welcome: OnceLock<Bytes>,
}

impl HtmlResponses {
    pub(super) fn welcome(&self, content: impl FnOnce() -> Bytes) -> &Bytes {
        self.welcome.get_or_init(content)
    }

    pub(super) fn page(&self, output: Option<&tola_address::OutputPath>) -> Arc<RequestedHtml> {
        let mut pages = self
            .pages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let key = output.map_or("", tola_address::OutputPath::as_str);
        if let Some(page) = pages.get(key) {
            return Arc::clone(page);
        }
        Arc::clone(pages.entry(key.to_owned()).or_default())
    }
}

pub(super) struct InstalledRevision {
    revision: Arc<SiteRevision>,
    pub(super) html: HtmlResponses,
}

impl InstalledRevision {
    pub(super) fn new(revision: Arc<SiteRevision>) -> Self {
        Self {
            revision,
            html: HtmlResponses::default(),
        }
    }
}

impl std::ops::Deref for InstalledRevision {
    type Target = SiteRevision;

    fn deref(&self) -> &SiteRevision {
        &self.revision
    }
}

/// Readers retain one immutable revision while the runtime replaces the current value.
#[derive(Clone)]
pub(super) struct CurrentSite {
    revision: Arc<ArcSwapOption<InstalledRevision>>,
    unpublished_html: Arc<HtmlResponses>,
}

impl CurrentSite {
    pub(super) fn new() -> Self {
        Self {
            revision: Arc::new(ArcSwapOption::empty()),
            unpublished_html: Arc::new(HtmlResponses::default()),
        }
    }

    pub(super) fn revision(&self) -> Option<Arc<SiteRevision>> {
        self.revision
            .load_full()
            .map(|installed| Arc::clone(&installed.revision))
    }

    pub(super) fn installed(&self) -> Option<Arc<InstalledRevision>> {
        self.revision.load_full()
    }

    pub(super) fn unpublished_html(&self) -> &HtmlResponses {
        &self.unpublished_html
    }

    /// The caller owns the final event guard. No source reads or hooks run here.
    pub(super) fn replace(
        &self,
        session: &mut BuildSession,
        candidate: CheckedRevision,
    ) -> anyhow::Result<(Option<Arc<SiteRevision>>, Arc<SiteRevision>)> {
        session.install_revision(candidate, |revision| {
            let revision = Arc::new(revision);
            let installed = Arc::new(InstalledRevision::new(Arc::clone(&revision)));
            let previous = self.revision.swap(Some(installed));
            Ok((
                previous.map(|installed| Arc::clone(&installed.revision)),
                revision,
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(
        session: &mut BuildSession,
        root: &std::path::Path,
        title: &str,
    ) -> CheckedRevision {
        let program = format!("#document(\"index.html\")[{title}]");
        crate::dev::tests::checked_candidate(
            session,
            crate::dev::tests::write_site(root, &program, ""),
            tola_build::build::BuildMode::Production,
        )
        .0
    }

    #[test]
    fn installed_readers_keep_previous_content() {
        let root = tempfile::tempdir().unwrap();
        let sites = CurrentSite::new();
        let mut session = BuildSession::new();
        let first = candidate(&mut session, root.path(), "First");
        sites.replace(&mut session, first).unwrap();
        let reader = sites.installed().unwrap();

        let second = candidate(&mut session, root.path(), "Second");
        sites.replace(&mut session, second).unwrap();
        let current = sites.installed().unwrap();

        let old = reader
            .outputs()
            .output_by_path_text("index.html")
            .unwrap()
            .bytes();
        let new = current
            .outputs()
            .output_by_path_text("index.html")
            .unwrap()
            .bytes();
        assert!(String::from_utf8_lossy(old).contains("First"));
        assert!(String::from_utf8_lossy(new).contains("Second"));
    }
}
