//! Preview commands for pages realized by a source check.
//!
//! A shared template names its first few pages and offers a chooser for the rest.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use lsp_types::{CodeLens, Command, Position, Range, Uri};
use tola_build::cancellation::BuildCancellation;
use tola_build::check::SourceDiagnosticSession;
use tola_build::config::ResolvedSiteConfig;
use tola_typst::typst::syntax::{FileId, Source};

use crate::compiler::RevisionCompilations;

/// The lenses one source has, or `None` when its check realizes no page.
#[expect(
    clippy::too_many_arguments,
    reason = "a lens reads session state, revision inputs, the source, its client spelling, and cancellation"
)]
pub(super) fn respond(
    session: &mut SourceDiagnosticSession,
    compilations: &mut RevisionCompilations,
    config: &Arc<ResolvedSiteConfig>,
    source_revision: u64,
    client_root: &Path,
    overrides: &[(PathBuf, Arc<str>)],
    source: &Source,
    cancellation: &BuildCancellation,
) -> Result<Option<Vec<CodeLens>>> {
    let routes = crate::routes::respond(
        session,
        compilations,
        config,
        source_revision,
        overrides,
        source,
        cancellation,
    )?
    .routes;
    if routes.is_empty() {
        return Ok(None);
    }
    let client_root = crate::uri::ClientRoot::new(client_root);
    let uri = document_uri(source.id(), config, &client_root)?;
    let pages = crate::routes::named_pages(&routes);
    let mut lenses: Vec<CodeLens> = pages
        .named
        .iter()
        .map(|route| {
            lens(
                &uri,
                format!("Save and preview {}", route.route),
                Some(route.route.clone()),
            )
        })
        .collect();
    if pages.remaining > 0 {
        lenses.push(lens(
            &uri,
            format!("Choose from {} pages", routes.len()),
            None,
        ));
    }
    Ok(Some(lenses))
}

/// The document a lens opens: the source's own file, addressed in the spelling the client named.
fn document_uri(
    id: FileId,
    config: &ResolvedSiteConfig,
    client_root: &crate::uri::ClientRoot,
) -> Result<Uri> {
    let root = tola_build::filesystem::normalize_existing_prefix(config.get_root());
    client_root.address(&root.join(id.vpath().get_without_slash()))
}

/// The lens that opens one page, or asks which page when it names none.
fn lens(uri: &Uri, title: String, route: Option<String>) -> CodeLens {
    let mut arguments = vec![serde_json::Value::String(uri.as_str().to_owned())];
    // A client that runs no command still reads what the lens names, so the lens has the route
    // it opens as data; where the site is served is the connection's own fact, and the connection
    // fills the address in when it knows one.
    let data = route
        .as_deref()
        .map(|route| crate::protocol::preview_lens_data(route, None));
    if let Some(route) = route {
        arguments.push(serde_json::Value::String(route));
    }
    CodeLens {
        range: Range::new(Position::new(0, 0), Position::new(0, 0)),
        command: Some(Command {
            title,
            command: "tola.openPreview".to_owned(),
            arguments: Some(arguments),
        }),
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_uri_follows_client_root_spelling() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let spelled = directory.path().join("client");
        std::fs::write(root.join("tola.toml"), "").unwrap();
        std::fs::create_dir_all(root.join("content")).unwrap();
        let config = tola_build::config::loading::load_site_config(
            Some(&root.join("tola.toml")),
            tola_typst::PackageLocations::default(),
            &tola_build::config::loading::BuildOverrides::default(),
        )
        .unwrap()
        .into_config();
        let id = crate::identity::file_id(
            &crate::uri::from_file_path(&root.join("content/document.typ")).unwrap(),
            config.get_root(),
        )
        .unwrap();
        assert_eq!(
            document_uri(
                id,
                &config,
                &crate::uri::ClientRoot::with_resolved(&spelled, &root),
            )
            .unwrap(),
            crate::uri::from_file_path(&spelled.join("content/document.typ")).unwrap()
        );
    }

    #[test]
    fn lens_names_the_route_it_opens() {
        let uri: Uri = "file:///site/page.typ".parse().unwrap();
        let named = lens(
            &uri,
            "Save and preview /page/".to_owned(),
            Some("/page/".to_owned()),
        );
        assert_eq!(named.data, Some(serde_json::json!({ "route": "/page/" })));
        let offered = lens(&uri, "Choose from 2 pages".to_owned(), None);
        assert_eq!(offered.data, None);
    }
}
