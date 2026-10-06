//! Native resolution of a configured asset URL to the URL this build publishes.

use typst::diag::{At, SourceResult, bail};
use typst::engine::Engine;
use typst::foundations::{Str, func};
use typst::syntax::{Span, Spanned};

use tola_address::{OutputPath, UrlPath};

/// The name `@tola/code` provides the code stylesheet under.
pub const CODE_STYLESHEET_FILE: &str = "code-stylesheet.css";

/// The reserved output path of the code stylesheet every build publishes.
pub fn code_stylesheet_output() -> String {
    format!("{}/{}", tola_address::RESERVED_ROOT, CODE_STYLESHEET_FILE)
}

// The code stylesheet's output path is reserved for Tola and the deployment base path applies, so
// the URL is resolved rather than returned as a literal.
#[func]
pub(super) fn tola_code_stylesheet_url(engine: &mut Engine, span: Span) -> SourceResult<Str> {
    let output = OutputPath::parse(&code_stylesheet_output())
        .expect("the reserved stylesheet output is a portable path");
    Ok(super::library::browser_output_url(engine, span, &output)?.into())
}

#[func]
pub(super) fn tola_asset_url(
    engine: &mut Engine,
    /// A declared site-root asset URL, such as "/app.js".
    declared_url: Spanned<Str>,
) -> SourceResult<Str> {
    // The key a declaration is stored under is its decoded spelling, so the lookup decodes once
    // here as well: `/caf%C3%A9.css` and `/café.css` name the one declaration.
    let url_path = UrlPath::parse(declared_url.v.as_str())
        .map_err(|error| error.to_string())
        .at(declared_url.span)?;
    let urls = super::library::asset_urls(engine)
        .ok_or("Tola could not read the site's configured asset URLs")
        .at(declared_url.span)?;
    let browser_url = match urls.get(url_path.as_str()) {
        Ok(value) => value
            .clone()
            .cast::<Str>()
            .map_err(|_| "Tola's configured asset URL map is malformed")
            .at(declared_url.span)?,
        Err(_) => bail!(
            declared_url.span,
            "`{}` is not an asset URL this site publishes",
            url_path.as_str();
            hint: "add it to `assets.files` in `tola.toml` as `{{ source = \"…\", url = \"{}\" }}`",
            url_path.as_str();
            hint: "check the member path under a `assets.trees` source"
        ),
    };
    Ok(browser_url)
}

/// What a host binding's string argument names, when it names something in this site.
///
/// A domain is a vocabulary, not a lookup: the consumer that resolves the argument decides what
/// keys belong to it. This exists so that consumer never has to match the binding's name, the
/// author's spelling of it, or the package an author imported it from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgumentDomain {
    /// A site-root URL one `assets` declaration publishes, as `asset-url` takes it.
    SiteAssetUrl,
}

/// One host binding whose string argument carries a domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArgumentDomainDeclaration {
    /// The native's own name: what `Func::name` reports for the value a site imports.
    ///
    /// A function's identity is its own name, not the name a site binds or writes it under, so an
    /// import rename and the several packages that re-export one native all reach this row.
    pub function: &'static str,
    /// The parameter's name as Typst exposes it, which is how the author writes it too.
    pub parameter: &'static str,
    /// The vocabulary the argument names.
    pub domain: ArgumentDomain,
}

/// Every host binding whose string argument names something in this site.
///
/// The row's names are checked against the native they describe by this module's own test, so a
/// rename of the function or its parameter cannot leave the table describing nothing.
pub const ARGUMENT_DOMAINS: &[ArgumentDomainDeclaration] = &[ArgumentDomainDeclaration {
    function: "tola-asset-url",
    parameter: "declared-url",
    domain: ArgumentDomain::SiteAssetUrl,
}];

#[cfg(test)]
mod tests {
    use typst::foundations::{Func, NativeFunc};

    use super::*;

    /// Every row names a native of this crate and a parameter that native declares.
    #[test]
    fn argument_domains_name_their_native_parameter() {
        for row in ARGUMENT_DOMAINS {
            let data = match row.function {
                "tola-asset-url" => tola_asset_url::data(),
                other => panic!("no native of this crate is named `{other}`"),
            };
            assert_eq!(Func::from(data).name(), Some(row.function));
            assert!(
                data.params.iter().any(|param| param.name == row.parameter),
                "`{}` declares no parameter `{}`",
                row.function,
                row.parameter
            );
        }
    }
}
