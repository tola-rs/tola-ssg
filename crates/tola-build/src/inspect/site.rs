//! Shared JSON projections of complete site builds.

use serde_json::{Value, json};

use crate::build::SiteBuild;
use crate::filesystem::display_path;
use crate::site::Resource;
use crate::site::references::ReferenceResolution;

/// Project realized document addresses and properties in permalink order.
pub fn documents(build: &SiteBuild) -> Value {
    Value::Array(
        build
            .index()
            .address()
            .pages()
            .into_iter()
            .map(|document| {
                json!({
                    "permalink": document.permalink.as_str(),
                    "output": document.output.as_str(),
                    "properties": {
                        "title": document.properties.title.as_deref(),
                        "description": document.properties.description.as_deref(),
                        "authors": document.properties.author.iter().map(|author| author.as_str()).collect::<Vec<_>>(),
                        "keywords": document.properties.keywords.iter().map(|keyword| keyword.as_str()).collect::<Vec<_>>(),
                        "date": match document.properties.date {
                            typst::foundations::Smart::Auto => json!("auto"),
                            typst::foundations::Smart::Custom(date) => json!(date.map(typst::foundations::Value::Datetime)),
                        },
                    },
                })
            })
            .collect(),
    )
}

/// Project realized document and asset routes in URL order.
pub fn routes(build: &SiteBuild) -> Value {
    let root = build.config().get_root();
    Value::Array(
        build
            .index()
            .address()
            .resources()
            .into_iter()
            .map(|(url, resource)| match resource {
                Resource::Page { document } => json!({
                    "url": url.as_str(),
                    "kind": "document",
                    "output": document.output.as_str(),
                }),
                Resource::Asset { route } => json!({
                    "url": url.as_str(),
                    "kind": "asset",
                    "source": route
                        .source
                        .as_deref()
                        .map(|path| display_path(path, root)),
                }),
            })
            .collect(),
    )
}

/// Project every output in the sealed graph with its declared semantics and representation.
///
/// HTML pages, paged documents (PDF, PNG, SVG), and assets are all listed; each row
/// reports the output's own kind.
pub fn outputs(build: &SiteBuild) -> Value {
    Value::Array(
        build
            .graph()
            .outputs()
            .iter()
            .map(|output| {
                json!({
                    "output": output.path().as_str(),
                    "kind": output.kind().as_str(),
                    "semantics": output.declaration().semantics().as_str(),
                    "media-type": output.declaration().media_type().as_str(),
                    "producer": output.owner().to_string(),
                    "digest": output.digest().to_hex().to_string(),
                    "representation": crate::output::manifest::RepresentationId::from_output(output).to_hex(),
                })
            })
            .collect(),
    )
}

/// Project discovered document references and their resolution outcomes.
pub fn references(build: &SiteBuild) -> Value {
    Value::Array(
        build
            .references()
            .references()
            .map(|reference| {
                let resolution = match reference.resolution() {
                    ReferenceResolution::Found { target } => json!({
                        "status": "found",
                        "url": target.url().as_str(),
                        "query": target.query(),
                        "fragment": target.fragment(),
                    }),
                    ReferenceResolution::External => json!({ "status": "external" }),
                    ReferenceResolution::Unresolved { reason } => json!({
                        "status": "unresolved",
                        "reason": reason.message(reference.category()),
                        "help": reason.help(reference.category()),
                    }),
                };
                json!({
                    "document": reference.page().as_str(),
                    "destination": reference.destination(),
                    "html_context": reference.html_context(),
                    "category": reference.category().as_str(),
                    "origin": reference.origin().map(|origin| json!({
                        "path": origin.path,
                        "line": origin.line,
                        "column": origin.column,
                    })),
                    "resolution": resolution,
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspected_outputs_list_every_graph_output_once() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("content")).unwrap();
        std::fs::write(
            directory.path().join("site.typ"),
            r#"#document("index.html")[Home]
#document("paper.pdf")[#rect(width: 1pt, height: 1pt)]"#,
        )
        .unwrap();
        let config = crate::config::SiteConfigSchema::default()
            .resolve(
                &directory.path().join("tola.toml"),
                tola_typst::PackageLocations::default(),
                &crate::config::loading::BuildOverrides::default(),
            )
            .unwrap();
        let stylesheet = crate::output::GeneratedFile::new(
            "styles",
            tola_address::OutputPath::parse("styles/app").unwrap(),
            b"body {}".to_vec(),
        )
        .unwrap()
        .with_media_type(crate::output::semantics::ResponseMediaType::CSS);
        let mut request = crate::build::BuildRequest::default();
        request.generated_files.push(stylesheet);
        let built = crate::BuildSession::new()
            .prepare(std::sync::Arc::new(config), request)
            .run()
            .map_err(|failure| failure.into_error())
            .unwrap();

        let outputs_json = outputs(&built);
        let rows = outputs_json.as_array().unwrap();
        let projected_paths = rows
            .iter()
            .map(|row| row["output"].as_str().unwrap())
            .collect::<Vec<_>>();
        let graph_paths = built
            .graph()
            .outputs()
            .iter()
            .map(|output| output.path().as_str())
            .collect::<Vec<_>>();
        assert_eq!(projected_paths, graph_paths);

        let page = rows
            .iter()
            .find(|row| row["output"] == "index.html")
            .expect("the HTML page is listed");
        assert_eq!(page["kind"], "html-document");
        assert_eq!(page["media-type"], "text/html; charset=utf-8");
        assert_eq!(page["producer"], "the root Bundle");
        let paper = rows
            .iter()
            .find(|row| row["output"] == "paper.pdf")
            .expect("the PDF document is listed");
        assert_eq!(paper["kind"], "pdf-document");
        assert_eq!(paper["media-type"], "application/pdf");
        let stylesheet = rows
            .iter()
            .find(|row| row["output"] == "styles/app")
            .expect("the generated stylesheet is listed");
        assert_eq!(stylesheet["kind"], "asset");
        assert_eq!(stylesheet["semantics"], "opaque");
        assert_eq!(stylesheet["media-type"], "text/css; charset=utf-8");
    }
}
