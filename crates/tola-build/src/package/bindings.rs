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
        }
    }

    /// Whether a compilation bound to `self` still reads the same world inputs under `next`.
    ///
    /// The site values must agree; which asset declarations still resolve the
    /// same way is `AssetUrls::values_match`'s question.
    pub(crate) fn matches(&self, next: &Self) -> bool {
        self.site == next.site
            && self.asset_urls.values_match(&next.asset_urls)
            && self.asset_origins == next.asset_origins
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
        })
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

fn site_values(config: &ResolvedSiteConfig) -> Dict {
    let site_config = &config.site;
    let mut site = Dict::new();
    site.insert("origin".into(), site_config.origin.as_deref().into_value());
    site.insert(
        "base-path".into(),
        site_config.base_path.as_str().into_value(),
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
    site.insert(
        "copyright".into(),
        site_config.copyright.as_str().into_value(),
    );
    let mut extra_fields = site_config.extra.iter().collect::<Vec<_>>();
    extra_fields.sort_unstable_by_key(|(key, _)| *key);
    let extra = extra_fields
        .into_iter()
        .map(|(key, value)| (key.as_str().into(), toml_to_value(value)))
        .collect::<Dict>();
    site.insert("extra".into(), extra.into_value());
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

fn toml_to_value(toml_value: &toml::Value) -> typst::foundations::Value {
    match toml_value {
        toml::Value::String(string) => string.as_str().into_value(),
        toml::Value::Integer(integer) => integer.into_value(),
        toml::Value::Float(float) => float.into_value(),
        toml::Value::Boolean(boolean) => boolean.into_value(),
        toml::Value::Datetime(datetime) => datetime.to_string().into_value(),
        toml::Value::Array(elements) => elements
            .iter()
            .map(toml_to_value)
            .collect::<Array>()
            .into_value(),
        toml::Value::Table(fields) => fields
            .iter()
            .map(|(key, value)| (key.as_str().into(), toml_to_value(value)))
            .collect::<Dict>()
            .into_value(),
    }
}

#[cfg(test)]
mod tests {
    use typst::foundations::IntoValue;

    use super::*;

    fn configured_bindings(settings: &str) -> SiteBindings {
        let site_config = crate::config::tests::OwnedSiteConfig::new(settings);
        SiteBindings::from_config(&site_config.config, Default::default())
    }

    #[test]
    fn site_value_changes_prevent_reuse() {
        let bindings = configured_bindings("[site]\ntitle = \"First\"\nbase-path = \"/blog/\"");
        let unchanged = configured_bindings("[site]\ntitle = \"First\"\nbase-path = \"/blog/\"");
        assert!(bindings.matches(&unchanged));

        let retitled = configured_bindings("[site]\ntitle = \"Second\"\nbase-path = \"/blog/\"");
        assert!(!bindings.matches(&retitled));

        let remounted = configured_bindings("[site]\ntitle = \"First\"\nbase-path = \"/docs/\"");
        assert!(!bindings.matches(&remounted));
    }

    #[test]
    fn site_value_url_joins_origin_and_base_path() {
        let identity = configured_bindings(
            "[site]\norigin = \"https://example.test\"\nbase-path = \"/docs/blog/\"",
        );

        assert_eq!(
            identity.site.get("origin").unwrap(),
            &"https://example.test".into_value()
        );
        assert_eq!(
            identity.site.get("base-path").unwrap(),
            &"/docs/blog/".into_value()
        );
        assert_eq!(
            identity.site.get("url").unwrap(),
            &"https://example.test/docs/blog/".into_value()
        );
    }

    #[test]
    fn site_publishes_the_documented_members() {
        // The site value is what a site reads — `@tola/site` re-exports it verbatim — so
        // this member list, in this order, is the contract a site reads.
        let site_config =
            crate::config::tests::OwnedSiteConfig::new("[site]\nlanguage = \"en\"\n");
        let published = site_values(&site_config.config);
        let published = published
            .iter()
            .map(|(member, _)| member.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            published,
            [
                "origin",
                "base-path",
                "url",
                "title",
                "authors",
                "description",
                "language",
                "languages",
                "copyright",
                "extra",
            ]
        );
    }
}
