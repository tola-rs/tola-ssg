//! Native resolution of configured asset URLs.

use typst::diag::{At, SourceResult, bail};
use typst::engine::Engine;
use typst::foundations::{Str, func};
use typst::syntax::{Span, Spanned};

use tola_address::{OutputPath, UrlPath};

/// `@tola/code` stylesheet file name.
pub const CODE_STYLESHEET_FILE: &str = "code-stylesheet.css";

/// Reserved output path of the published code stylesheet.
pub fn code_stylesheet_output() -> String {
    format!("{}/{}", tola_address::RESERVED_ROOT, CODE_STYLESHEET_FILE)
}

#[func]
pub(super) fn tola_code_stylesheet_url(engine: &mut Engine, span: Span) -> SourceResult<Str> {
    let out = OutputPath::parse(&code_stylesheet_output())
        .expect("reserved stylesheet output is a valid output path");
    Ok(super::library::browser_output_url(engine, span, &out)?.into())
}

#[func]
pub(super) fn tola_asset_url(
    engine: &mut Engine,
    /// Declared site-root asset URL, such as `"/app.js"`.
    declared_url: Spanned<Str>,
) -> SourceResult<Str> {
    let span = declared_url.span;
    // Asset declarations are keyed by decoded spelling, so `/caf%C3%A9.css` and `/café.css` match.
    let url = UrlPath::parse(declared_url.v.as_str())
        .map_err(|err| err.to_string())
        .at(span)?;
    let urls = super::library::asset_urls(engine)
        .ok_or("Tola could not read the site's configured asset URLs")
        .at(span)?;
    match urls.get(url.as_str()) {
        Ok(val) => val
            .clone()
            .cast::<Str>()
            .map_err(|_| "Tola's configured asset URL map is malformed")
            .at(span),
        Err(_) => bail!(
            span,
            "`{}` is not an asset URL this site publishes",
            url.as_str();
            hint: "add it to `assets.files` in the site configuration as `{{ source = \"…\", url = \"{}\" }}`",
            url.as_str();
            hint: "check the member path under a `assets.trees` source"
        ),
    }
}

/// Domain carried by a host binding's string argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgumentDomain {
    /// Site-root URL published by an `assets` declaration.
    SiteAssetUrl,
}

/// Host binding whose string argument carries a site-specific domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArgumentDomainDeclaration {
    /// Native function name.
    pub function: &'static str,
    /// Parameter name exposed to Typst.
    pub parameter: &'static str,
    /// Vocabulary named by the argument.
    pub domain: ArgumentDomain,
}

/// Host bindings whose string arguments name site values.
pub const ARGUMENT_DOMAINS: &[ArgumentDomainDeclaration] = &[ArgumentDomainDeclaration {
    function: "tola-asset-url",
    parameter: "declared-url",
    domain: ArgumentDomain::SiteAssetUrl,
}];

#[cfg(test)]
mod tests {
    use typst::foundations::{Func, NativeFunc};

    use super::*;

    #[test]
    fn argument_domains_match_native_params() {
        for row in ARGUMENT_DOMAINS {
            let data = match row.function {
                "tola-asset-url" => tola_asset_url::data(),
                other => panic!("unknown native `{other}`"),
            };
            assert_eq!(Func::from(data).name(), Some(row.function));
            assert!(
                data.params.iter().any(|p| p.name == row.parameter),
                "`{}` declares no parameter `{}`",
                row.function,
                row.parameter
            );
        }
    }
}
