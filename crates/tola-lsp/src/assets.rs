//! Asset output paths and declared `asset-url` resolution/completion.

use std::path::Path;

use lsp_types::Hover;
use tola_build::config::SiteConfig;
use tola_build::config::section::assets::{AssetUrl, AssetUrlError};
use tola_build::{AssetOrigin, AssetUrls, PublishedAsset};
use tola_packages::{ARGUMENT_DOMAINS, ArgumentDomain};
use tola_typst::typst::foundations::Func;
use tola_typst::typst::syntax::Source;

use crate::position;

/// Hover summary for an `asset(…)` site-root path argument at `at`.
pub(super) fn hover(cfg: &SiteConfig, src: &Source, at: usize) -> Option<Hover> {
    let arg = tola_typst_syntax::syntax::path_argument(src, at)?;
    if arg.base != tola_typst_syntax::syntax::PathBase::Site {
        return None;
    }
    let range = arg.range;
    let written = src.text().get(range.clone())?;
    let desc = match published_path(cfg, written) {
        Ok(out) => format!("Published at `{out}`."),
        Err(rule) => {
            tracing::debug!(%rule, "asset argument names no path the site publishes");
            format!("`{written}` is not a path this site publishes")
        }
    };
    Some(crate::protocol::markdown_hover(
        desc,
        Some(position::utf16_range(src.lines(), range)?),
    ))
}

fn published_path(cfg: &SiteConfig, written: &str) -> Result<String, AssetUrlError> {
    let declared = format!("/{}", written.trim_start_matches('/'));
    let asset = AssetUrl::parse(&declared)?;
    let out = cfg
        .root()
        .join(&cfg.build().publish_dir)
        .join(asset.output_path().as_str());
    Ok(tola_diagnostic::display_path(&out, cfg.root()))
}

/// Resolution of a written site-root URL against declared site assets.
pub(super) enum Written<'a> {
    Published(PublishedAsset<'a>),
    Unpublished,
}

/// Resolve a written URL by its decoded declaration spelling.
pub(super) fn written<'a>(urls: &'a AssetUrls, text: &str) -> Written<'a> {
    let declared = decoded_spelling(text);
    urls.published()
        .find(|asset| asset.declared == declared)
        .map_or(Written::Unpublished, Written::Published)
}

fn matching_declared_urls<'a>(urls: &'a AssetUrls, typed: &str) -> Vec<&'a str> {
    let typed = decoded_spelling(typed);
    urls.published()
        .map(|asset| asset.declared)
        .filter(|declared| declared.starts_with(typed.as_str()))
        .collect()
}

/// Domain declared for `param` of `callee`, matched by function identity rather than call-site alias.
pub(super) fn domain_of(callee: &Func, param: &str) -> Option<ArgumentDomain> {
    let name = callee.name()?;
    ARGUMENT_DOMAINS
        .iter()
        .find(|row| row.function == name && row.parameter == param)
        .map(|row| row.domain)
}

/// Completions for a written asset-URL argument, keeping inventory order.
pub(super) fn url_completions(
    urls: &AssetUrls,
    site_root: &Path,
    src: &Source,
    typed: &str,
    written: std::ops::Range<usize>,
    cursor: usize,
) -> lsp_types::CompletionResponse {
    let Some(replace) = position::utf16_range(src.lines(), written.clone()) else {
        return lsp_types::CompletionResponse::Array(Vec::new());
    };
    let Some(insert) = position::utf16_range(src.lines(), written.start..cursor) else {
        return lsp_types::CompletionResponse::Array(Vec::new());
    };
    let items = matching_declared_urls(urls, typed)
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
        // Deploy-root URLs hold no relative navigation, so no candidate is withheld.
        is_incomplete: false,
        items,
    })
}

/// Definition location for a written asset URL, if declared.
pub(super) fn written_definition(
    urls: &AssetUrls,
    client_root: &crate::uri::ClientRoot,
    src: &Source,
    written_range: std::ops::Range<usize>,
) -> Option<lsp_types::GotoDefinitionResponse> {
    let text = src.text().get(written_range)?;
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

/// Author-facing note naming the file or tree that declared an asset.
pub(super) fn declaration_note(origin: &AssetOrigin, site_root: &Path) -> String {
    match origin {
        AssetOrigin::File { source } => {
            format!("`{}`", tola_diagnostic::display_path(source, site_root))
        }
        AssetOrigin::TreeMember { source, member } => {
            format!(
                "`{}` in the tree `{}`",
                tola_diagnostic::display_path(member, site_root),
                tola_diagnostic::display_path(source, site_root)
            )
        }
    }
}

fn decoded_spelling(text: &str) -> String {
    // Keep invalid partial escapes literal while the author is typing.
    AssetUrl::parse(text)
        .map(|url| url.as_str().to_owned())
        .unwrap_or_else(|_| text.to_owned())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn asset_output(written: &str) -> Result<String, AssetUrlError> {
        let dir = tempfile::tempdir().expect("a site directory");
        let cfg = crate::compiler::tests::site_configuration(dir.path(), "");
        published_path(&cfg, written)
    }

    #[test]
    fn asset_output_under_root() {
        for (written, expected) in [
            ("downloads/file.pdf", Some("public/downloads/file.pdf")),
            ("/downloads/file.pdf", Some("public/downloads/file.pdf")),
            ("../outside.pdf", None),
            ("", None),
        ] {
            match expected {
                Some(path) => assert_eq!(
                    asset_output(written).ok().as_deref(),
                    Some(path),
                    "{written:?}"
                ),
                None => assert!(asset_output(written).is_err(), "{written:?}"),
            }
        }
    }

    fn site_with_assets() -> (tempfile::TempDir, Arc<SiteConfig>) {
        let dir = tempfile::tempdir().expect("a site directory");
        let root = dir.path();
        std::fs::create_dir_all(root.join("brand")).expect("a tree directory");
        std::fs::write(root.join("brand/logo.svg"), b"<svg></svg>").expect("a tree member");
        std::fs::write(root.join("app.js"), "export {}").expect("a declared file");
        std::fs::write(root.join("图.svg"), b"<svg></svg>").expect("a declared file");
        let cfg = crate::compiler::tests::site_configuration(
            root,
            concat!(
                "[site]\nbase-path = \"/docs/\"\n\n",
                "[assets]\n",
                "files = [{ source = \"app.js\", url = \"/app.js\" }, ",
                "{ source = \"图.svg\", url = \"/%E5%9B%BE.svg\" }]\n",
                "trees = [{ source = \"brand\", url-prefix = \"/brand\" }]\n",
            ),
        );
        (dir, cfg)
    }

    fn published_urls(cfg: &SiteConfig) -> AssetUrls {
        AssetUrls::for_check_project(cfg, &Default::default(), &Default::default())
            .expect("the site's asset URLs resolve")
    }

    #[test]
    fn url_completion_replaces_written() {
        let (_dir, cfg) = site_with_assets();
        let urls = published_urls(&cfg);
        let src = Source::detached("#asset-url(\"/brand/\")".to_owned());
        let span = src.text().find("/brand/").expect("the written url");
        let span = span..span + "/brand/".len();
        let cursor = span.end;

        let lsp_types::CompletionResponse::List(list) =
            url_completions(&urls, cfg.root(), &src, "/brand/", span, cursor)
        else {
            panic!("a completion list")
        };

        assert!(!list.is_incomplete);
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
        assert_eq!(edit.insert.end, edit.replace.end);
    }

    #[test]
    fn declared_url_resolves() {
        let (_dir, cfg) = site_with_assets();
        let urls = published_urls(&cfg);

        let Written::Published(asset) = written(&urls, "/brand/logo.svg") else {
            panic!("the tree member is published")
        };
        assert_eq!(asset.declared, "/brand/logo.svg");
        assert_eq!(asset.address, "/docs/brand/logo.svg");

        let Written::Published(encoded) = written(&urls, "/%E5%9B%BE.svg") else {
            panic!("the encoded spelling names the declaration")
        };
        assert_eq!(encoded.declared, "/图.svg");
        assert!(matches!(
            written(&urls, "/brand/absent.svg"),
            Written::Unpublished
        ));
    }
}
