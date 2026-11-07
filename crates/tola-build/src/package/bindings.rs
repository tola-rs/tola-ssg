//! The values the engine binds one compilation to, and the library it installs for them.

use std::sync::Arc;

use typst::foundations::{Array, Dict, IntoValue};
use typst::utils::LazyHash;

use crate::config::ResolvedSiteConfig;
use crate::config::section::site::SiteLanguage;
use crate::content::SourceRecords;
use tola_packages::library::{HostInputs, SiteLibrary};

/// Immutable site values shared by source evaluation and Bundle compilation.
#[derive(Clone, Debug)]
pub(crate) struct SiteBindings {
    site: Arc<LazyHash<Dict>>,
    asset_urls: crate::asset::AssetUrls,
    asset_origins: Dict,
    modules: Dict,
    module_accessed: tola_typst::AccessedDeps,
    module_diagnostics: tola_typst::Diagnostics,
}

impl SiteBindings {
    pub(crate) fn from_config(
        config: &ResolvedSiteConfig,
        asset_urls: crate::asset::AssetUrls,
    ) -> Self {
        Self {
            site: Arc::new(LazyHash::new(site_values(config))),
            asset_origins: asset_origins(config, &asset_urls),
            asset_urls,
            modules: Dict::new(),
            module_accessed: Default::default(),
            module_diagnostics: tola_typst::Diagnostics::new(),
        }
    }

    /// Whether a compilation bound to `self` still reads the same world inputs under `next`.
    pub(crate) fn matches(&self, next: &Self) -> bool {
        self.site == next.site
            && self.asset_urls.values_match(&next.asset_urls)
            && self.asset_origins == next.asset_origins
            && self.modules == next.modules
    }

    /// The library one compilation compiles with, holding these values and these sources.
    pub(crate) fn library(&self, sources: SourceRecords) -> SiteLibrary {
        let (source_records, source_records_by_file, source_origins) = sources.into_parts();
        SiteLibrary::new(HostInputs {
            site: (**self.site).clone(),
            asset_urls: self.asset_urls.to_typst_dict(),
            asset_origins: self.asset_origins.clone(),
            source_records,
            source_records_by_file,
            source_origins,
            modules: self.modules.clone(),
            module_validation: None,
        })
    }

    pub(crate) fn prepare_modules(
        mut self,
        config: &ResolvedSiteConfig,
        host: &crate::compiler::TypstHost,
        cancellation: &crate::cancellation::BuildCancellation,
    ) -> anyhow::Result<Self> {
        use typst::foundations::Value;
        let evaluated = match &config.module_inputs.validated_options {
            Some(evaluated) => Arc::clone(evaluated),
            None => Arc::new(crate::modules::options::evaluate_all(
                config,
                host,
                cancellation,
            )?),
        };
        self.modules = config.module_inputs.contexts(config, &self.asset_urls)?;
        for (alias, options) in &evaluated.values {
            let mut record = match self.modules.get(alias) {
                Ok(Value::Dict(record)) => record.clone(),
                _ => anyhow::bail!("module `{alias}` has no configuration record"),
            };
            record.insert("options".into(), options.clone().into_value());
            self.modules
                .insert(alias.as_str().into(), record.into_value());
        }
        self.module_accessed = evaluated.accessed.clone();
        self.module_diagnostics = evaluated.diagnostics.clone();
        Ok(self)
    }

    pub(crate) fn module_reads(&self) -> &[tola_typst::FileRead] {
        &self.module_accessed.reads
    }

    pub(crate) fn module_package_checks(&self) -> &[tola_typst::PackageCheck] {
        &self.module_accessed.package_checks
    }

    pub(crate) fn module_accessed(&self) -> &tola_typst::AccessedDeps {
        &self.module_accessed
    }

    pub(crate) fn module_diagnostics(&self) -> &tola_typst::Diagnostics {
        &self.module_diagnostics
    }
}

fn asset_origins(config: &ResolvedSiteConfig, assets: &crate::asset::AssetUrls) -> Dict {
    assets
        .origins()
        .filter_map(|(declared, origin)| {
            let crate::asset::AssetOrigin::File { source } = origin else {
                return None;
            };
            Some((
                declared.trim_start_matches('/').into(),
                crate::filesystem::display_path(source, config.get_root()).into_value(),
            ))
        })
        .collect()
}

pub(crate) fn site_values(config: &ResolvedSiteConfig) -> Dict {
    let site_config = &config.site;
    let mut site = Dict::new();
    site.insert("origin".into(), site_config.origin.as_deref().into_value());
    site.insert(
        "base-path".into(),
        site_config.base_path.base_path().into_value(),
    );
    site.insert("url".into(), config.site_url().into_value());
    site.insert("title".into(), site_config.title.as_str().into_value());
    site.insert(
        "authors".into(),
        site_config
            .authors
            .iter()
            .map(|author| {
                let mut author_fields = Dict::new();
                author_fields.insert("name".into(), author.name.as_str().into_value());
                author_fields.insert("email".into(), author.email.as_deref().into_value());
                author_fields.insert("url".into(), author.url.as_deref().into_value());
                author_fields.into_value()
            })
            .collect::<Array>()
            .into_value(),
    );
    site.insert(
        "description".into(),
        site_config.description.as_str().into_value(),
    );
    site.insert(
        "language".into(),
        language_fields(&site_config.language()).into_value(),
    );
    site.insert(
        "languages".into(),
        site_config
            .languages()
            .map(|language| language_fields(&language).into_value())
            .collect::<Array>()
            .into_value(),
    );
    site
}

/// One language as templates read it: the whole tag as `tag` and its parts for `#set text`.
fn language_fields(language: &SiteLanguage) -> Dict {
    let mut fields = Dict::new();
    fields.insert("tag".into(), language.tag().into_value());
    fields.insert("lang".into(), language.lang().into_value());
    fields.insert(
        "script".into(),
        match language.script() {
            Some(script) => script.into_value(),
            None => typst::foundations::Value::Auto,
        },
    );
    fields.insert("region".into(), language.region().into_value());
    fields
}
