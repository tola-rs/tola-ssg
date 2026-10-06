//! Installed development revisions, their requested HTML representations, and build-cache handoff.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::dev::http::html::HtmlInjection;
use crate::dev::http::response::ResponseBody;
use crate::dev::reload::transport::ReloadEndpoint;
use bytes::Bytes;

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

#[derive(Default)]
struct PreparedHtml {
    endpoint: Option<(ReloadEndpoint, tola_address::SiteUrlMount)>,
    pages: BTreeMap<String, Arc<RequestedHtml>>,
}

/// At most one representation per requested output in the installed graph, plus its welcome
/// page. Endpoint replacement releases every previous slot; responses own only byte fragments.
#[derive(Default)]
pub(super) struct HtmlResponses {
    prepared: Mutex<PreparedHtml>,
    welcome: OnceLock<Bytes>,
}

impl HtmlResponses {
    pub(super) fn welcome(&self, content: impl FnOnce() -> Bytes) -> &Bytes {
        self.welcome.get_or_init(content)
    }

    pub(super) fn page(
        &self,
        output: Option<&tola_address::OutputPath>,
        mount: &tola_address::SiteUrlMount,
        endpoint: &ReloadEndpoint,
    ) -> Arc<RequestedHtml> {
        let mut prepared = self
            .prepared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if prepared
            .endpoint
            .as_ref()
            .is_none_or(|(current, current_mount)| {
                current.port() != endpoint.port()
                    || current.session_token() != endpoint.session_token()
                    || current.generation() != endpoint.generation()
                    || current_mount != mount
            })
        {
            prepared.pages.clear();
            prepared.endpoint = Some((endpoint.clone(), mount.clone()));
        }
        let key = output.map_or("", tola_address::OutputPath::as_str);
        if let Some(page) = prepared.pages.get(key) {
            Arc::clone(page)
        } else {
            let page = Arc::new(RequestedHtml::default());
            prepared.pages.insert(key.to_owned(), Arc::clone(&page));
            page
        }
        // The slot leaves the map lock before either initialization can scan page bytes.
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

use arc_swap::ArcSwapOption;
use tola_build::build::{BuildSession, CheckedRevision};
use tola_build::site::SiteRevision;

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
    use tola_build::config::ResolvedSiteConfig;

    /// Build one candidate through `session`, so it has the origin
    /// [`BuildSession::install_revision`] requires.
    fn candidate(
        session: &mut BuildSession,
        root: &std::path::Path,
        title: &str,
    ) -> (CheckedRevision, Arc<ResolvedSiteConfig>) {
        std::fs::create_dir_all(root.join("content")).unwrap();
        std::fs::write(root.join("site.typ"), "#document(\"index.html\")[Site]").unwrap();
        let source = format!(
            "[site]\ntitle = {}\n",
            serde_json::to_string(title).unwrap()
        );
        let config = tola_build::config::loading::resolve_site_config(
            &root.join("tola.toml"),
            &source,
            tola_typst::PackageLocations::from_absolute_roots(None, None).unwrap(),
            &tola_build::config::loading::BuildOverrides::default(),
        )
        .unwrap()
        .into_config();
        let attempt = session.prepare(
            Arc::new(config),
            tola_build::build::BuildRequest::new(tola_build::build::BuildMode::Production),
        );
        let candidate = attempt.run().unwrap().into_unchecked_revision(None);
        let config = Arc::clone(candidate.site().config());
        let checked = candidate
            .check(&tola_build::cancellation::BuildCancellation::new())
            .unwrap()
            .into_checked()
            .unwrap();
        (checked, config)
    }

    #[test]
    fn replacement_swaps_whole_visible_site() {
        let root = tempfile::tempdir().unwrap();
        let sites = CurrentSite::new();
        let mut session = BuildSession::new();
        let (first, _) = candidate(&mut session, root.path(), "First");
        let (previous, first) = sites.replace(&mut session, first).unwrap();
        assert!(previous.is_none());

        let (second, second_config) = candidate(&mut session, root.path(), "Second");
        let (previous, second) = sites.replace(&mut session, second).unwrap();
        assert!(Arc::ptr_eq(&previous.unwrap(), &first));
        assert!(!Arc::ptr_eq(&sites.revision().unwrap(), &first));
        assert!(Arc::ptr_eq(&sites.revision().unwrap(), &second));
        assert!(Arc::ptr_eq(second.config(), &second_config));
    }

    #[test]
    fn previous_revision_survives_readers() {
        let root = tempfile::tempdir().unwrap();
        let sites = CurrentSite::new();
        let mut session = BuildSession::new();
        let (first, _) = candidate(&mut session, root.path(), "First");
        let (_, first) = sites.replace(&mut session, first).unwrap();
        let first_id = first.manifest().revision().clone();
        let (second, _) = candidate(&mut session, root.path(), "Second");
        let (previous, _) = sites.replace(&mut session, second).unwrap();
        let previous = previous.unwrap();
        drop(first);
        assert_eq!(previous.manifest().revision(), &first_id);
    }
}
