//! Source analysis followed by root Bundle compilation.
//!
//! Source metadata converges before the root program receives its frozen
//! sources. Document identity and contextual queries belong to Typst's
//! native Bundle introspection.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tola_typst::BundleOptions;

use crate::compiler::TypstHost;
use crate::config::ResolvedSiteConfig;
use crate::content::{ContentUnit, SourceSet};
use tola_address::OutputPath;

pub(crate) struct EvaluatedSiteProgram {
    sources: SourceSet,
    source_analysis: crate::compiler::analysis::SourceAnalysisCache,
    source_reads: Vec<(
        PathBuf,
        Vec<tola_typst::FileRead>,
        Vec<tola_typst::PackageCheck>,
    )>,
    candidate_files: Arc<tola_typst::FileSnapshot>,
    package_bindings: crate::package::SiteBindings,
}

/// A compiled Bundle and the world used to resolve its diagnostics.
pub(crate) struct RealizedBundle {
    pub(crate) compilation: tola_typst::BundleCompilation,
    pub(crate) world: Arc<tola_typst::TypstWorld>,
}

/// A root Bundle that did not realize, with the world that could not realize it.
///
/// The world resolves the same imports, packages, and fonts the failed compilation read, so an
/// editor answers about a file the site's own program never reached.
pub(crate) struct RealizeFailure {
    /// The world compiled against, absent when the world itself could not be built.
    pub(crate) world: Option<Arc<tola_typst::TypstWorld>>,
    /// The failure the caller reports.
    pub(crate) error: anyhow::Error,
}

impl RealizeFailure {
    /// The failure a caller that needs a compiled Bundle reports.
    pub(crate) fn into_error(self) -> anyhow::Error {
        self.error
    }
}

impl std::fmt::Debug for RealizeFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RealizeFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

pub(crate) struct CompiledSiteProgram {
    pub(crate) document_outputs: BTreeSet<OutputPath>,
    pub(crate) documents: Vec<crate::site::HtmlPage>,
    pub(crate) html_inventories:
        std::collections::BTreeMap<OutputPath, Arc<tola_typst::HtmlDocumentInventory>>,
    pub(crate) diagnostics: tola_typst::Diagnostics,
    pub(crate) bundle_entries: tola_typst::BundleEntries,
    pub(crate) compilation: Arc<tola_typst::BundleCompilation>,
    pub(crate) world: Arc<tola_typst::TypstWorld>,
}

pub(crate) fn evaluate(
    config: &ResolvedSiteConfig,
    host: &TypstHost,
    content: &[ContentUnit],
    package_bindings: crate::package::SiteBindings,
    cancellation: &tola_typst::BundleCancellation,
    source_analysis_reuse: crate::compiler::analysis::SourceAnalysisReuse<'_>,
    inputs: &mut crate::compiler::BuildInputs,
) -> Result<EvaluatedSiteProgram, crate::compiler::analysis::SourceAnalysisFailure> {
    let scan = crate::compiler::analysis::analyze(
        config,
        host,
        content,
        package_bindings,
        source_analysis_reuse,
        cancellation,
        inputs,
    )?;
    let source_analysis = crate::compiler::analysis::SourceAnalysisCache::from_scan(&scan);
    let source_reads = scan.dependency_reads();
    let package_bindings = scan.package_bindings().clone();
    Ok(EvaluatedSiteProgram {
        sources: scan.source_set,
        source_analysis,
        source_reads,
        candidate_files: Arc::clone(&scan.candidate_files),
        package_bindings,
    })
}

/// Export one fully bound native Bundle and derive inventories from that same DOM.
pub(crate) fn export(
    config: &ResolvedSiteConfig,
    compilation: &Arc<tola_typst::BundleCompilation>,
    world: &Arc<tola_typst::TypstWorld>,
    cancellation: &tola_typst::BundleCancellation,
    previous_bundle_entries: Option<&tola_typst::BundleEntries>,
) -> Result<CompiledSiteProgram> {
    let started = std::time::Instant::now();
    let (documents, bundle_entries) = rayon::join(
        || -> Result<_> {
            let realized = crate::compiler::documents::from_compilation(compilation, cancellation)?;
            let document_outputs = compilation
                .outputs()
                .filter(|(_, kind)| kind.is_document())
                .map(|(path, _)| crate::compiler::documents::output_path_for_virtual(path))
                .collect::<Result<BTreeSet<_>>>()?;
            Ok((realized, document_outputs))
        },
        || {
            compilation.export_entries(
                &export_options(config),
                cancellation,
                previous_bundle_entries,
            )
        },
    );
    cancellation.ensure_active()?;
    let (realized, document_outputs) = documents?;
    let bundle_entries = bundle_entries.map_err(|error| {
        super::diagnostic::with_export_diagnostics(error, world, compilation, config.get_root())
    })?;
    tracing::debug!(target: "tola::compile",
        bundle_outputs_ms = started.elapsed().as_secs_f64() * 1000.0,
        "finished immutable Bundle outputs");
    Ok(CompiledSiteProgram {
        document_outputs,
        documents: realized.documents,
        html_inventories: realized.html_inventories,
        diagnostics: compilation.diagnostics().clone(),
        bundle_entries,
        compilation: Arc::clone(compilation),
        world: Arc::clone(world),
    })
}

/// Compile the root Bundle without exporting files or collecting other producers.
pub(crate) fn realize(
    config: &ResolvedSiteConfig,
    host: &TypstHost,
    cancellation: &tola_typst::BundleCancellation,
    evaluated: &EvaluatedSiteProgram,
    inputs: &mut crate::compiler::BuildInputs,
) -> Result<RealizedBundle, RealizeFailure> {
    let library = evaluated
        .package_bindings
        .library(evaluated.sources.inputs().to_source_records());
    let world = match host.world(
        config.get_root(),
        &config.build.entry,
        &library,
        Arc::clone(&evaluated.candidate_files),
        cancellation,
    ) {
        Ok(world) => Arc::new(world),
        Err(error) => {
            return Err(RealizeFailure {
                world: None,
                error: tola_typst::CompileError::from(error).into(),
            });
        }
    };
    let realized = (|| -> Result<RealizedBundle> {
        cancellation.ensure_active().map_err(anyhow::Error::new)?;
        let mut compilation = match tola_typst::compile_bundle_world(&world, cancellation) {
            Ok(compilation) => {
                inputs.record_accessed(compilation.accessed());
                compilation
            }
            Err(failure) => {
                inputs.record_accessed(failure.accessed());
                return Err(anyhow::Error::new(failure));
            }
        };
        cancellation.ensure_active().map_err(anyhow::Error::new)?;
        crate::compiler::html::align_inline_frames(&mut compilation, cancellation)
            .map_err(|error| match error {
                tola_typst::HtmlFrameStyleError::Cancelled => tola_typst::CompileError::Cancelled,
                tola_typst::HtmlFrameStyleError::Rejected { diagnostics } => {
                    tola_typst::CompileError::compilation(world.as_ref(), diagnostics)
                }
            })
            .context("Tola could not align the inline frames the site renders")?;
        cancellation.ensure_active().map_err(anyhow::Error::new)?;
        Ok(RealizedBundle {
            compilation,
            world: Arc::clone(&world),
        })
    })();
    realized.map_err(|error| RealizeFailure {
        error: super::diagnostic::with_diagnostics(
            error,
            config.get_root(),
            crate::codes::typst::SITE_PROGRAM,
        ),
        world: Some(world),
    })
}

impl EvaluatedSiteProgram {
    pub(crate) fn sources(&self) -> &SourceSet {
        &self.sources
    }

    pub(crate) fn source_read_count(&self) -> usize {
        self.source_reads.len()
    }

    pub(crate) fn dependency_readers<'a>(
        &'a self,
        compilation: &'a tola_typst::BundleCompilation,
    ) -> impl Iterator<
        Item = (
            crate::compiler::TypstDependencyReader,
            Vec<tola_typst::FileRead>,
            Vec<tola_typst::PackageCheck>,
        ),
    > + 'a {
        self.source_reads
            .iter()
            .map(|(source, reads, checks)| {
                (
                    crate::compiler::TypstDependencyReader::ContentSource(source.clone()),
                    reads.clone(),
                    checks.clone(),
                )
            })
            .chain(std::iter::once_with(|| {
                (
                    crate::compiler::TypstDependencyReader::SiteProgram,
                    compilation.file_reads().to_vec(),
                    compilation.package_checks().to_vec(),
                )
            }))
    }

    pub(crate) fn package_checks<'a>(
        &'a self,
        compilation: &'a tola_typst::BundleCompilation,
    ) -> impl Iterator<Item = &'a tola_typst::PackageCheck> + 'a {
        self.source_reads
            .iter()
            .flat_map(|(_, _, checks)| checks)
            .chain(compilation.package_checks())
    }

    pub(crate) fn source_analysis(&self) -> &crate::compiler::analysis::SourceAnalysisCache {
        &self.source_analysis
    }
}

fn export_options(config: &ResolvedSiteConfig) -> BundleOptions {
    let mut options = BundleOptions::default();
    options.html.pretty = !config.build.minify.html;
    options
}

#[cfg(test)]
mod tests {

    use crate::compiler::tests::compiler_host;
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn compiled(source: &str) -> (TempDir, CompiledSiteProgram) {
        let directory = TempDir::new().unwrap();
        let compiled = compile_sources(&directory, source, &[], "").unwrap();
        (directory, compiled)
    }

    fn compile_sources(
        directory: &TempDir,
        source: &str,
        content: &[(&str, &str)],
        configuration: &str,
    ) -> Result<CompiledSiteProgram> {
        let root = directory.path();
        fs::create_dir_all(root.join("content"))?;
        for (path, text) in content {
            let file = root.join("content").join(path);
            fs::create_dir_all(file.parent().unwrap())?;
            fs::write(file, text)?;
        }
        let config = crate::config::tests::load_test_config(root, configuration);
        fs::write(&config.build.entry, source)?;
        fs::write(root.join("value.txt"), "payload")?;
        let content = crate::content::discover_content_units_for_config_with_cancellation(
            &config,
            &crate::cancellation::BuildCancellation::default(),
        )?;
        let host = compiler_host(&config)?;
        let cancellation = tola_typst::BundleCancellation::new();
        let mut inputs = crate::compiler::BuildInputs::default();
        let evaluated = evaluate(
            &config,
            &host,
            &content,
            crate::package::SiteBindings::from_config(&config, Default::default()),
            &cancellation,
            crate::compiler::analysis::SourceAnalysisReuse::None,
            &mut inputs,
        )
        .map_err(crate::compiler::analysis::SourceAnalysisFailure::into_error)?;
        let realized = realize(&config, &host, &cancellation, &evaluated, &mut inputs)
            .map_err(RealizeFailure::into_error)?;
        export(
            &config,
            &Arc::new(realized.compilation),
            &realized.world,
            &cancellation,
            None,
        )
    }

    fn html(compiled: &CompiledSiteProgram, path: &str) -> String {
        let path = typst::syntax::VirtualPath::new(path).unwrap();
        let entry = compiled.bundle_entries.get(&path).unwrap();
        String::from_utf8(entry.bytes().to_vec()).unwrap()
    }

    #[test]
    fn current_document_scopes_heading_queries() {
        let (_directory, compiled) = compiled(
            r#"#import "@tola/document:0.0.0": current-document
#document("guide/index.html")[
= Guide
#context {
  let page = current-document()
  let titles = query(selector(heading).within(page.location)).map(h => h.body)
  [#page.output #page.route #titles.len()]
}
]
#document("about/index.html")[
= About
#context [#current-document().route]
]"#,
        );
        let guide = html(&compiled, "/guide/index.html");
        assert!(guide.contains("guide/index.html /guide/ 1"), "{guide}");
        let about = html(&compiled, "/about/index.html");
        assert!(about.contains("/about/"), "{about}");
    }

    #[test]
    fn composed_document_keeps_its_properties() {
        let (_directory, compiled) = compiled(
            r#"#document("index.html", title: [Outer])[
#set document(title: [Combined])
#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: [First],))
First
#tola-meta((title: [Second],))
Second
]"#,
        );
        assert_eq!(
            compiled.documents[0].properties.title.as_deref(),
            Some("Combined")
        );
        let body = html(&compiled, "/index.html");
        assert!(body.contains("<title>Combined</title>"), "{body}");
        assert!(body.contains("First") && body.contains("Second"), "{body}");
    }

    #[test]
    fn computed_reads_are_recorded() {
        let (_directory, compiled) = compiled(
            r#"#let stem = "value"
#document("index.html")[#read(stem + ".txt")]"#,
        );
        assert!(html(&compiled, "/index.html").contains("payload"));
        assert!(compiled.compilation.file_reads().iter().any(|read| {
            matches!(
                read.evidence().locator(),
                tola_typst::ReadLocator::Root(path)
                    if path == std::path::Path::new("value.txt")
            )
        }));
    }
    #[test]
    fn explicit_permalinks_bypass_routing() {
        let directory = TempDir::new().unwrap();
        let compiled = compile_sources(
            &directory,
            r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/address:0.0.0": route-to-output
#for source in all-sources().filter(source => not source.meta.at("draft", default: false)) {
  document(route-to-output(source.meta.permalink), format: "html")[#include source.file]
}"#,
            &[
                (
                    "---.typ",
                    "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((permalink: \"/chosen/\",))\nChosen body",
                ),
                (
                    "posts/-..-/leaf.typ",
                    "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((draft: true,))\nExcluded body",
                ),
            ],
            "",
        )
        .unwrap();
        assert_eq!(
            compiled.document_outputs,
            BTreeSet::from([OutputPath::parse("chosen/index.html").unwrap()])
        );
        assert!(html(&compiled, "/chosen/index.html").contains("Chosen body"));
    }

    /// A source whose layout segment cannot name a published route fails the build, and the
    /// diagnostic is anchored at that source's own file.
    ///
    /// The `segment 1 (…)` echo and the site-program span of the old shape came from the deleted
    /// `document-path(source)` native, which took a source record and formatted its path; nothing
    /// surviving does, so a source is named by anchoring the diagnostic at it.
    #[test]
    fn routeless_source_is_named_in_the_error() {
        let directory = TempDir::new().unwrap();
        let error = compile_sources(
            &directory,
            r#"#document("index.html", format: "html")[Home]"#,
            &[(
                "a..typ",
                r#"#import "@tola/address:0.0.0": route
#import "@tola/source:0.0.0": current-source
#let route = route(current-source().route-segments)
#import "@tola/source:0.0.0": tola-meta
#tola-meta((permalink: route))"#,
            )],
            "",
        )
        .err()
        .expect("a layout segment that cannot name a route fails the build");
        let diagnostics = crate::diagnostic::attached(&error).unwrap();
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| {
                diagnostic
                    .location
                    .as_ref()
                    .is_some_and(|location| location.path.ends_with("a..typ"))
            })
            .expect("the diagnostic is anchored at the source");
        assert!(
            diagnostic
                .message
                .contains("a URL segment must not end with a dot or space"),
            "{}",
            diagnostic.message
        );
    }

    #[test]
    fn shared_recommended_output_is_rejected() {
        let directory = TempDir::new().unwrap();
        let sources = [
            (
                "about.typ",
                "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((permalink: \"/flat/\"))\nFlat",
            ),
            (
                "about/index.typ",
                "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((permalink: \"/directory/\"))\nDirectory",
            ),
            (
                "index.typ",
                "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((permalink: \"/\"))\nHome",
            ),
        ];
        let error = compile_sources(
            &directory,
            r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/address:0.0.0": route, route-to-output
#for source in all-sources() {
  document(route-to-output(route(source.route-segments)), format: "html")[#include source.file]
}"#,
            &sources,
            "",
        )
        .err()
        .expect("emitted documents cannot share one recommended output");
        assert!(
            crate::diagnostic::attached(&error)
                .unwrap()
                .iter()
                .any(|diagnostic| { diagnostic.message.contains("about/index.html") }),
            "{error:#}"
        );
        let compiled = compile_sources(
            &directory,
            r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/address:0.0.0": route, route-to-output
#for source in all-sources() {
  assert.eq(route(source.route-segments), if source.id == "index.typ" { "/" } else { "/about/" })
  document(route-to-output(source.meta.permalink), format: "html")[
    #source.id
    #include source.file
  ]
}"#,
            &sources,
            "",
        )
        .unwrap();
        assert_eq!(
            compiled.document_outputs,
            BTreeSet::from([
                OutputPath::parse("flat/index.html").unwrap(),
                OutputPath::parse("directory/index.html").unwrap(),
                OutputPath::parse("index.html").unwrap(),
            ])
        );
        for (output, identity) in [
            ("/flat/index.html", "about.typ"),
            ("/directory/index.html", "about/index.typ"),
            ("/index.html", "index.typ"),
        ] {
            assert!(html(&compiled, output).contains(identity), "{output}");
        }
    }

    #[test]
    fn content_entry_keeps_helpers_visible() {
        let directory = TempDir::new().unwrap();
        let compiled = compile_sources(
            &directory,
            r#"#import "@tola/source:0.0.0": all-sources
#import "helpers/card.typ": card
#let sources = all-sources()
#assert.eq(sources.map(source => source.id).sorted(), ("document.typ", "helpers/card.typ"))
#let document-source = sources.find(source => source.id == "document.typ")
#document("index.html", format: "html")[#card(document-source.meta.title)]"#,
            &[
                (
                    "document.typ",
                    "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: [Document]))",
                ),
                ("helpers/card.typ", "#let card(body) = html.strong(body)"),
            ],
            "[build]\nentry = \"content/site.typ\"",
        )
        .unwrap();
        assert!(html(&compiled, "/index.html").contains("<strong>Document</strong>"));
    }

    #[test]
    fn slugified_source_names_reach_outputs() {
        let directory = TempDir::new().unwrap();
        let compiled = compile_sources(
            &directory,
            r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/address:0.0.0": slugify
#for source in all-sources() {
  document("pages/" + slugify(source.id, mode: "ascii") + "/index.html", format: "html")[#include source.file]
}"#,
            &[
                ("fractions/½-price.typ", "Fraction body"),
                ("root/／escape.typ", "Root body"),
            ],
            "",
        )
        .unwrap();
        assert_eq!(
            compiled.document_outputs,
            BTreeSet::from([
                OutputPath::parse("pages/fractions-1-2-price.typ/index.html").unwrap(),
                OutputPath::parse("pages/root-escape.typ/index.html").unwrap(),
            ])
        );
        assert!(
            html(&compiled, "/pages/fractions-1-2-price.typ/index.html").contains("Fraction body")
        );
        assert!(html(&compiled, "/pages/root-escape.typ/index.html").contains("Root body"));
    }
}
