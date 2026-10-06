//! What an `asset(…)` argument publishes, and which published asset URLs an argument may name.
//!
//! An asset argument names an output the site creates rather than a file the site already holds,
//! and it resolves from the site root: `asset("downloads/file.pdf", bytes)` publishes
//! `public/downloads/file.pdf`. So it answers where the output lands, not where a file is served.
//!
//! The second question belongs to `asset-url(declared-url)`: its argument names one of the URLs an
//! `assets` declaration publishes. The inventory of those URLs is the generator's (`AssetUrls`,
//! the same view a check resolves against), while the domain an argument has is declared by
//! the crate that owns the binding (`tola_packages::ARGUMENT_DOMAINS`).

use std::path::Path;

use lsp_types::Hover;
use tola_build::config::ResolvedSiteConfig;
use tola_build::config::section::assets::{AssetUrl, AssetUrlError};
use tola_build::{AssetOrigin, AssetUrls, PublishedAsset};
use tola_packages::{ARGUMENT_DOMAINS, ArgumentDomain};
use tola_typst::typst::foundations::Func;
use tola_typst::typst::syntax::Source;

use crate::position;

/// What the site publishes the asset argument at the offset as, when the offset sits in one.
pub(super) fn hover(config: &ResolvedSiteConfig, source: &Source, at: usize) -> Option<Hover> {
    let argument = tola_typst_syntax::syntax::path_argument(source, at)?;
    if argument.base != tola_typst_syntax::syntax::PathBase::Site {
        return None;
    }
    let range = argument.range;
    let written = source.text().get(range.clone())?;
    // An output is a logical path below the output root, so a path the Bundle cannot publish names
    // no output at all: the answer says so rather than staying silent, and the rule the path breaks
    // goes to the debug log.
    let described = match published_path(config, written) {
        Ok(published) => format!("Published at `{published}`."),
        Err(rule) => {
            tracing::debug!(%rule, "asset argument names no path the site publishes");
            format!("`{written}` is not a path this site publishes")
        }
    };
    Some(crate::protocol::markdown_hover(
        described,
        Some(position::utf16_range(source.lines(), range)?),
    ))
}

/// The site-relative path one asset argument publishes, or the rule the written path breaks.
///
/// The argument takes both spellings of the site root, and the logical output identity comes from
/// the declaration rule the Bundle itself applies.
fn published_path(config: &ResolvedSiteConfig, written: &str) -> Result<String, AssetUrlError> {
    let declared = format!("/{}", written.trim_start_matches('/'));
    let asset = AssetUrl::parse(&declared)?;
    let output = config
        .get_root()
        .join(&config.build().publish_dir)
        .join(asset.output_path().as_str());
    Ok(tola_build::filesystem::display_path(
        &output,
        config.get_root(),
    ))
}

/// What one written site-root URL resolves to among this site's published asset URLs.
pub(super) enum Written<'a> {
    /// A declaration publishes it, at the address a browser fetches.
    Published(PublishedAsset<'a>),
    /// No declaration of this site publishes it.
    Unpublished,
}

/// Resolve one written site-root URL the way `asset-url` resolves it.
///
/// The declaration's own spelling is the key, so the decode happens once here exactly as the
/// lookup decodes it: `/caf%C3%A9.svg` and `/café.svg` name the one declaration. Everything else
/// names nothing this site publishes, whether the spelling is wrong or no declaration covers it.
pub(super) fn written<'a>(urls: &'a AssetUrls, text: &str) -> Written<'a> {
    let decoded = decoded(text);
    urls.published()
        .find(|asset| asset.declared == decoded)
        .map_or(Written::Unpublished, Written::Published)
}

/// Every published URL whose declared spelling starts with `typed`, in declared order.
///
/// Completion answers with these, so the order is the inventory's own rather than a preference
/// this function invents.
fn candidates<'a>(urls: &'a AssetUrls, typed: &str) -> Vec<&'a str> {
    let typed = decoded(typed);
    urls.published()
        .map(|asset| asset.declared)
        .filter(|declared| declared.starts_with(typed.as_str()))
        .collect()
}

/// The domain one named parameter of the function a call reaches declares, when one is declared.
///
/// The function is identified by the value's own name rather than by what the call site spells, so
/// importing the binding under another name, or through another package that re-exports it, keeps
/// the domain.
pub(super) fn domain_of(callee: &Func, parameter: &str) -> Option<ArgumentDomain> {
    let function = callee.name()?;
    ARGUMENT_DOMAINS
        .iter()
        .find(|row| row.function == function && row.parameter == parameter)
        .map(|row| row.domain)
}

/// The completion response one written asset-URL argument answers with.
///
/// Candidates keep the inventory's own order, and the edit replaces the inside of the argument's
/// quotes: the insert range covers only what the author has typed, so a client that keeps typing
/// after accepting a candidate replaces the rest of the quoted spelling. The edit is the
/// insert-and-replace shape, which the reply boundary projects for a client that did not declare
/// `completionItem.insertReplaceSupport`.
pub(super) fn url_completions(
    urls: &AssetUrls,
    site_root: &Path,
    source: &Source,
    typed: &str,
    written: std::ops::Range<usize>,
    cursor: usize,
) -> lsp_types::CompletionResponse {
    let Some(replace) = position::utf16_range(source.lines(), written.clone()) else {
        return lsp_types::CompletionResponse::Array(Vec::new());
    };
    let Some(insert) = position::utf16_range(source.lines(), written.start..cursor) else {
        return lsp_types::CompletionResponse::Array(Vec::new());
    };
    let items = candidates(urls, typed)
        .into_iter()
        .map(|declared| {
            let detail = urls
                .origin(declared)
                .map(|origin| declaration_note(origin, site_root));
            lsp_types::CompletionItem {
                label: declared.to_owned(),
                kind: Some(lsp_types::CompletionItemKind::VALUE),
                detail,
                text_edit: Some(lsp_types::CompletionTextEdit::InsertAndReplace(
                    lsp_types::InsertReplaceEdit {
                        new_text: declared.to_owned(),
                        insert,
                        replace,
                    },
                )),
                ..lsp_types::CompletionItem::default()
            }
        })
        .collect();
    lsp_types::CompletionResponse::List(lsp_types::CompletionList {
        // Deploy-root URLs hold no relative navigation, so no candidate is ever withheld.
        is_incomplete: false,
        items,
    })
}

/// The file that declares one written URL, or nothing when the site publishes no such URL.
pub(super) fn written_definition(
    urls: &AssetUrls,
    client_root: &crate::uri::ClientRoot,
    source: &Source,
    written_range: std::ops::Range<usize>,
) -> Option<lsp_types::GotoDefinitionResponse> {
    let text = source.text().get(written_range)?;
    let Written::Published(asset) = written(urls, text) else {
        return None;
    };
    let path = match asset.origin? {
        AssetOrigin::File { source } => source.clone(),
        AssetOrigin::TreeMember { source, member } => source.join(member),
    };
    Some(lsp_types::GotoDefinitionResponse::Scalar(
        lsp_types::Location {
            uri: client_root.address(&path).ok()?,
            range: lsp_types::Range::default(),
        },
    ))
}

/// How one declaration is named to an author: its own file, or the tree a member came from.
pub(super) fn declaration_note(origin: &AssetOrigin, site_root: &Path) -> String {
    match origin {
        AssetOrigin::File { source } => format!(
            "`{}`",
            tola_build::filesystem::display_path(source, site_root)
        ),
        AssetOrigin::TreeMember { source, member } => {
            format!(
                "`{}` in the tree `{}`",
                tola_build::filesystem::display_path(member, site_root),
                tola_build::filesystem::display_path(source, site_root)
            )
        }
    }
}

/// One written URL's decoded spelling, as the lookup keys it.
///
/// A URL that does not decode is returned as written: completion runs while the author is still
/// typing, and a half-written escape is not yet a URL.
fn decoded(text: &str) -> String {
    AssetUrl::parse(text)
        .map(|url| url.as_str().to_owned())
        .unwrap_or_else(|_| text.to_owned())
}

#[cfg(test)]
mod tests {
    use tola_build::config::loading::{BuildOverrides, load_site_config};
    use tola_packages::HOST_MODULE;
    use tola_packages::library::{HostInputs, SiteLibrary};
    use tola_typst::typst::foundations::{Array, Dict, Value};

    use super::*;

    fn asset_output(written: &str) -> Result<String, AssetUrlError> {
        let directory = tempfile::tempdir().expect("a site directory");
        let configuration = directory.path().join("tola.toml");
        std::fs::write(&configuration, "").expect("a configuration");
        let config = load_site_config(
            Some(&configuration),
            tola_typst::PackageLocations::default(),
            &BuildOverrides::default(),
        )
        .expect("the site configuration loads")
        .into_config();
        published_path(&config, written)
    }

    #[test]
    fn output_lands_under_the_output_root() {
        assert_eq!(
            asset_output("downloads/file.pdf").ok().as_deref(),
            Some("public/downloads/file.pdf")
        );
        assert_eq!(
            asset_output("/downloads/file.pdf").ok().as_deref(),
            Some("public/downloads/file.pdf"),
            "a leading slash still names the site root"
        );
    }

    #[test]
    fn output_leaving_the_site_is_refused() {
        assert!(asset_output("../outside.pdf").is_err());
        assert!(asset_output("").is_err());
    }

    /// A site that declares one exact file and one tree, mounted below `/docs/`.
    fn site_with_declared_assets() -> (tempfile::TempDir, ResolvedSiteConfig) {
        let directory = tempfile::tempdir().expect("a site directory");
        let root = directory.path();
        std::fs::create_dir_all(root.join("brand")).expect("a tree directory");
        std::fs::write(root.join("brand/logo.svg"), b"<svg></svg>").expect("a tree member");
        std::fs::write(root.join("app.js"), "export {}").expect("a declared file");
        std::fs::write(root.join("图.svg"), b"<svg></svg>").expect("a declared file");
        let configuration = root.join("tola.toml");
        std::fs::write(
            &configuration,
            concat!(
                "[site]\nbase-path = \"/docs/\"\n\n",
                "[assets]\n",
                "files = [{ source = \"app.js\", url = \"/app.js\" }, ",
                "{ source = \"图.svg\", url = \"/%E5%9B%BE.svg\" }]\n",
                "trees = [{ source = \"brand\", url-prefix = \"/brand\" }]\n",
            ),
        )
        .expect("a configuration");
        let config = load_site_config(
            Some(&configuration),
            tola_typst::PackageLocations::default(),
            &BuildOverrides::default(),
        )
        .expect("the site configuration loads")
        .into_config();
        (directory, config)
    }

    fn published_urls(config: &ResolvedSiteConfig) -> AssetUrls {
        AssetUrls::for_check(config, &Default::default()).expect("the site's asset URLs resolve")
    }

    #[test]
    fn url_completion_replaces_written_url() {
        let (_directory, config) = site_with_declared_assets();
        let urls = published_urls(&config);
        let source = Source::detached("#asset-url(\"/brand/\")".to_owned());
        let written = source.text().find("/brand/").expect("the written url");
        let written = written..written + "/brand/".len();
        let cursor = written.end;

        let lsp_types::CompletionResponse::List(list) = url_completions(
            &urls,
            config.get_root(),
            &source,
            "/brand/",
            written,
            cursor,
        ) else {
            panic!("a completion list")
        };

        assert!(!list.is_incomplete, "a deploy-root URL withholds nothing");
        assert_eq!(
            list.items
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            ["/brand/logo.svg"]
        );
        let Some(lsp_types::CompletionTextEdit::InsertAndReplace(edit)) = &list.items[0].text_edit
        else {
            panic!("an insert-and-replace edit")
        };
        assert_eq!(edit.new_text, "/brand/logo.svg");
        assert_eq!(edit.insert.start, edit.replace.start);
        assert_eq!(
            edit.insert.end, edit.replace.end,
            "everything typed is replaced"
        );
    }

    #[test]
    fn candidates_offer_the_declared_urls() {
        let (_directory, config) = site_with_declared_assets();
        let urls = published_urls(&config);

        assert_eq!(candidates(&urls, "/brand/"), ["/brand/logo.svg"]);
        assert_eq!(
            candidates(&urls, "/"),
            ["/app.js", "/brand/logo.svg", "/图.svg"],
            "declared order, not the order the author typed"
        );
        assert!(candidates(&urls, "/nothing/").is_empty());
    }

    #[test]
    fn declared_url_resolves_to_its_address() {
        let (_directory, config) = site_with_declared_assets();
        let urls = published_urls(&config);

        let Written::Published(asset) = written(&urls, "/brand/logo.svg") else {
            panic!("the tree member is published")
        };
        assert_eq!(asset.declared, "/brand/logo.svg");
        assert_eq!(
            asset.address, "/docs/brand/logo.svg",
            "the key is the declaration and the address has the mount"
        );
        // The declaration spells `/図.svg` and is written here percent-encoded, which is how
        // the lookup decodes it too: one declaration, however it is spelled.
        let Written::Published(encoded) = written(&urls, "/%E5%9B%BE.svg") else {
            panic!("the encoded spelling names the declaration")
        };
        assert_eq!(encoded.declared, "/图.svg");
        assert!(matches!(
            written(&urls, "/brand/absent.svg"),
            Written::Unpublished
        ));
    }

    #[test]
    fn written_definition_keeps_the_client_spelling() {
        let (directory, config) = site_with_declared_assets();
        let urls = published_urls(&config);
        let source = Source::detached("#asset-url(\"/brand/logo.svg\")");
        let url = "/brand/logo.svg";
        let at = source.text().find(url).expect("the written URL");
        let spelled = directory.path().join("spelled");

        let definition = written_definition(
            &urls,
            &crate::uri::ClientRoot::with_resolved(&spelled, config.get_root()),
            &source,
            at..at + url.len(),
        );

        let Some(lsp_types::GotoDefinitionResponse::Scalar(location)) = definition else {
            panic!("the declared tree member is a definition");
        };
        assert_eq!(
            location.uri,
            crate::uri::from_file_path(&spelled.join("brand/logo.svg")).unwrap()
        );
    }

    /// The value a site imports as `asset-url` is the native the domain table names.
    ///
    /// The binding spells the function `asset-url`; the value itself is named `tola-asset-url`, and
    /// that identity is what the table keys on.
    #[test]
    fn asset_url_parameter_has_url_domain() {
        let library = SiteLibrary::new(HostInputs {
            site: Dict::new(),
            asset_urls: Dict::new(),
            asset_origins: Dict::new(),
            source_records: Array::new(),
            source_records_by_file: Dict::new(),
            source_origins: Dict::new(),
        });
        let library = library.shared();
        let Value::Module(sys) = library.global.scope().get("sys").unwrap().read() else {
            panic!("sys is a module")
        };
        let Value::Dict(inputs) = sys.scope().get("inputs").unwrap().read() else {
            panic!("sys.inputs is a dictionary")
        };
        let Value::Module(host) = inputs.get(HOST_MODULE).unwrap().clone() else {
            panic!("the host channel is a module")
        };
        let Value::Func(asset_url) = host.scope().get("asset-url").unwrap().read() else {
            panic!("asset-url is a function")
        };

        assert_eq!(asset_url.name(), Some("tola-asset-url"));
        assert_eq!(
            domain_of(asset_url, "declared-url"),
            Some(ArgumentDomain::SiteAssetUrl)
        );
        assert_eq!(domain_of(asset_url, "undeclared-parameter"), None);
    }

    #[test]
    fn completion_names_relative_asset_sources() {
        let (_directory, config) = site_with_declared_assets();
        let urls = published_urls(&config);
        let source = Source::detached("#asset-url(\"/\")");
        let start = source.text().find('/').unwrap();
        let lsp_types::CompletionResponse::List(list) = url_completions(
            &urls,
            config.get_root(),
            &source,
            "/",
            start..start + 1,
            start + 1,
        ) else {
            panic!("asset URL completions")
        };
        for (url, paths) in [
            ("/app.js", vec!["app.js"]),
            ("/brand/logo.svg", vec!["logo.svg", "brand"]),
            ("/图.svg", vec!["图.svg"]),
        ] {
            let detail = list
                .items
                .iter()
                .find(|item| item.label == url)
                .unwrap()
                .detail
                .as_ref()
                .unwrap();
            let named = detail.split('`').skip(1).step_by(2).collect::<Vec<_>>();
            assert_eq!(named, paths);
        }
    }
}
