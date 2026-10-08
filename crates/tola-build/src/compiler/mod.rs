//! Typst source analysis, Bundle compilation, and document interpretation.

pub(crate) mod analysis;
pub(crate) mod bundle;
mod dependency;
mod diagnostic;
pub(crate) mod documents;
mod host;
mod html;
mod inputs;
pub(crate) mod outputs;

pub(crate) use dependency::{
    CompilationDependencies, RebuildDecision, ReusedDependencyReaders, TypstDependencyReader,
};
pub(crate) use diagnostic::{error_diagnostics, source_location, warning_diagnostics};
pub(crate) use host::TypstHost;
pub(crate) use html::minify_generated_payloads;
pub(crate) use inputs::BuildInputs;

#[derive(Clone)]
pub(crate) struct SiteProgramCache {
    pub(crate) root: std::path::PathBuf,
    pub(crate) documents: Vec<crate::site::HtmlPage>,
    pub(crate) html_inventories: std::collections::BTreeMap<
        tola_address::OutputPath,
        std::sync::Arc<tola_typst::HtmlDocumentInventory>,
    >,
    pub(crate) outputs: crate::output::graph::OutputGraph,
    /// Retained so a reused compilation reports the same warnings.
    pub(crate) diagnostics: tola_typst::Diagnostics,
    pub(crate) payload_diagnostics: Vec<crate::diagnostic::Diagnostic>,
    pub(crate) pretty_html: bool,
    pub(crate) minified_languages: tola_minify::MinifiedLanguages,
    pub(crate) entry: std::path::PathBuf,
    pub(crate) bundle_entries: tola_typst::BundleEntries,
    pub(crate) compilation: std::sync::Arc<tola_typst::BundleCompilation>,
    pub(crate) world: std::sync::Arc<tola_typst::TypstWorld>,
}

impl SiteProgramCache {
    pub(crate) fn matches_site(&self, config: &crate::config::ResolvedSiteConfig) -> bool {
        self.root == crate::filesystem::normalize_path(config.get_root())
            && crate::filesystem::normalize_path(&self.entry)
                == crate::filesystem::normalize_path(&config.build.entry)
    }

    pub(crate) fn matches_export_config(&self, config: &crate::config::ResolvedSiteConfig) -> bool {
        self.pretty_html == !config.build.minify.html
    }

    /// Whether this compilation already has the minification the configuration requests.
    ///
    /// Re-exporting cannot undo a rewritten payload, so a changed answer here needs a fresh
    /// compilation rather than an export of the retained one.
    pub(crate) fn matches_minification(&self, config: &crate::config::ResolvedSiteConfig) -> bool {
        self.minified_languages == minified_languages(config)
    }
}

/// The languages one configuration minifies in configured assets and generated HTML.
pub(crate) fn minified_languages(
    config: &crate::config::ResolvedSiteConfig,
) -> tola_minify::MinifiedLanguages {
    tola_minify::MinifiedLanguages::new(config.build.minify.css, config.build.minify.javascript)
}

#[derive(Clone)]
pub(crate) enum CompilationReuse {
    SourceAnalysis {
        source_analysis: Box<analysis::SourceAnalysisCache>,
        dependencies: Box<CompilationDependencies>,
    },
    SiteProgram {
        site_program: Box<SiteProgramCache>,
        dependencies: Box<CompilationDependencies>,
        source_analysis: Box<analysis::SourceAnalysisCache>,
    },
}

impl CompilationReuse {
    pub(crate) fn source_analysis_reuse(&self) -> analysis::SourceAnalysisReuse<'_> {
        match self {
            Self::SourceAnalysis {
                source_analysis,
                dependencies,
            }
            | Self::SiteProgram {
                source_analysis,
                dependencies,
                ..
            } => analysis::SourceAnalysisReuse::retained(source_analysis, dependencies),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::Path;

    /// Typst host for `config` with default build resources and a fresh cancellation token.
    pub(crate) fn compiler_host(
        config: &crate::config::ResolvedSiteConfig,
    ) -> std::result::Result<crate::compiler::TypstHost, tola_typst::FontLoadError> {
        compiler_host_with(config, &crate::resources::BuildResources::default())
    }

    /// Build a host from caller-owned resources, which the caller can compare it against.
    pub(crate) fn compiler_host_with(
        config: &crate::config::ResolvedSiteConfig,
        resources: &crate::resources::BuildResources,
    ) -> std::result::Result<crate::compiler::TypstHost, tola_typst::FontLoadError> {
        crate::compiler::TypstHost::for_config_with_resources(
            config,
            resources,
            &crate::cancellation::BuildCancellation::default(),
        )
    }

    pub(crate) fn configure_site(
        config: &mut crate::config::ResolvedSiteConfig,
        content: &Path,
        entry: &Path,
    ) {
        config.build.entry = entry.to_path_buf();
        config.build.content_dir = content.to_path_buf();
    }

    pub(crate) fn source_metadata(
        scan: &crate::compiler::analysis::SourceScan,
        suffix: &str,
    ) -> typst::foundations::Dict {
        analyzed_source(scan, suffix)
            .metadata()
            .expect("analyzed source declares metadata")
            .to_typst_dict()
    }

    pub(crate) fn metadata_field(
        scan: &crate::compiler::analysis::SourceScan,
        suffix: &str,
        field: &str,
    ) -> typst::foundations::Value {
        source_metadata(scan, suffix)
            .get(field)
            .unwrap_or_else(|_| panic!("analyzed source `{suffix}` declares `{field}`"))
            .clone()
    }

    fn analyzed_source<'a>(
        scan: &'a crate::compiler::analysis::SourceScan,
        suffix: &str,
    ) -> &'a crate::content::ContentSource {
        scan.source_set
            .sources()
            .iter()
            .find(|source| source.source().ends_with(suffix))
            .unwrap_or_else(|| panic!("analyzed source `{suffix}` is in the source set"))
    }

    /// Evaluate and realize one site program, for tests that need a compiled Bundle.
    ///
    /// The returned directory owns the site the Bundle was read from; keep it alive.
    pub(crate) fn realized_site(
        site: &str,
    ) -> (
        tempfile::TempDir,
        crate::compiler::bundle::RealizedBundle,
        tola_typst::BundleCancellation,
    ) {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join("content")).unwrap();
        std::fs::write(directory.path().join("site.typ"), site).unwrap();
        let config = crate::config::tests::load_test_config(directory.path(), "");
        let host = crate::compiler::TypstHost::for_config_with_resources(
            &config,
            &crate::resources::BuildResources::default(),
            &crate::cancellation::BuildCancellation::default(),
        )
        .unwrap();
        let cancellation = tola_typst::BundleCancellation::new();
        let mut inputs = crate::compiler::BuildInputs::default();
        let evaluated = crate::compiler::bundle::evaluate(
            &config,
            &host,
            &[],
            crate::package::SiteBindings::from_config(&config, Default::default()),
            &cancellation,
            crate::compiler::analysis::SourceAnalysisReuse::None,
            &mut inputs,
        )
        .unwrap();
        let realized = crate::compiler::bundle::realize(
            &config,
            &host,
            &cancellation,
            &evaluated,
            &mut inputs,
        )
        .unwrap();
        (directory, realized, cancellation)
    }

    pub(crate) fn brand_icons(
        mark_svg: &str,
        unused_svg: &str,
    ) -> std::sync::Arc<tola_icons::IconCollections> {
        let mut collection = tola_icons::IconCollection::new();
        collection.insert_svg("mark", mark_svg).unwrap();
        collection.insert_svg("unused", unused_svg).unwrap();
        let mut collections = tola_icons::IconCollections::new();
        collections.mount("brand", collection).unwrap();
        std::sync::Arc::new(collections)
    }

    #[test]
    fn compiler_cancellation_is_classified() {
        let canceller = crate::cancellation::BuildCanceller::new();
        let cancellation = canceller.token();
        let typst = cancellation.bundle_cancellation();
        canceller.cancel();
        assert!(crate::cancellation::is_cancelled(&anyhow::Error::new(
            typst.ensure_active().unwrap_err()
        )));
        assert!(crate::cancellation::is_cancelled(&anyhow::Error::new(
            cancellation.ensure_active().unwrap_err()
        )));
    }
}
