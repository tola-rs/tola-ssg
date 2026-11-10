//! Description-only projections of the stages one build attempt completed.

use serde::Serialize;
use serde_json::{Value, json};

use crate::cancellation::{BuildCancellation, BuildCancelled};
use crate::site::Resource;
use crate::site::references::ReferenceResolution;

/// A missing projection was not completed; an empty projection was completed without records.
/// This value contains no output bytes and cannot be published as a site.
#[derive(Debug, Clone, Default, Serialize)]
pub struct BuildReport {
    pub pages: Option<Vec<Value>>,
    pub routes: Option<Vec<Value>>,
    pub outputs: Option<Vec<Value>>,
    pub references: Option<Vec<Value>>,
}

impl BuildReport {
    pub(super) fn record_outputs(
        &mut self,
        graph: &crate::output::graph::OutputGraph,
        cancellation: &BuildCancellation,
    ) -> Result<(), BuildCancelled> {
        self.outputs = Some(graph.outputs().iter().map(|output| {
            cancellation.ensure_active()?;
            Ok(json!({
                "output": output.path().as_str(),
                "kind": output.kind().as_str(),
                "semantics": output.declaration().semantics().as_str(),
                "media-type": output.declaration().media_type().as_str(),
                "producer": output.owner().to_string(),
                "digest": output.digest().to_hex().to_string(),
                "representation": crate::output::manifest::RepresentationId::from_output(output).to_hex(),
            }))
        }).collect::<Result<Vec<_>, BuildCancelled>>()?);
        Ok(())
    }

    pub(super) fn record_index(
        &mut self,
        index: &crate::site::SiteIndex,
        root: &std::path::Path,
        cancellation: &BuildCancellation,
    ) -> Result<(), BuildCancelled> {
        self.pages = Some(index.address().pages().into_iter().map(|page| {
            cancellation.ensure_active()?;
            Ok(json!({
                "permalink": page.permalink.as_str(),
                "output": page.output.as_str(),
                "properties": {
                    "title": page.properties.title.as_deref(),
                    "description": page.properties.description.as_deref(),
                    "authors": page.properties.author.iter().map(|author| author.as_str()).collect::<Vec<_>>(),
                    "keywords": page.properties.keywords.iter().map(|keyword| keyword.as_str()).collect::<Vec<_>>(),
                    "date": match page.properties.date {
                        typst::foundations::Smart::Auto => json!("auto"),
                        typst::foundations::Smart::Custom(date) => json!(date.map(typst::foundations::Value::Datetime)),
                    },
                },
            }))
        }).collect::<Result<Vec<_>, BuildCancelled>>()?);
        self.routes = Some(index.address().resources().into_iter().map(|(url, resource)| {
            cancellation.ensure_active()?;
            Ok(match resource {
                Resource::Page { document } => json!({
                    "url": url.as_str(),
                    "kind": "document",
                    "output": document.output.as_str(),
                }),
                Resource::Asset { route } => json!({
                    "url": url.as_str(),
                    "kind": "asset",
                    "source": route.source.as_deref().map(|path| tola_diagnostic::display_path(path, root)),
                }),
            })
        }).collect::<Result<Vec<_>, BuildCancelled>>()?);
        Ok(())
    }

    pub(super) fn record_references(
        &mut self,
        references: &crate::site::references::References,
        cancellation: &BuildCancellation,
    ) -> Result<(), BuildCancelled> {
        self.references = Some(
            references
                .references()
                .map(|reference| {
                    cancellation.ensure_active()?;
                    let resolution = match reference.resolution() {
                        ReferenceResolution::Found { target } => json!({
                            "status": "found",
                            "url": target.url().as_str(),
                            "query": target.query(),
                            "fragment": target.fragment(),
                        }),
                        ReferenceResolution::External => json!({"status": "external"}),
                        ReferenceResolution::Unresolved { reason } => json!({
                            "status": "unresolved",
                            "reason": reason.message(reference.category()),
                            "help": reason.help(reference.category()),
                        }),
                    };
                    Ok(json!({
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
                    }))
                })
                .collect::<Result<Vec<_>, BuildCancelled>>()?,
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn report_output_kinds_media_types() {
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
        let request = crate::build::BuildRequest {
            capture_report: true,
            generated_files: vec![stylesheet],
            ..crate::build::BuildRequest::default()
        };
        let built = crate::BuildSession::new()
            .prepare(std::sync::Arc::new(config), request)
            .run()
            .map_err(|failure| failure.into_error())
            .unwrap();

        let rows = built.report().unwrap().outputs.as_ref().unwrap();
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
