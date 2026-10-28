//! Development server, current revisions, and live reload.

mod hooks;
mod http;
mod rebuild;
mod reload;
mod session;
mod site;
mod watch;

pub(crate) use session::{ServerLaunch, run, run_preview};

#[cfg(test)]
pub(crate) mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use tola_build::build::{BuildMode, BuildRequest, BuildSession, CheckedRevision};
    use tola_build::config::ResolvedSiteConfig;

    /// The site `root` holds, documented by `program` and configured by `configuration`.
    pub(crate) fn write_site(
        root: &Path,
        program: &str,
        configuration: &str,
    ) -> Arc<ResolvedSiteConfig> {
        std::fs::create_dir_all(root.join("content")).unwrap();
        std::fs::write(root.join("site.typ"), program).unwrap();
        Arc::new(site_config(root, configuration))
    }

    /// The site `root` holds, resolved from `configuration`.
    pub(crate) fn site_config(root: &Path, configuration: &str) -> ResolvedSiteConfig {
        tola_build::config::loading::resolve_site_config(
            &root.join("tola.toml"),
            configuration,
            tola_typst::PackageLocations::default(),
            &tola_build::config::loading::BuildOverrides::default(),
        )
        .unwrap()
        .into_config()
    }

    /// The checked candidate `config` builds into through `session`, which gives it the origin
    /// [`BuildSession::install_revision`] requires.
    pub(crate) fn checked_candidate(
        session: &mut BuildSession,
        config: Arc<ResolvedSiteConfig>,
        mode: BuildMode,
    ) -> (CheckedRevision, Arc<ResolvedSiteConfig>) {
        let attempt = session.prepare(config, BuildRequest::new(mode));
        let candidate = attempt.run().unwrap().into_unchecked_revision(None);
        let config = Arc::clone(candidate.site().config());
        let checked = candidate
            .check(&tola_build::cancellation::BuildCancellation::new())
            .unwrap()
            .into_checked()
            .unwrap();
        (checked, config)
    }

    /// The outputs `config` publishes in `mode`, built outside any session.
    pub(crate) fn built_outputs(
        config: &ResolvedSiteConfig,
        mode: BuildMode,
    ) -> tola_build::output::revision::OutputRevision {
        tola_build::build::build_site(config, mode)
            .unwrap()
            .into_unchecked_revision(None)
            .site()
            .outputs()
            .clone()
    }

    /// Write the entry that reads `body` from `body.txt`, which the entry documents as
    /// `index.html`.
    pub(crate) fn write_source(config: &ResolvedSiteConfig, body: Option<&[u8]>) {
        let source = if let Some(body) = body {
            std::fs::write(config.get_root().join("body.txt"), body).unwrap();
            "#document(\"index.html\")[#read(\"body.txt\")]"
        } else {
            ""
        };
        std::fs::write(&config.build().entry, source).unwrap();
    }
}
