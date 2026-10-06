//! Sitemap exports of explicitly selected compiled document targets.

use std::collections::BTreeSet;

use typst::foundations::Value;

use crate::cancellation::BuildCancellation;
use crate::config::ResolvedSiteConfig;
use crate::seo::RenderError;
use crate::seo::declaration::{SeoDeclaration, index_path, published_date, resolve_target};

pub(super) fn render_outputs(
    config: &ResolvedSiteConfig,
    compilation: &tola_typst::BundleCompilation,
    cancellation: &BuildCancellation,
) -> Result<Vec<super::RenderedOutput>, RenderError> {
    cancellation.ensure_active()?;
    compilation.metadata_declarations("tola-sitemap").into_iter().map(|raw| {
        cancellation.ensure_active()?;
        let declaration = SeoDeclaration::parse("tola-sitemap", raw)?;
        declaration.check_fields(&declaration.fields, &["output", "targets"], "")?;
        declaration.require_origin(config)?;
        let output = declaration.output()?;
        let targets = declaration.required_array(&declaration.fields, "targets", "")?;
        let mut urls = BTreeSet::new();
        let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n");
        for (index, value) in targets.iter().enumerate() {
            cancellation.ensure_active()?;
            let field = index_path("/targets", index);
            let (target, lastmod) = match value {
                Value::Dict(fields) => {
                    declaration.check_fields(fields, &["target", "lastmod"], &field)?;
                    (declaration.required(fields, "target", &field)?, fields.get("lastmod").ok())
                }
                target => (target, None),
            };
            let target = resolve_target(
                &declaration,
                target,
                &field,
                &output,
                config,
                compilation,
                cancellation,
            )?;
            if target.url.fragment().is_some() {
                return Err(declaration
                    .invalid(
                        &field,
                        format!(
                            "sitemap target `{}` names an element, not a complete document",
                            target.url
                        ),
                    )
                    .with_help("Name the whole document by its output path")
                    .into());
            }
            if !urls.insert(target.url.clone()) {
                return Err(declaration.invalid(
                    &field,
                    format!("duplicate sitemap target `{}`", target.url),
                ).into());
            }
            xml.push_str("  <url>\n    <loc>");
            xml.push_str(&crate::html::escape(target.url.as_str()));
            xml.push_str("</loc>\n");
            if let Some(value) = lastmod.filter(|value| !matches!(value, Value::None)) {
                let field = crate::seo::declaration::field_path(&field, "lastmod");
                let date = match value {
                    Value::Datetime(date) => crate::seo::date::format_datetime(date)
                        .map_err(|error| declaration.invalid(&field, error.to_string()))?,
                    Value::Str(text) if text.len() == 10 => {
                        let timestamp = format!("{text}T00:00:00Z");
                        timestamp.parse::<atom_syndication::FixedDateTime>().map_err(|_| declaration.invalid(&field, "expected a valid YYYY-MM-DD date"))?;
                        text.to_string()
                    }
                    value => published_date(&declaration, value, &field)?.to_rfc3339(),
                };
                xml.push_str("    <lastmod>");
                xml.push_str(&date);
                xml.push_str("</lastmod>\n");
            }
            xml.push_str("  </url>\n");
        }
        xml.push_str("</urlset>\n");
        let url = tola_address::asset_url_from_output(&output);
        Ok(super::RenderedOutput {
            producer: "sitemap", url, declaration: crate::output::semantics::OutputDeclaration::sitemap(), bytes: xml.into_bytes().into(),
        })
    }).collect()
}
