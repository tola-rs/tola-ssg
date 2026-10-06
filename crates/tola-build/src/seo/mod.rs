//! Feed and sitemap outputs declared by the final Typst Bundle.

use std::sync::Arc;

use crate::cancellation::BuildCancellation;
use crate::config::{ReferenceLevel, ResolvedSiteConfig};

mod date;
mod declaration;
pub(crate) mod feed;
mod sitemap;

pub(crate) use declaration::SeoDeclarationError;

#[derive(Debug, thiserror::Error)]
pub(crate) enum RenderError {
    #[error(transparent)]
    Declaration(#[from] SeoDeclarationError),
    #[error(transparent)]
    Cancelled(#[from] crate::cancellation::BuildCancelled),
    #[error(transparent)]
    HtmlExport(tola_typst::HtmlFragmentError),
    #[error(transparent)]
    Compiler(tola_typst::CompileError),
}

impl From<tola_typst::CompileError> for RenderError {
    fn from(error: tola_typst::CompileError) -> Self {
        match error {
            tola_typst::CompileError::Cancelled => {
                Self::Cancelled(crate::cancellation::BuildCancelled)
            }
            error => Self::Compiler(error),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RenderedOutputs {
    pub(crate) outputs: Vec<RenderedOutput>,
    pub(crate) warnings: Vec<tola_typst::NativeDiagnostic>,
}

#[derive(Debug, Clone)]
pub(crate) struct RenderedOutput {
    pub(crate) producer: &'static str,
    pub(crate) url: tola_address::UrlPath,
    pub(crate) declaration: crate::output::semantics::OutputDeclaration,
    pub(crate) bytes: Arc<[u8]>,
}

/// Complete immutable SEO outputs and diagnostics from a successful rendering pass.
#[derive(Debug)]
pub(crate) struct SeoCompilation {
    compilation: Arc<tola_typst::BundleCompilation>,
    configuration: SeoConfiguration,
    rendered: RenderedOutputs,
}

impl SeoCompilation {
    pub(crate) fn prepare(
        config: &ResolvedSiteConfig,
        compilation: &Arc<tola_typst::BundleCompilation>,
        cancellation: &BuildCancellation,
        previous: Option<&Arc<Self>>,
    ) -> Result<Arc<Self>, RenderError> {
        cancellation.ensure_active()?;
        let prepared = if let Some(previous) = previous
            && Arc::ptr_eq(&previous.compilation, compilation)
            && previous.configuration.matches(config)
        {
            Arc::clone(previous)
        } else {
            let rendered = render_outputs(config, compilation, cancellation)?;
            Arc::new(Self {
                compilation: Arc::clone(compilation),
                configuration: SeoConfiguration::capture(config),
                rendered,
            })
        };
        cancellation.ensure_active()?;
        Ok(prepared)
    }

    pub(crate) fn outputs(&self) -> &[RenderedOutput] {
        &self.rendered.outputs
    }

    pub(crate) fn warnings(&self) -> &[tola_typst::NativeDiagnostic] {
        &self.rendered.warnings
    }
}

/// Inputs read by canonical URLs and document-link diagnostics.
/// Keep this boundary in sync with configuration reads in the SEO renderers.
#[derive(Debug, PartialEq, Eq)]
struct SeoConfiguration {
    origin: Option<String>,
    base_path: String,
    navigation: ReferenceLevel,
    resources: ReferenceLevel,
}

impl SeoConfiguration {
    fn capture(config: &ResolvedSiteConfig) -> Self {
        Self {
            origin: config.site.origin.clone(),
            base_path: config.site.base_path.clone(),
            navigation: config.build.references.navigation,
            resources: config.build.references.resources,
        }
    }

    fn matches(&self, config: &ResolvedSiteConfig) -> bool {
        *self == Self::capture(config)
    }
}

pub(crate) fn render_outputs(
    config: &crate::config::ResolvedSiteConfig,
    compilation: &tola_typst::BundleCompilation,
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<RenderedOutputs, RenderError> {
    cancellation.ensure_active()?;
    let rendered = (|| {
        let mut rendered = feed::render_outputs(config, compilation, cancellation)?;
        rendered
            .outputs
            .extend(sitemap::render_outputs(config, compilation, cancellation)?);
        Ok(rendered)
    })();
    cancellation.ensure_active()?;
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::section::site::LanguageDeclaration;

    #[test]
    fn cancelled_render_reports_cancellation() {
        let (_directory, config, compilation) = compile(
            "#document(\"index.html\")[Home]\n#metadata(42) <tola-feed>",
            "",
        );
        let canceller = crate::cancellation::BuildCanceller::default();
        canceller.cancel();

        let rendered = render_outputs(&config, &compilation, &canceller.token());

        assert!(matches!(rendered, Err(RenderError::Cancelled(_))));
    }

    /// Builtin package files for a world with no icon collections.
    struct PackageFiles;

    impl tola_typst::FileProvider for PackageFiles {
        fn target(&self, id: typst::syntax::FileId) -> Option<tola_typst::FileTarget> {
            let typst::syntax::VirtualRoot::Package(package) = id.root() else {
                return None;
            };
            crate::package::read_package(package, id.vpath().get_with_slash())
                .map(|bytes| tola_typst::FileTarget::Bytes(bytes.into()))
        }
    }

    fn compile(
        source: &str,
        settings: &str,
    ) -> (
        tempfile::TempDir,
        ResolvedSiteConfig,
        tola_typst::BundleCompilation,
    ) {
        let directory = tempfile::TempDir::new().unwrap();
        let main = directory.path().join("site.typ");
        std::fs::write(
            &main,
            format!("#import \"@tola/web:0.0.0\": feed, sitemap\n{source}"),
        )
        .unwrap();
        let config = crate::config::tests::load_test_config(directory.path(), settings);
        let bindings = crate::package::SiteBindings::from_config(&config, Default::default());
        let library = bindings.library(Default::default());
        let files = tola_typst::FileResolver::new().with_provider(PackageFiles);
        let world = tola_typst::TypstWorld::builder(&main, directory.path())
            .with_files(Arc::new(files))
            .with_shared_library(library.shared())
            .with_local_cache()
            .no_fonts()
            .build(&tola_typst::BundleCancellation::default())
            .unwrap();
        let compilation =
            tola_typst::compile_bundle_world(&world, &tola_typst::BundleCancellation::default())
                .unwrap();
        (directory, config, compilation)
    }

    fn render(source: &str) -> RenderedOutputs {
        let (_directory, config, compilation) = compile(
            source,
            "[site]\norigin = \"https://example.com\"\nbase-path = \"/blog/\"\ntitle = \"Notes\"\ndescription = \"Published notes\"",
        );
        render_outputs(
            &config,
            &compilation,
            &crate::cancellation::BuildCancellation::default(),
        )
        .unwrap()
    }

    fn declaration_error(source: &str, settings: &str) -> SeoDeclarationError {
        let (_directory, config, compilation) = compile(source, settings);
        let error =
            render_outputs(&config, &compilation, &BuildCancellation::default()).unwrap_err();
        let RenderError::Declaration(error) = error else {
            panic!("expected a declaration error, got {error}");
        };
        error
    }

    fn output<'a>(outputs: &'a RenderedOutputs, path: &str) -> &'a [u8] {
        outputs
            .outputs
            .iter()
            .find(|output| output.url.as_str() == path)
            .unwrap()
            .bytes
            .as_ref()
    }

    #[test]
    fn each_feed_keeps_its_own_entries() {
        let outputs = render(
            r#"
#document("notes/index.html", title: [Notes])[#html.p(id: "first")[First] #html.p(id: "second")[Second]]
#let first = (id: "urn:notes:first", target: (output: "notes/index.html", fragment: "first"), title: [First], published: "2026-09-01T09:00:00+08:00")
#let second = (id: "urn:notes:second", target: (output: "notes/index.html", fragment: "second"), title: [Second], published: datetime(year: 2026, month: 9, day: 2))
#feed(format: "json", entries: (first,))
#feed(entries: (second, first))
#feed(format: "atom", id: "urn:notes:channel", authors: ("Alice",), entries: (second,))
"#,
        );
        assert_eq!(outputs.outputs.len(), 3);
        let json: serde_json::Value =
            serde_json::from_slice(output(&outputs, "/feed.json")).unwrap();
        assert_eq!(json["items"].as_array().unwrap().len(), 1);
        assert_eq!(json["items"][0]["id"], "urn:notes:first");
        assert_eq!(
            json["items"][0]["url"],
            "https://example.com/blog/notes/#first"
        );
        assert_eq!(
            json["items"][0]["date_published"],
            "2026-09-01T09:00:00+08:00"
        );
        let rss = rss::Channel::read_from(output(&outputs, "/feed.xml")).unwrap();
        assert_eq!(rss.items().len(), 2);
        assert_eq!(rss.items()[0].guid().unwrap().value(), "urn:notes:second");
        assert!(!rss.items()[0].guid().unwrap().is_permalink());
        let atom = atom_syndication::Feed::read_from(output(&outputs, "/atom.xml")).unwrap();
        assert_eq!(atom.id(), "urn:notes:channel");
        assert_eq!(atom.entries()[0].id(), "urn:notes:second");
        assert!(atom.entries()[0].published().is_some());
    }

    #[test]
    fn site_defaults_supply_feed_fields() {
        let settings = r#"
[site]
origin = "https://example.com"
base-path = "/blog/"
title = "Site notes"
description = "Published notes"
language = "EN-GB"
authors = [{ name = "Alice", email = "alice@example.com", url = "https://example.com/alice/" }]
"#;
        let (_directory, config, compilation) = compile(
            r#"
#document("index.html", title: [Home])[Home]
#let channel = feed.with(format: "json")
#channel()
#channel(output: "empty.json", authors: ())
#channel(
  output: "custom.json",
  title: [Chosen title],
  description: [Chosen description],
  language: "fr-CA",
  authors: ("Bob",),
)
#feed(format: "atom", output: "channels/notes.xml")
"#,
            settings,
        );
        let outputs = render_outputs(&config, &compilation, &BuildCancellation::default()).unwrap();
        let defaults: serde_json::Value =
            serde_json::from_slice(output(&outputs, "/feed.json")).unwrap();
        assert_eq!(defaults["title"], "Site notes");
        assert_eq!(defaults["description"], "Published notes");
        assert_eq!(defaults["language"], "en-GB");
        assert_eq!(defaults["authors"][0]["name"], "Alice");
        assert_eq!(defaults["authors"][0]["url"], "https://example.com/alice/");
        assert_eq!(defaults["feed_url"], "https://example.com/blog/feed.json");
        let empty: serde_json::Value =
            serde_json::from_slice(output(&outputs, "/empty.json")).unwrap();
        assert!(empty.get("authors").is_none());
        let custom: serde_json::Value =
            serde_json::from_slice(output(&outputs, "/custom.json")).unwrap();
        assert_eq!(custom["title"], "Chosen title");
        assert_eq!(custom["description"], "Chosen description");
        assert_eq!(custom["language"], "fr-CA");
        assert_eq!(custom["authors"][0]["name"], "Bob");
        assert_eq!(custom["feed_url"], "https://example.com/blog/custom.json");
        let atom =
            atom_syndication::Feed::read_from(output(&outputs, "/channels/notes.xml")).unwrap();
        assert_eq!(atom.id(), "https://example.com/blog/channels/notes.xml");
        assert_eq!(atom.authors()[0].name(), "Alice");
        assert_eq!(atom.authors()[0].email(), Some("alice@example.com"));

        let error = declaration_error(
            r#"#document("index.html")[Home] #feed(format: "atom", authors: ())"#,
            settings,
        );
        assert!(
            error
                .source_diagnostic()
                .source()
                .message
                .contains("/authors")
        );
    }

    #[test]
    fn feed_bodies_use_final_html() {
        let outputs = render(
            r#"
#document("post/index.html", title: [Post], html.html(
  html.head(html.base(href: "/blog/media/"))
  + html.body[
    #html.article(id: "article")[
      #html.h2[Document heading]
      #html.table[#html.tr[#html.td[Cell]]]
      #html.pre[#html.code[let answer = 42;]]
      #html.a(href: "../target/#part")[Target]
      #html.img(src: "photo.png")
    ]
    #html.p[Outside the selected document]
  ]
))
#let entry = (id: "urn:post", target: "post/index.html", published: datetime(year: 2026, month: 9, day: 1), summary: [Short summary], content: (document: "post/index.html", id: "article"))
#feed(output: "feed.json", format: "json", entries: (entry,))
#feed(output: "feed.xml", format: "rss", entries: (entry,))
#feed(output: "feed.atom", format: "atom", authors: ("Alice",), entries: (entry,))
#feed(output: "body.json", format: "json", entries: (entry + (content: (document: "post/index.html")),))
"#,
        );
        let json: serde_json::Value =
            serde_json::from_slice(output(&outputs, "/feed.json")).unwrap();
        let html = json["items"][0]["content_html"].as_str().unwrap();
        assert!(html.contains("<h2>Document heading</h2>"), "{html}");
        assert!(html.contains("<table>"), "{html}");
        assert!(html.contains("<code>let answer = 42;</code>"), "{html}");
        assert!(
            html.contains("href=\"https://example.com/blog/target/#part\""),
            "{html}"
        );
        assert!(
            html.contains("src=\"https://example.com/blog/media/photo.png\""),
            "{html}"
        );
        assert!(!html.contains("Outside the selected document"));
        assert_eq!(json["items"][0]["summary"], "Short summary");
        let rss = rss::Channel::read_from(output(&outputs, "/feed.xml")).unwrap();
        assert_eq!(rss.items()[0].content(), Some(html));
        let atom = atom_syndication::Feed::read_from(output(&outputs, "/feed.atom")).unwrap();
        assert_eq!(atom.entries()[0].content().unwrap().value(), Some(html));
        let body: serde_json::Value =
            serde_json::from_slice(output(&outputs, "/body.json")).unwrap();
        assert!(
            body["items"][0]["content_html"]
                .as_str()
                .unwrap()
                .contains("Outside the selected document")
        );

        let outputs = render(
            r#"
#document("post/index.html", title: [Post])[#html.article(id: "article")[$ x / y $]]
#feed(output: "feed.json", format: "json", entries: ((id: "math", target: "post/index.html", published: datetime(year: 2026, month: 9, day: 1), content: (document: "post/index.html", id: "article")),))
"#,
        );
        let json: serde_json::Value =
            serde_json::from_slice(output(&outputs, "/feed.json")).unwrap();
        let html = json["items"][0]["content_html"].as_str().unwrap();
        assert!(html.contains("<style>"), "{html}");
        assert!(html.contains("mfrac"), "{html}");
        assert!(html.contains("<math"), "{html}");
    }

    #[test]
    fn content_target_needs_one_html_element() {
        for (body, id, expected) in [
            (
                "#html.article(id: \"article\")[Document]",
                "missing",
                "no HTML element in the target document",
            ),
            (
                "#html.p(id: \"same\")[First] #html.p(id: \"same\")[Second]",
                "same",
                "multiple elements in the target document",
            ),
        ] {
            let source = format!(
                "#document(\"index.html\", title: [Page])[{body}]\n#feed(entries: ((id: \"entry\", target: \"index.html\", published: datetime(year: 2026, month: 9, day: 1), content: (document: \"index.html\", id: \"{id}\")),))"
            );
            let (_directory, config, compilation) = compile(
                &source,
                "[site]\norigin = \"https://example.test\"\ntitle = \"Feed\"",
            );
            let error = render_outputs(
                &config,
                &compilation,
                &crate::cancellation::BuildCancellation::default(),
            )
            .unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
            assert!(error.to_string().contains("/entries/0/content"), "{error}");
        }
    }

    const SEO_CACHE_SOURCE: &str = r#"
#document("index.html", title: [Post])[
  #html.p[Document body]
  #html.a(href: "https://[")[Unresolved navigation]
  #html.img(src: "https://[image")
]
#let entry = (id: "urn:entry", target: "index.html", published: datetime(year: 2026, month: 9, day: 1), content: (document: "index.html"))
#feed(format: "atom", output: "feed.atom", authors: ("Alice",), entries: (entry,))
#sitemap(output: "sitemap.xml", targets: ("index.html",))
#feed(format: "json", output: "feed.json", entries: (entry,))
#feed(format: "rss", output: "feed.xml", entries: (entry,))
"#;

    const SEO_CACHE_CONFIG: &str = "[site]\norigin = \"https://example.test\"\nbase-path = \"/blog/\"\ntitle = \"Feed\"\ndescription = \"Notes\"\n[build.references]\nnavigation = \"warn\"\nresources = \"warn\"";

    fn seo_cache_site() -> (
        tempfile::TempDir,
        ResolvedSiteConfig,
        Arc<tola_typst::BundleCompilation>,
    ) {
        let (directory, config, compilation) = compile(SEO_CACHE_SOURCE, SEO_CACHE_CONFIG);
        (directory, config, Arc::new(compilation))
    }

    fn assert_seo_matches_fresh(
        prepared: &SeoCompilation,
        config: &ResolvedSiteConfig,
        compilation: &tola_typst::BundleCompilation,
    ) {
        let fresh = render_outputs(config, compilation, &BuildCancellation::default()).unwrap();
        assert_eq!(prepared.warnings(), fresh.warnings);
        assert_eq!(prepared.outputs().len(), fresh.outputs.len());
        for (cached, fresh) in prepared.outputs().iter().zip(&fresh.outputs) {
            assert_eq!(cached.producer, fresh.producer);
            assert_eq!(cached.url, fresh.url);
            assert_eq!(cached.declaration, fresh.declaration);
            assert_eq!(cached.bytes, fresh.bytes);
        }
    }

    #[test]
    fn seo_outputs_reuse_only_matching_inputs() {
        let (_directory, config, compilation) = seo_cache_site();
        let cancellation = BuildCancellation::default();
        let original = SeoCompilation::prepare(&config, &compilation, &cancellation, None).unwrap();
        assert_seo_matches_fresh(&original, &config, &compilation);
        assert_eq!(
            original
                .outputs()
                .iter()
                .map(|output| output.url.as_str())
                .collect::<Vec<_>>(),
            ["/feed.atom", "/feed.json", "/feed.xml", "/sitemap.xml"]
        );
        assert_eq!(original.warnings().len(), 2);
        assert!(
            original
                .warnings()
                .iter()
                .all(|warning| warning.source().span.id().is_some()
                    && warning.source().severity == typst::diag::Severity::Warning)
        );
        let json: serde_json::Value = serde_json::from_slice(&original.outputs()[1].bytes).unwrap();
        let html = json["items"][0]["content_html"].as_str().unwrap();
        assert!(html.contains("href=\"https://[\""), "{html}");
        assert!(html.contains("src=\"https://[image\""), "{html}");

        let reused =
            SeoCompilation::prepare(&config, &compilation, &cancellation, Some(&original)).unwrap();
        assert!(Arc::ptr_eq(&original, &reused));
        assert_seo_matches_fresh(&reused, &config, &compilation);

        let mut unrelated = config.clone();
        unrelated.site.copyright = "Different template metadata".into();
        unrelated.site.title = "Revised feed".into();
        unrelated.site.description = "Revised description".into();
        unrelated.site.language = LanguageDeclaration::Tag("fr".into());
        unrelated.build.references.fragments = ReferenceLevel::Warn;
        let reused =
            SeoCompilation::prepare(&unrelated, &compilation, &cancellation, Some(&original))
                .unwrap();
        assert!(Arc::ptr_eq(&original, &reused));
        assert_seo_matches_fresh(&reused, &unrelated, &compilation);

        type SiteSettingChange = fn(&mut ResolvedSiteConfig);
        let changes: [(&str, SiteSettingChange); 2] = [
            ("origin", |config| {
                config.site.origin = Some("https://elsewhere.test".into())
            }),
            ("base-path", |config| {
                config.site.base_path = "/notes/".into()
            }),
        ];
        for (field, change) in changes {
            let mut changed = config.clone();
            change(&mut changed);
            let updated =
                SeoCompilation::prepare(&changed, &compilation, &cancellation, Some(&original))
                    .unwrap();
            assert!(!Arc::ptr_eq(&original, &updated), "{field}");
            assert_seo_matches_fresh(&updated, &changed, &compilation);
            assert_ne!(
                updated.outputs()[0].bytes,
                original.outputs()[0].bytes,
                "{field}"
            );
        }

        let (_equivalent_directory, _, equivalent_bundle) =
            compile(SEO_CACHE_SOURCE, SEO_CACHE_CONFIG);
        let equivalent_bundle = Arc::new(equivalent_bundle);
        let equivalent =
            SeoCompilation::prepare(&config, &equivalent_bundle, &cancellation, Some(&original))
                .unwrap();
        assert!(!Arc::ptr_eq(&original, &equivalent));
        assert_seo_matches_fresh(&equivalent, &config, &equivalent_bundle);
        assert_eq!(equivalent.outputs()[0].bytes, original.outputs()[0].bytes);

        let (_changed_directory, _, changed_bundle) = compile(
            &SEO_CACHE_SOURCE.replace("Document body", "Revised body"),
            SEO_CACHE_CONFIG,
        );
        let changed_bundle = Arc::new(changed_bundle);
        let updated =
            SeoCompilation::prepare(&config, &changed_bundle, &cancellation, Some(&original))
                .unwrap();
        assert!(!Arc::ptr_eq(&original, &updated));
        assert_seo_matches_fresh(&updated, &config, &changed_bundle);
        assert_ne!(updated.outputs()[0].bytes, original.outputs()[0].bytes);
    }

    #[test]
    fn failed_seo_does_not_reuse_outputs() {
        let (_directory, config, compilation) = seo_cache_site();
        let cancellation = BuildCancellation::default();
        let original = SeoCompilation::prepare(&config, &compilation, &cancellation, None).unwrap();

        let changes: [fn(&mut ResolvedSiteConfig); 3] = [
            |config| config.build.references.navigation = ReferenceLevel::Error,
            |config| config.build.references.resources = ReferenceLevel::Error,
            |config| config.site.origin = None,
        ];
        for change in changes {
            let mut changed = config.clone();
            change(&mut changed);
            let fresh = render_outputs(&changed, &compilation, &cancellation).unwrap_err();
            let attempted =
                SeoCompilation::prepare(&changed, &compilation, &cancellation, Some(&original))
                    .unwrap_err();
            let (RenderError::Declaration(fresh), RenderError::Declaration(attempted)) =
                (fresh, attempted)
            else {
                panic!("expected source-attached declaration errors");
            };
            assert_eq!(attempted.source_diagnostic(), fresh.source_diagnostic());
        }

        let (_failed_directory, _, failed_bundle) = compile(
            &format!("{SEO_CACHE_SOURCE}\n#metadata(42) <tola-feed>"),
            SEO_CACHE_CONFIG,
        );
        assert!(matches!(
            SeoCompilation::prepare(
                &config,
                &Arc::new(failed_bundle),
                &cancellation,
                Some(&original)
            ),
            Err(RenderError::Declaration(_))
        ));

        let recovered =
            SeoCompilation::prepare(&config, &compilation, &cancellation, Some(&original)).unwrap();
        assert!(Arc::ptr_eq(&original, &recovered));
        assert_seo_matches_fresh(&recovered, &config, &compilation);
    }

    #[test]
    fn cancelled_seo_does_not_reuse_outputs() {
        let (_directory, config, compilation) = seo_cache_site();
        let original =
            SeoCompilation::prepare(&config, &compilation, &BuildCancellation::default(), None)
                .unwrap();
        let canceller = crate::cancellation::BuildCanceller::default();
        let cancellation = canceller.token();
        canceller.cancel();

        for previous in [None, Some(&original)] {
            assert!(matches!(
                SeoCompilation::prepare(&config, &compilation, &cancellation, previous),
                Err(RenderError::Cancelled(_))
            ));
        }
    }

    #[test]
    fn nested_content_error_keeps_pointer() {
        use std::error::Error;

        let error = declaration_error(
            r#"
#document("post/index.html", title: [Post])[Document]
#feed(format: "json", title: "Feed", entries: ((
  target: "post/index.html",
  published: datetime(year: 2026, month: 9, day: 1),
  summary: emph(strong(delta: 300)[Bad]),
),))
"#,
            "[site]\norigin = \"https://example.test\"\n",
        );
        let violation = error
            .source()
            .unwrap()
            .source()
            .unwrap()
            .downcast_ref::<feed::FeedContentViolation>()
            .unwrap();
        assert!(matches!(
            violation,
            feed::FeedContentViolation::ContentField { element } if element == "strong"
        ));
        let message = error.to_string();
        assert!(message.contains("/entries/0/summary/body/delta"));
        assert_eq!(message.matches("/entries/0/summary").count(), 1);
    }

    #[test]
    fn feeds_keep_summary_separate_from_content() {
        let outputs = render(
            r#"
#document("post/index.html", title: [Post])[Document]
#let entry = (
  id: "urn:post:1", target: "post/index.html", published: datetime(year: 2026, month: 9, day: 1),
  summary: [#link("/blog/next/?a=1&b=2")[Next]],
  content: [*Full body* #link("/outside/")[Outside]],
)
#feed(output: "feed.json", format: "json", entries: (entry,))
#feed(output: "feed.xml", format: "rss", entries: (entry,))
#feed(output: "feed.atom", format: "atom", authors: ("Alice",), entries: (entry,))
"#,
        );
        let json: serde_json::Value =
            serde_json::from_slice(output(&outputs, "/feed.json")).unwrap();
        assert_eq!(json["items"][0]["summary"], "Next");
        assert_eq!(
            json["items"][0]["content_html"],
            "<strong>Full body</strong> <a href=\"https://example.com/outside/\">Outside</a>"
        );
        let rss = rss::Channel::read_from(output(&outputs, "/feed.xml")).unwrap();
        assert_eq!(
            rss.items()[0].description(),
            Some("<a href=\"https://example.com/blog/next/?a=1&amp;b=2\">Next</a>")
        );
        assert_ne!(rss.items()[0].description(), rss.items()[0].content());
        let atom = atom_syndication::Feed::read_from(output(&outputs, "/feed.atom")).unwrap();
        assert_ne!(
            atom.entries()[0].summary().unwrap().as_str(),
            atom.entries()[0].content().unwrap().value().unwrap()
        );

        let outputs = render(
            r#"
#document("index.html", title: [Home])[Home]
#feed(format: "json", output: "feed.json", entries: ((id: "home", target: "index.html", published: datetime(year: 2026, month: 9, day: 1), summary: "Short summary"),))
"#,
        );
        let json: serde_json::Value =
            serde_json::from_slice(output(&outputs, "/feed.json")).unwrap();
        assert_eq!(json["items"][0]["summary"], "Short summary");
        assert_eq!(json["items"][0]["content_text"], "");
        assert!(json["items"][0]["content_html"].is_null());
    }

    #[test]
    fn contextual_targets_use_final_document() {
        let outputs = render(
            r#"
#document("document/index.html", title: [Outer])[
  #set document(title: [Inner])
  Document
] <document>
#context {
  let published = datetime(year: 2026, month: 9, day: 1)
  feed(format: "json", output: "label.json", entries: ((
    target: <document>, published: published,
  ),))
  feed(format: "json", output: "location.json", entries: ((
    id: auto, target: query(<document>).first().location(), published: published,
  ),))
}
"#,
        );
        for path in ["/label.json", "/location.json"] {
            let json: serde_json::Value = serde_json::from_slice(output(&outputs, path)).unwrap();
            assert_eq!(json["items"][0]["title"], "Inner");
            assert_eq!(
                json["items"][0]["url"],
                "https://example.com/blog/document/"
            );
            assert_eq!(json["items"][0]["id"], "https://example.com/blog/document/");
        }

        let outputs = render(
            r#"
#document("index.html", title: [Home])[#html.p(id: "part%20x")[Part]]
#feed(format: "json", output: "feed.json", entries: ((id: "part", target: (output: "index.html", fragment: "part%20x"), published: datetime(year: 2026, month: 9, day: 1)),))
"#,
        );
        let json: serde_json::Value =
            serde_json::from_slice(output(&outputs, "/feed.json")).unwrap();
        assert_eq!(
            json["items"][0]["url"],
            "https://example.com/blog/#part%2520x"
        );

        let outputs = render(
            r#"
#document("paper.pdf", title: [Paper])[#rect(width: 1pt, height: 1pt)]
#feed(format: "json", output: "feed.json", entries: ((id: "paper", target: (output: "paper.pdf", fragment: "page=3&view=Fit"), published: datetime(year: 2026, month: 9, day: 1)),))
"#,
        );
        let json: serde_json::Value =
            serde_json::from_slice(output(&outputs, "/feed.json")).unwrap();
        assert_eq!(
            json["items"][0]["url"],
            "https://example.com/blog/paper.pdf#page=3&view=Fit"
        );
    }

    #[test]
    fn each_sitemap_keeps_its_own_entries() {
        let outputs = render(
            r#"
#document("a/index.html")[A]
#document("b/index.html")[B]
#document("c/index.html")[C]
#sitemap(output: "first.xml", targets: ((target: "b/index.html", lastmod: datetime(year: 2026, month: 9, day: 1)), "a/index.html"))
#sitemap(output: "second.xml", targets: ("c/index.html",))
"#,
        );
        assert_eq!(outputs.outputs.len(), 2);
        let xml = std::str::from_utf8(output(&outputs, "/first.xml")).unwrap();
        let document = roxmltree::Document::parse(xml).unwrap();
        let locations = document
            .descendants()
            .filter(|node| {
                node.has_tag_name(("http://www.sitemaps.org/schemas/sitemap/0.9", "loc"))
            })
            .map(|node| node.text().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            locations,
            ["https://example.com/blog/b/", "https://example.com/blog/a/"]
        );
        assert!(xml.contains("<lastmod>2026-09-01</lastmod>"));
        let second = std::str::from_utf8(output(&outputs, "/second.xml")).unwrap();
        assert!(second.contains("https://example.com/blog/c/"));
    }

    #[test]
    fn feed_target_needs_exported_anchor() {
        let error = declaration_error(
            r#"
#document("index.html", title: [Home])[#html.p[Part] <part>]
#feed(entries: ((target: <part>, published: datetime(year: 2026, month: 9, day: 1)),))
"#,
            "[site]\norigin = \"https://example.com\"\ntitle = \"Home\"",
        );
        let diagnostic = error.source_diagnostic();
        assert!(diagnostic.source().message.contains("/entries/0/target"));
    }

    #[test]
    fn feed_target_uses_exported_anchor() {
        let outputs = render(
            r#"
#document("b/index.html")[#link(<part>)[Part]]
#document("index.html", title: [Home])[#html.p[Part] <part>]
#feed(entries: ((target: <part>, published: datetime(year: 2026, month: 9, day: 1)),))
"#,
        );
        let rss = rss::Channel::read_from(output(&outputs, "/feed.xml")).unwrap();
        assert_eq!(
            rss.items()[0].link(),
            Some("https://example.com/blog/#part")
        );
    }

    #[test]
    fn feed_requires_publication_date() {
        let error = declaration_error(
            r#"
#document("index.html", title: [Home])[Home]
#feed(entries: ((target: "index.html",),))
"#,
            "[site]\norigin = \"https://example.com\"\ntitle = \"Home\"",
        );
        let diagnostic = error.source_diagnostic();
        assert!(diagnostic.source().message.contains("/entries/0/published"));
    }

    #[test]
    fn sitemap_target_refuses_element_destination() {
        let error = declaration_error(
            r#"
#document("index.html")[#html.p(id: "part")[Part]]
#sitemap(targets: ((target: (output: "index.html", fragment: "part"),),))
"#,
            "[site]\norigin = \"https://example.com\"",
        );
        let diagnostic = error.source_diagnostic();
        assert!(diagnostic.source().message.contains("/targets/0"));
    }

    #[test]
    fn declaration_errors_keep_source_spans() {
        let (_directory, config, compilation) = compile(
            r#"
#document("index.html", title: [Home])[Home]
#feed(entries: ((id: "home", target: "missing/index.html", published: datetime(year: 2026, month: 9, day: 1)),))
"#,
            "[site]\norigin = \"https://example.com\"\ntitle = \"Home\"",
        );
        let error = render_outputs(
            &config,
            &compilation,
            &crate::cancellation::BuildCancellation::default(),
        )
        .err()
        .unwrap();
        assert!(error.to_string().contains("/entries/0/target"), "{error}");
        let RenderError::Declaration(error) = error else {
            panic!("expected a declaration error");
        };
        let diagnostic = error.source_diagnostic();
        let original = compilation.metadata_declarations("tola-feed")[0].span();
        assert_eq!(diagnostic.source().span, original.into());
    }

    #[test]
    fn declared_writes_need_valid_location() {
        for declaration in [
            r#"#sitemap(output: "_tola/hotreload.js", targets: ("index.html",))"#,
            r#"#sitemap(output: "_TOLA/sitemap.xml", targets: ("index.html",))"#,
            r#"#feed(output: "_tola/feed.xml", entries: ())"#,
            r#"#feed(output: "_tola", entries: ())"#,
        ] {
            let error = declaration_error(
                &format!("#document(\"index.html\")[Home]\n{declaration}"),
                "[site]\norigin = \"https://example.test\"\ntitle = \"Feed\"",
            );
            let diagnostic = error.source_diagnostic();
            assert!(
                diagnostic.source().message.contains("/output"),
                "{diagnostic:?}"
            );
        }

        let (_directory, config, compilation) = compile(
            r#"#document("index.html")[Home] #sitemap(targets: ("index.html",))"#,
            "",
        );
        let error = render_outputs(
            &config,
            &compilation,
            &crate::cancellation::BuildCancellation::default(),
        )
        .err()
        .unwrap();
        assert!(error.to_string().contains("site.origin"));
    }
}
