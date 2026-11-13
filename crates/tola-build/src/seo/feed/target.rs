//! Feed entry URLs and their exported anchors.

use typst::introspection::Location;
use typst::syntax::VirtualPath;

use crate::cancellation::BuildCancellation;
use crate::config::ResolvedSiteConfig;
use crate::seo::RenderError;
use crate::seo::declaration::{
    DocumentTarget, SeoDeclaration, SeoDeclarationError, TargetAnchor, resolve_document,
};
use tola_address::OutputPath;
use typst::foundations::Value;

const FRAGMENT_ENCODE_SET: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
    .add(b'%')
    .add(b' ')
    .add(b'"')
    .add(b'<')
    .add(b'>')
    .add(b'`');

pub(super) fn resolve_target<'a>(
    declaration: &SeoDeclaration,
    value: &Value,
    field: &str,
    output: &OutputPath,
    config: &ResolvedSiteConfig,
    compilation: &'a tola_typst::BundleCompilation,
    cancellation: &BuildCancellation,
) -> Result<DocumentTarget<'a>, RenderError> {
    let (mut target, anchor) =
        resolve_document(declaration, value, field, config, compilation, cancellation)?;
    match anchor {
        Some(TargetAnchor::Fragment(fragment)) => {
            if let Some(html) = target
                .document
                .html_inventory(&cancellation.bundle_cancellation())?
            {
                let mut found = false;
                for anchor in html.fragments() {
                    cancellation.ensure_active()?;
                    if anchor.value() == fragment {
                        found = true;
                        break;
                    }
                }
                if !found {
                    return Err(declaration
                    .invalid(
                        field,
                        format!(
                            "target document `{}` has no exported anchor `{fragment}`",
                            target.output
                        ),
                    )
                    .with_help(
                        "Name an anchor the document exports, or drop `fragment` to target the whole document",
                    )
                    .into());
                }
            }
            let fragment =
                percent_encoding::utf8_percent_encode(&fragment, FRAGMENT_ENCODE_SET).to_string();
            target.url.set_fragment(Some(&fragment));
        }
        Some(TargetAnchor::Location(location)) => {
            let fragment =
                location_fragment(declaration, field, location, output, config, compilation)?;
            target.url.set_fragment(fragment.as_deref());
        }
        None => {}
    }
    cancellation.ensure_active()?;
    Ok(target)
}

fn location_fragment(
    declaration: &SeoDeclaration,
    field: &str,
    location: Location,
    output: &OutputPath,
    config: &ResolvedSiteConfig,
    compilation: &tola_typst::BundleCompilation,
) -> Result<Option<String>, SeoDeclarationError> {
    let introspector = compilation.introspector();
    let base_path = VirtualPath::new(output.as_str()).expect("validated output path");
    let resolved = typst::model::LateLinkResolver::new(Some(&base_path), introspector)
        .resolve(location)
        .ok_or_else(|| {
            declaration
                .invalid(field, "target element has no exported anchor")
                .with_help("Target the document's output path, or an element the site links to")
        })?
        .into_relative_uri()
        .map_err(|_| {
            declaration.invalid(field, "the target link cannot be built from this location")
        })?;
    let output_url = tola_address::asset_url_from_output(output);
    let resolved_url = url::Url::parse(&config.canonical_url(&output_url))
        .and_then(|base| base.join(&resolved))
        .map_err(|_| {
            declaration.invalid(field, "the target link cannot be built from this location")
        })?;
    Ok(resolved_url.fragment().map(str::to_owned))
}
