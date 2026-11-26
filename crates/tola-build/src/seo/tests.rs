use super::*;
use crate::config::section::site::LanguageDeclaration;

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
    src: &str,
    settings: &str,
) -> (tempfile::TempDir, SiteConfig, tola_typst::BundleCompilation) {
    let dir = tempfile::TempDir::new().unwrap();
    let main = dir.path().join("site.typ");
    std::fs::write(
        &main,
        format!(
            "#import \"@tola/feed:0.0.0\": feed\n#import \"@tola/sitemap:0.0.0\": sitemap\n{src}"
        ),
    )
    .unwrap();
    let cfg = crate::config::tests::load_test_config(dir.path(), settings);
    let bindings = crate::package::SiteBindings::from_config(&cfg, Default::default(), None);
    let lib = bindings.library(Default::default());
    let files = tola_typst::FileStore::new().with_provider(PackageFiles);
    let world = tola_typst::TypstWorld::builder(&main, dir.path())
        .with_files(Arc::new(files))
        .with_shared_library(lib.shared())
        .with_local_cache()
        .no_fonts()
        .build(&tola_typst::BundleCancellation::default())
        .unwrap();
    let compilation =
        tola_typst::compile_bundle_world(&world, &tola_typst::BundleCancellation::default())
            .unwrap();
    (dir, cfg, compilation)
}

fn render(src: &str) -> RenderedOutputs {
    let (_dir, cfg, compilation) = compile(
        src,
        "[site]\norigin = \"https://example.com\"\nbase-path = \"/blog/\"\ntitle = \"Notes\"\ndescription = \"Published notes\"",
    );
    render_outputs(&cfg, &compilation, &BuildCancellation::default()).unwrap()
}

const SITE_SETTINGS: &str = r#"
[site]
origin = "https://example.com"
base-path = "/blog/"
title = "Site notes"
description = "Published notes"
language = "EN-GB"
authors = [{ name = "Alice", email = "alice@example.com", url = "https://example.com/alice/" }]
"#;

fn declaration_error(src: &str, settings: &str) -> SeoDeclarationError {
    let (_dir, cfg, compilation) = compile(src, settings);
    let err = render_outputs(&cfg, &compilation, &BuildCancellation::default()).unwrap_err();
    let RenderError::Declaration(err) = err else {
        panic!("expected a declaration error, got {err}");
    };
    err
}

fn output<'a>(outputs: &'a RenderedOutputs, path: &str) -> &'a [u8] {
    outputs
        .outputs
        .iter()
        .find(|out| out.url.as_str() == path)
        .unwrap()
        .bytes
        .as_ref()
}

#[test]
fn feeds_keep_own_entries() {
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
    let json: serde_json::Value = serde_json::from_slice(output(&outputs, "/feed.json")).unwrap();
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
fn site_defaults_fill_feed_fields() {
    let (_dir, cfg, compilation) = compile(
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
        SITE_SETTINGS,
    );
    let outputs = render_outputs(&cfg, &compilation, &BuildCancellation::default()).unwrap();
    let defaults: serde_json::Value =
        serde_json::from_slice(output(&outputs, "/feed.json")).unwrap();
    assert_eq!(defaults["title"], "Site notes");
    assert_eq!(defaults["description"], "Published notes");
    assert_eq!(defaults["language"], "en-GB");
    assert_eq!(defaults["authors"][0]["name"], "Alice");
    assert_eq!(defaults["authors"][0]["url"], "https://example.com/alice/");
    assert_eq!(defaults["feed_url"], "https://example.com/blog/feed.json");
    let empty: serde_json::Value = serde_json::from_slice(output(&outputs, "/empty.json")).unwrap();
    assert!(empty.get("authors").is_none());
    let custom: serde_json::Value =
        serde_json::from_slice(output(&outputs, "/custom.json")).unwrap();
    assert_eq!(custom["title"], "Chosen title");
    assert_eq!(custom["description"], "Chosen description");
    assert_eq!(custom["language"], "fr-CA");
    assert_eq!(custom["authors"][0]["name"], "Bob");
    assert_eq!(custom["feed_url"], "https://example.com/blog/custom.json");
    let atom = atom_syndication::Feed::read_from(output(&outputs, "/channels/notes.xml")).unwrap();
    assert_eq!(atom.id(), "https://example.com/blog/channels/notes.xml");
    assert_eq!(atom.authors()[0].name(), "Alice");
    assert_eq!(atom.authors()[0].email(), Some("alice@example.com"));
}

#[test]
fn empty_authors_refused() {
    let err = declaration_error(
        r#"#document("index.html")[Home] #feed(format: "atom", authors: ())"#,
        SITE_SETTINGS,
    );
    assert!(
        err.source_diagnostic()
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
    let json: serde_json::Value = serde_json::from_slice(output(&outputs, "/feed.json")).unwrap();
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
    let body: serde_json::Value = serde_json::from_slice(output(&outputs, "/body.json")).unwrap();
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
    let json: serde_json::Value = serde_json::from_slice(output(&outputs, "/feed.json")).unwrap();
    let html = json["items"][0]["content_html"].as_str().unwrap();
    assert!(html.contains("<style>"), "{html}");
    assert!(html.contains("mfrac"), "{html}");
    assert!(html.contains("<math"), "{html}");
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
    SiteConfig,
    Arc<tola_typst::BundleCompilation>,
) {
    let (dir, cfg, compilation) = compile(SEO_CACHE_SOURCE, SEO_CACHE_CONFIG);
    (dir, cfg, Arc::new(compilation))
}

fn assert_seo_matches_fresh(
    prepared: &SeoCompilation,
    cfg: &SiteConfig,
    compilation: &tola_typst::BundleCompilation,
) {
    let fresh = render_outputs(cfg, compilation, &BuildCancellation::default()).unwrap();
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
fn seo_reuse_matches_inputs() {
    let (_dir, cfg, compilation) = seo_cache_site();
    let cancel = BuildCancellation::default();
    let original = SeoCompilation::prepare(&cfg, &compilation, &cancel, None).unwrap();
    assert_seo_matches_fresh(&original, &cfg, &compilation);
    assert_eq!(
        original
            .outputs()
            .iter()
            .map(|out| out.url.as_str())
            .collect::<Vec<_>>(),
        ["/feed.atom", "/feed.json", "/feed.xml", "/sitemap.xml"]
    );
    assert_eq!(original.warnings().len(), 2);
    assert!(
        original
            .warnings()
            .iter()
            .all(|w| w.source().span.id().is_some()
                && w.source().severity == typst::diag::Severity::Warning)
    );
    let json: serde_json::Value = serde_json::from_slice(&original.outputs()[1].bytes).unwrap();
    let html = json["items"][0]["content_html"].as_str().unwrap();
    assert!(html.contains("href=\"https://[\""), "{html}");
    assert!(html.contains("src=\"https://[image\""), "{html}");

    let reused = SeoCompilation::prepare(&cfg, &compilation, &cancel, Some(&original)).unwrap();
    assert!(Arc::ptr_eq(&original, &reused));
    assert_seo_matches_fresh(&reused, &cfg, &compilation);

    let mut unrelated = cfg.clone();
    unrelated.site.title = "Revised feed".into();
    unrelated.site.description = "Revised description".into();
    unrelated.site.language = LanguageDeclaration::Tag("fr".into());
    unrelated.build.references.fragments = ReferenceLevel::Warn;
    let reused =
        SeoCompilation::prepare(&unrelated, &compilation, &cancel, Some(&original)).unwrap();
    assert!(Arc::ptr_eq(&original, &reused));
    assert_seo_matches_fresh(&reused, &unrelated, &compilation);

    type SiteSettingChange = fn(&mut SiteConfig);
    let changes: [(&str, SiteSettingChange); 2] = [
        ("origin", |cfg| {
            cfg.site.origin = Some("https://elsewhere.test".into())
        }),
        ("base-path", |cfg| {
            cfg.site.base_path = SiteUrlMount::from_base_path("/notes/").unwrap()
        }),
    ];
    for (field, change) in changes {
        let mut changed = cfg.clone();
        change(&mut changed);
        let updated =
            SeoCompilation::prepare(&changed, &compilation, &cancel, Some(&original)).unwrap();
        assert!(!Arc::ptr_eq(&original, &updated), "{field}");
        assert_seo_matches_fresh(&updated, &changed, &compilation);
        assert_ne!(
            updated.outputs()[0].bytes,
            original.outputs()[0].bytes,
            "{field}"
        );
    }

    let (_eq_dir, _, eq_bundle) = compile(SEO_CACHE_SOURCE, SEO_CACHE_CONFIG);
    let eq_bundle = Arc::new(eq_bundle);
    let equivalent = SeoCompilation::prepare(&cfg, &eq_bundle, &cancel, Some(&original)).unwrap();
    assert!(!Arc::ptr_eq(&original, &equivalent));
    assert_seo_matches_fresh(&equivalent, &cfg, &eq_bundle);
    assert_eq!(equivalent.outputs()[0].bytes, original.outputs()[0].bytes);

    let (_changed_dir, _, changed_bundle) = compile(
        &SEO_CACHE_SOURCE.replace("Document body", "Revised body"),
        SEO_CACHE_CONFIG,
    );
    let changed_bundle = Arc::new(changed_bundle);
    let updated = SeoCompilation::prepare(&cfg, &changed_bundle, &cancel, Some(&original)).unwrap();
    assert!(!Arc::ptr_eq(&original, &updated));
    assert_seo_matches_fresh(&updated, &cfg, &changed_bundle);
    assert_ne!(updated.outputs()[0].bytes, original.outputs()[0].bytes);
}

#[test]
fn failed_seo_skips_reuse() {
    let (_dir, cfg, compilation) = seo_cache_site();
    let cancel = BuildCancellation::default();
    let original = SeoCompilation::prepare(&cfg, &compilation, &cancel, None).unwrap();

    let changes: [fn(&mut SiteConfig); 3] = [
        |cfg| cfg.build.references.navigation = ReferenceLevel::Error,
        |cfg| cfg.build.references.resources = ReferenceLevel::Error,
        |cfg| cfg.site.origin = None,
    ];
    for change in changes {
        let mut changed = cfg.clone();
        change(&mut changed);
        let fresh = render_outputs(&changed, &compilation, &cancel).unwrap_err();
        let attempted =
            SeoCompilation::prepare(&changed, &compilation, &cancel, Some(&original)).unwrap_err();
        let (RenderError::Declaration(fresh), RenderError::Declaration(attempted)) =
            (fresh, attempted)
        else {
            panic!("expected source-attached declaration errors");
        };
        assert_eq!(attempted.source_diagnostic(), fresh.source_diagnostic());
    }

    let (_failed_dir, _, failed_bundle) = compile(
        &format!("{SEO_CACHE_SOURCE}\n#metadata(42) <tola-feed>"),
        SEO_CACHE_CONFIG,
    );
    assert!(matches!(
        SeoCompilation::prepare(&cfg, &Arc::new(failed_bundle), &cancel, Some(&original)),
        Err(RenderError::Declaration(_))
    ));

    let recovered = SeoCompilation::prepare(&cfg, &compilation, &cancel, Some(&original)).unwrap();
    assert!(Arc::ptr_eq(&original, &recovered));
    assert_seo_matches_fresh(&recovered, &cfg, &compilation);
}

#[test]
fn cancelled_seo_skips_reuse() {
    let (_dir, cfg, compilation) = seo_cache_site();
    let original =
        SeoCompilation::prepare(&cfg, &compilation, &BuildCancellation::default(), None).unwrap();
    let canceller = crate::cancellation::BuildCanceller::default();
    let cancel = canceller.token();
    canceller.cancel();

    for previous in [None, Some(&original)] {
        assert!(matches!(
            SeoCompilation::prepare(&cfg, &compilation, &cancel, previous),
            Err(RenderError::Cancelled(_))
        ));
    }
}

#[test]
fn nested_content_error_keeps_pointer() {
    use std::error::Error;

    let err = declaration_error(
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
    let violation = err
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
    let msg = err.to_string();
    assert!(msg.contains("/entries/0/summary/body/delta"));
    assert_eq!(msg.matches("/entries/0/summary").count(), 1);
}

#[test]
fn feeds_separate_summary_and_content() {
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
    let json: serde_json::Value = serde_json::from_slice(output(&outputs, "/feed.json")).unwrap();
    assert_eq!(json["items"][0]["summary"], "Next");
    let content_html = json["items"][0]["content_html"].as_str().unwrap();
    assert!(
        content_html.contains("<strong>Full body</strong>"),
        "{content_html}"
    );
    assert!(
        content_html.contains("https://example.com/outside/"),
        "content resolves its own link: {content_html}"
    );
    let rss = rss::Channel::read_from(output(&outputs, "/feed.xml")).unwrap();
    let desc = rss.items()[0].description().unwrap();
    assert!(desc.contains("https://example.com/blog/next/"), "{desc}");
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
    let json: serde_json::Value = serde_json::from_slice(output(&outputs, "/feed.json")).unwrap();
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
    let json: serde_json::Value = serde_json::from_slice(output(&outputs, "/feed.json")).unwrap();
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
    let json: serde_json::Value = serde_json::from_slice(output(&outputs, "/feed.json")).unwrap();
    assert_eq!(
        json["items"][0]["url"],
        "https://example.com/blog/paper.pdf#page=3&view=Fit"
    );
}

#[test]
fn sitemaps_keep_own_entries() {
    let outputs = render(
        r#"
#document("a/index.html")[A] <a>
#document("b/index.html")[B] <b>
#document("c/index.html")[C]
#context sitemap(output: "first.xml", targets: ((target: locate(<b>), lastmod: datetime(year: 2026, month: 9, day: 1)), <a>))
#sitemap(output: "second.xml", targets: ("c/index.html",))
"#,
    );
    assert!(outputs.warnings.is_empty());
    assert_eq!(outputs.outputs.len(), 2);
    let xml = std::str::from_utf8(output(&outputs, "/first.xml")).unwrap();
    let doc = roxmltree::Document::parse(xml).unwrap();
    let locs = doc
        .descendants()
        .filter(|n| n.has_tag_name(("http://www.sitemaps.org/schemas/sitemap/0.9", "loc")))
        .map(|n| n.text().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        locs,
        ["https://example.com/blog/b/", "https://example.com/blog/a/"]
    );
    assert!(xml.contains("<lastmod>2026-09-01</lastmod>"));
    let second = std::str::from_utf8(output(&outputs, "/second.xml")).unwrap();
    assert!(second.contains("https://example.com/blog/c/"));
}

#[test]
fn sitemap_rejects_element_anchors() {
    for target in [
        "<part>",
        "locate(<part>)",
        "(output: \"index.html\", fragment: \"absent\")",
    ] {
        let err = declaration_error(
            &format!(
                "#document(\"index.html\")[= Part <part>]\n#context sitemap(targets: ((target: {target}),))"
            ),
            "[site]\norigin = \"https://example.com\"",
        );
        assert!(err.to_string().contains("complete document"), "{err}");
    }
}

#[test]
fn sitemap_lastmod_preserves_format() {
    let outputs = render(
        r#"
#document("index.html")[Home]
#for (output, lastmod) in (
  ("native-date.xml", datetime(year: 2024, month: 2, day: 29)),
  ("string-date.xml", "2024-02-29"),
  ("native-time.xml", datetime(year: 2026, month: 9, day: 1, hour: 6, minute: 7, second: 8)),
  ("string-time.xml", "2026-09-01T06:07:08.123456789123+08:00"),
) {
  sitemap(output: output, targets: ((target: "index.html", lastmod: lastmod),))
}
"#,
    );
    for (path, date) in [
        ("/native-date.xml", "2024-02-29"),
        ("/string-date.xml", "2024-02-29"),
        ("/native-time.xml", "2026-09-01T06:07:08Z"),
        ("/string-time.xml", "2026-09-01T06:07:08.123456789+08:00"),
    ] {
        let xml = std::str::from_utf8(output(&outputs, path)).unwrap();
        let doc = roxmltree::Document::parse(xml).unwrap();
        let lastmod = doc
            .descendants()
            .find(|n| n.has_tag_name(("http://www.sitemaps.org/schemas/sitemap/0.9", "lastmod")))
            .unwrap();
        assert_eq!(lastmod.text(), Some(date));
    }
}

#[test]
fn date_errors_name_declaration_fields() {
    for date in [
        "\" 2026-09-01T06:07:08Z\"",
        "\"2026-09-01T06:07:08+0800\"",
        "\"2016-12-31T23:59:60Z\"",
        "\"2026-02-29\"",
        "datetime(hour: 6, minute: 7, second: 8)",
    ] {
        for (decl, field) in [
            (
                format!(
                    "#feed(entries: ((id: \"urn:post\", target: \"index.html\", published: {date}),))"
                ),
                "/entries/0/published",
            ),
            (
                format!("#sitemap(targets: ((target: \"index.html\", lastmod: {date}),))"),
                "/targets/0/lastmod",
            ),
        ] {
            let err = declaration_error(
                &format!("#document(\"index.html\", title: [Home])[Home]\n{decl}"),
                "[site]\norigin = \"https://example.com\"\ntitle = \"Notes\"",
            );
            assert!(err.to_string().contains(field), "{err}");
            assert!(err.source_diagnostic().source().span.id().is_some());
        }
    }
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
fn declaration_errors_name_offending_field() {
    const SETTINGS: &str = "[site]\norigin = \"https://example.com\"\ntitle = \"Home\"";
    for (decl, pointer) in [
        (
            r#"#document("index.html", title: [Home])[Home]
#feed(entries: ((target: "index.html",),))"#,
            "/entries/0/published",
        ),
        (
            r#"#document("index.html", title: [Home])[Home]
#feed(format: "json", output: "feed.xml", entries: ())"#,
            "/output",
        ),
        (
            r#"#document("index.html")[#html.p(id: "part")[Part]]
#sitemap(targets: ((target: (output: "index.html", fragment: "part"),),))"#,
            "/targets/0",
        ),
        (
            r#"#document("index.html")[Home]
#sitemap(output: "_tola/sitemap.xml", targets: ("index.html",))"#,
            "/output",
        ),
        (
            r#"#document("index.html")[Home]
#feed(output: "_tola/feed.xml", entries: ())"#,
            "/output",
        ),
    ] {
        let err = declaration_error(decl, SETTINGS);
        assert!(
            err.source_diagnostic().source().message.contains(pointer),
            "{decl}: expected `{pointer}` in {}",
            err.source_diagnostic().source().message
        );
    }

    let (_dir, cfg, compilation) = compile(
        r#"#document("index.html")[Home] #sitemap(targets: ("index.html",))"#,
        "",
    );
    let err = render_outputs(&cfg, &compilation, &BuildCancellation::default())
        .err()
        .unwrap();
    assert!(err.to_string().contains("site.origin"));
}
