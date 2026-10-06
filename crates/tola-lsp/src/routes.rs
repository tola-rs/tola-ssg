//! The routes a check realizes, including unsaved editor text: the pages one source is served at
//! and the site's whole route index.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use lsp_types::request::Request as LspRequest;
use serde::{Deserialize, Serialize};
use tola_build::cancellation::BuildCancellation;
use tola_build::check::{RealizedDocument, SourceCompilation, SourceDiagnosticSession};
use tola_build::config::ResolvedSiteConfig;
use tola_typst::typst::syntax::{FileId, Source, VirtualRoot};

use crate::compiler::RevisionCompilations;
use crate::protocol::{Route, RouteReply};

/// How many pages a source names before one entry stands for the rest.
pub(crate) const NAMED_PAGES: usize = 3;

/// The pages a source names, and how many one entry stands for.
pub(super) struct NamedPages<'a> {
    pub(super) named: &'a [Route],
    pub(super) remaining: usize,
}

/// A template many pages include would otherwise bury its own file, so a source names its first few
/// pages and one entry stands for the rest. Every client reads its pages from this selection, so a
/// source's pages read the same wherever they appear.
pub(super) fn named_pages(routes: &[Route]) -> NamedPages<'_> {
    let named = routes.len().min(NAMED_PAGES);
    NamedPages {
        named: &routes[..named],
        remaining: routes.len() - named,
    }
}

/// The documents a check realizes from `source`, with their configured addresses.
///
/// A template included by many documents answers with all of them. These describe checked
/// sources rather than a published revision; a failed compilation has no routes.
pub(super) fn respond(
    session: &mut SourceDiagnosticSession,
    compilations: &mut RevisionCompilations,
    config: &Arc<ResolvedSiteConfig>,
    source_revision: u64,
    overrides: &[(PathBuf, Arc<str>)],
    source: &Source,
    cancellation: &BuildCancellation,
) -> Result<RouteReply> {
    cancellation.ensure_active()?;
    let mut prepared = overrides.to_vec();
    crate::query::pin_source(source, Arc::from(source.text()), config, &mut prepared)?;
    let revision = compilations
        .inspect(session, config, source_revision, prepared, cancellation)
        .context("the site could not be checked")?;
    let Some(checked) = revision.checked() else {
        return Ok(RouteReply::default());
    };
    let routes = site_routes(checked, config, source.id(), cancellation)?;
    Ok(RouteReply { routes })
}

/// Every realized document whose body has `source`, in document order.
pub(super) fn site_routes(
    compilation: &SourceCompilation,
    config: &ResolvedSiteConfig,
    source: FileId,
    cancellation: &BuildCancellation,
) -> Result<Vec<Route>> {
    let addresses = Addresses::new(config);
    Ok(compilation
        .realized_documents(cancellation)?
        .into_iter()
        .filter(|document| document.sources.contains(&source))
        .map(|document| addresses.of(&document))
        .collect())
}

/// The addresses a site's realized documents answer at.
///
/// The site's origin derives from the configuration on every read, so it is read once for a whole
/// list of pages rather than once per page.
struct Addresses<'a> {
    config: &'a ResolvedSiteConfig,
    origin: bool,
}

impl<'a> Addresses<'a> {
    fn new(config: &'a ResolvedSiteConfig) -> Self {
        Self {
            config,
            origin: config.site_url().is_some(),
        }
    }

    /// The address `document` is served at.
    fn of(&self, document: &RealizedDocument) -> Route {
        Route {
            route: self.config.url_mount().browser_path(&document.permalink),
            url: self
                .origin
                .then(|| self.config.canonical_url(&document.permalink)),
            output: document.output.as_str().to_owned(),
        }
    }
}

/// One page the site realizes: the source that writes it, and its address.
#[derive(Debug, Deserialize, Serialize)]
pub(super) struct SiteRoute {
    /// The site-relative path of the page's own source; absent when the body has no source of
    /// the site's own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) source: Option<String>,
    #[serde(flatten)]
    pub(super) route: Route,
}

/// Every page the site realizes, in route order.
///
/// The index names what the checked sources compile to. Publication is the development server's
/// own fact, which the language server neither owns nor reads (LSP.md D1/D2), so no entry here
/// claims a published revision.
#[derive(Debug, Default, Deserialize, Serialize)]
pub(super) struct RouteIndexReply {
    pub(super) routes: Vec<SiteRoute>,
}

/// The route index request has no parameters; a client may send none or an empty object.
pub(super) enum RouteIndexRequest {}

impl LspRequest for RouteIndexRequest {
    type Params = serde_json::Value;
    type Result = RouteIndexReply;
    const METHOD: &'static str = "tola/routes";
}

/// One page the site realizes: the source that writes it, and the address it answers at.
pub(super) struct Page {
    /// The source the page is written in: the first source of the site's own that the page's body
    /// has.
    ///
    /// A page's body names its own source first and the templates it includes after it, so this
    /// is the page's own file rather than a template it shares with other pages. A body that
    /// names no source of the site's own — a document the site program writes itself — has none.
    pub(super) source: Option<FileId>,
    pub(super) route: Route,
}

/// Every page the check realizes, in document order.
pub(super) fn pages(
    compilation: &SourceCompilation,
    config: &ResolvedSiteConfig,
    cancellation: &BuildCancellation,
) -> Result<Vec<Page>> {
    realized_pages(compilation, config, None, cancellation)
}

/// The pages whose body has `source`, in document order.
///
/// A template included by many pages answers with all of them; a page answers with itself.
pub(super) fn pages_carrying(
    compilation: &SourceCompilation,
    config: &ResolvedSiteConfig,
    source: FileId,
    cancellation: &BuildCancellation,
) -> Result<Vec<Page>> {
    realized_pages(compilation, config, Some(source), cancellation)
}

/// The realized pages whose body has `carried`, or every realized page when it is `None`.
fn realized_pages(
    compilation: &SourceCompilation,
    config: &ResolvedSiteConfig,
    carried: Option<FileId>,
    cancellation: &BuildCancellation,
) -> Result<Vec<Page>> {
    let addresses = Addresses::new(config);
    Ok(compilation
        .realized_documents(cancellation)?
        .into_iter()
        .filter(|document| carried.is_none_or(|source| document.sources.contains(&source)))
        .map(|document| Page {
            source: document
                .sources
                .iter()
                .find(|id| matches!(id.root(), VirtualRoot::Project))
                .copied(),
            route: addresses.of(&document),
        })
        .collect())
}

/// Every page the compilation realizes, in route order.
pub(super) fn index(
    compilation: &SourceCompilation,
    config: &ResolvedSiteConfig,
    cancellation: &BuildCancellation,
) -> Result<RouteIndexReply> {
    let mut routes: Vec<SiteRoute> = pages(compilation, config, cancellation)?
        .into_iter()
        .map(|page| SiteRoute {
            source: page
                .source
                .map(|id| id.vpath().get_without_slash().to_owned()),
            route: page.route,
        })
        .collect();
    routes.sort_by(|left, right| {
        left.route
            .route
            .cmp(&right.route.route)
            .then_with(|| left.route.output.cmp(&right.route.output))
            .then_with(|| left.source.cmp(&right.source))
    });
    Ok(RouteIndexReply { routes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tola_build::config::loading::{BuildOverrides, load_site_config};
    use tola_build::diagnostic::Severity;

    /// A site whose entry program realizes one page per content source.
    ///
    /// Every file is written below the root; only files below `content` are sources, so a file
    /// elsewhere is readable without becoming a page of its own.
    fn site(files: &[(&str, &str)]) -> (tempfile::TempDir, Arc<ResolvedSiteConfig>) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::write(
            root.join("site.typ"),
            r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/address:0.0.0": route, route-to-output
#for source in all-sources() {
  document(route-to-output(route(source.route-segments)))[#include source.file]
}"#,
        )
        .unwrap();
        for (name, text) in files {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let toml = root.join("tola.toml");
        std::fs::write(&toml, "").unwrap();
        let configuration = Arc::new(
            load_site_config(
                Some(&toml),
                tola_typst::PackageLocations::default(),
                &BuildOverrides::default(),
            )
            .unwrap()
            .into_config(),
        );
        (directory, configuration)
    }

    /// The index of one `site`'s pages, asserted to come from a site that compiled.
    fn indexed_routes(configuration: &Arc<ResolvedSiteConfig>) -> Vec<SiteRoute> {
        let cancellation = BuildCancellation::new();
        let mut session = SourceDiagnosticSession::new(Arc::clone(configuration));
        let revision = session.inspect(Vec::new(), &cancellation).unwrap();
        assert!(
            revision
                .diagnostics()
                .iter()
                .all(|diagnostic| diagnostic.severity != Severity::Error),
            "{:#?}",
            revision.diagnostics()
        );
        index(
            revision.checked().expect("the site compiles"),
            configuration,
            &cancellation,
        )
        .unwrap()
        .routes
    }

    fn listed(pages: &[SiteRoute]) -> Vec<(Option<&str>, &str, &str)> {
        pages
            .iter()
            .map(|page| {
                (
                    page.source.as_deref(),
                    page.route.route.as_str(),
                    page.route.output.as_str(),
                )
            })
            .collect()
    }

    #[test]
    fn index_names_each_page_by_its_own_source() {
        let (_directory, configuration) = site(&[
            (
                "content/one.typ",
                "#include \"../templates/note.typ\"\nOne.\n",
            ),
            (
                "content/two.typ",
                "#include \"../templates/note.typ\"\nTwo.\n",
            ),
            ("templates/note.typ", "Note.\n"),
        ]);

        assert_eq!(
            listed(&indexed_routes(&configuration)),
            [
                (Some("content/one.typ"), "/one/", "one/index.html"),
                (Some("content/two.typ"), "/two/", "two/index.html"),
            ]
        );
    }

    #[test]
    fn index_orders_pages_by_route() {
        let (_directory, configuration) =
            site(&[("content/two.typ", "Two.\n"), ("content/one.typ", "One.\n")]);

        assert_eq!(
            listed(&indexed_routes(&configuration)),
            [
                (Some("content/one.typ"), "/one/", "one/index.html"),
                (Some("content/two.typ"), "/two/", "two/index.html"),
            ]
        );
    }
}
