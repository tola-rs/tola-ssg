use super::*;
use typst::foundations::Value;
use typst::introspection::Location;

struct QuerySite {
    directory: tempfile::TempDir,
    library: tola_packages::library::SiteLibrary,
    files: Arc<tola_typst::FileResolver>,
    cache: Arc<tola_typst::SharedFileCache>,
}

impl QuerySite {
    fn new(base_path: &str) -> Self {
        let directory = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(directory.path().join("content")).unwrap();
        let library = Self::library_for_mount(directory.path(), base_path);
        Self {
            directory,
            library,
            files: Arc::new(tola_typst::FileResolver::new().with_provider(IconFiles::default())),
            cache: Arc::new(tola_typst::SharedFileCache::new()),
        }
    }

    fn library_for_mount(root: &Path, base_path: &str) -> tola_packages::library::SiteLibrary {
        let config = crate::config::tests::load_test_config(
            root,
            &format!("[site]\norigin = \"https://example.test\"\nbase-path = \"{base_path}\"\n"),
        );
        crate::package::SiteBindings::from_config(&config, Default::default())
            .library(Default::default())
    }

    /// The world reading this site's program, with its own or the shared file cache.
    fn world(&self, source: &str, shared_cache: bool) -> tola_typst::TypstWorld {
        let main = self.directory.path().join("site.typ");
        std::fs::write(&main, source).unwrap();
        let world = tola_typst::TypstWorld::builder(&main, self.directory.path())
            .with_files(Arc::clone(&self.files))
            .with_shared_library(self.library.shared())
            .no_fonts();
        if shared_cache {
            world.with_shared_cache(Arc::clone(&self.cache))
        } else {
            world.with_local_cache()
        }
        .build(&tola_typst::BundleCancellation::default())
        .unwrap()
    }

    fn compile(&self, source: &str, shared_cache: bool) -> tola_typst::BundleCompilation {
        tola_typst::compile_bundle_world(&self.world(source, shared_cache), &Default::default())
            .unwrap()
    }
}

fn records(compilation: &tola_typst::BundleCompilation, document: &str, label: &str) -> Vec<Dict> {
    let path = typst::syntax::VirtualPath::new(document).unwrap();
    compilation
        .document(&path)
        .unwrap()
        .metadata_unique(label)
        .unwrap()
        .unwrap()
        .cast::<Array>()
        .unwrap()
        .into_iter()
        .map(|value| value.cast::<Dict>().unwrap())
        .collect()
}

fn document(reference: &Dict) -> Dict {
    reference
        .get("document")
        .unwrap()
        .clone()
        .cast::<Dict>()
        .unwrap()
}

fn target(reference: &Dict) -> Dict {
    reference
        .get("target")
        .unwrap()
        .clone()
        .cast::<Dict>()
        .unwrap()
}

/// The site program listing every heading `body` writes through `headings()`.
fn heading_listing(body: &str) -> String {
    format!(
        r#"#import "@tola/document:0.0.0": headings
#document("a/index.html")[
{body}  #context [#metadata(headings()) <headings>]
]
"#
    )
}

type LevelAndNesting = (u64, Option<u64>);

/// The unresolved link carries the reason no target matched.
#[test]
fn every_link_gets_one_classification() {
    let site = QuerySite::new("/");
    let compiled = site.compile(
        r#"
#import "@tola/document:0.0.0": references
#document("a/index.html")[
  #html.main[
    #link("../b/#intro")[URL]
    #link(<intro>)[Label]
    #link("https://remote.test/elsewhere")[External]
    #link("https://example.test/b/")[Own origin]
    #link("/missing/")[Missing]
  ] <page>
  #context [#metadata(references(from: auto, from-within: <page>)) <outgoing>]
]
#document("b/index.html")[
  = Introduction <intro>
]
"#,
        true,
    );
    let outgoing = records(&compiled, "a/index.html", "outgoing");
    let resolutions = outgoing
        .iter()
        .map(|reference| reference.get("resolution").unwrap().clone())
        .collect::<Vec<_>>();
    assert_eq!(
        resolutions,
        ["found", "found", "external", "external", "unresolved"].map(|value| value.into_value())
    );
    let reason = outgoing[4]
        .get("reason")
        .unwrap()
        .clone()
        .cast::<Dict>()
        .unwrap();
    assert_eq!(reason.get("tag").unwrap(), &"no-such-target".into_value());
}

#[test]
fn to_filter_selects_content() {
    let site = QuerySite::new("/");
    let compiled = site.compile(
        r#"
#import "@tola/document:0.0.0": references
#document("a/index.html")[#html.main[see #link(<intro>)[intro] and #link("/b/")[page]] <page>]
#document("b/index.html")[
  #html.main[
    = Intro <intro>
    #context [
      #metadata(references(to: <intro>)) <section>
      #metadata(references(to: auto)) <document>
      #metadata(references(to: none)) <anywhere>
    ]
  ] <page>
]
"#,
        true,
    );
    assert_eq!(records(&compiled, "b/index.html", "section").len(), 1);
    assert_eq!(records(&compiled, "b/index.html", "document").len(), 2);
    assert_eq!(records(&compiled, "b/index.html", "anywhere").len(), 2);
}

/// `from-within` takes a label or a location, and restricts the query to that region.
#[test]
fn source_region_accepts_label() {
    let program = |scope: Option<&str>| {
        let argument = scope.map_or_else(String::new, |scope| format!("from-within: {scope}"));
        format!(
            r#"
#import "@tola/document:0.0.0": references
#document("a/index.html")[
  #link("/b/")[Navigation]
  #html.main[#link("/b/")[Body]] <page>
  #context [#metadata(references({argument})) <region>]
]
#document("b/index.html")[B]
"#
        )
    };
    for scope in ["<page>", "query(selector(<page>)).first().location()"] {
        let site = QuerySite::new("/");
        let scoped = records(
            &site.compile(&program(Some(scope)), true),
            "a/index.html",
            "region",
        );
        assert_eq!(
            scoped.len(),
            1,
            "`from-within: {scope}` keeps only the region's reference"
        );

        let unscoped = records(
            &site.compile(&program(None), true),
            "a/index.html",
            "region",
        );
        assert_eq!(
            unscoped.len(),
            2,
            "without `from-within` the reference outside the region counts too"
        );
    }
}

#[test]
fn location_scopes_the_reference_query() {
    let site = QuerySite::new("/");
    for (program, scope_reads_document) in [
        (
            r#"
#import "@tola/document:0.0.0": headings, references
#document("b/index.html")[#html.main[#link(<intro>)[jump]] <page>]
#document("a/index.html")[
  = Intro <intro>
  #context [#metadata(references(to: headings().first().location)) <refs>]
]
"#,
            false,
        ),
        (
            r#"
#import "@tola/document:0.0.0": headings, references
#document("a/index.html")[
  = Intro <intro>
  #link("/b/")[to b]
  #context [#metadata(references(from: headings().first().location)) <refs>]
]
#document("b/index.html")[#link("/a/")[to a]]
"#,
            true,
        ),
    ] {
        let compiled = site.compile(program, true);
        let found = records(&compiled, "a/index.html", "refs");
        assert_eq!(found.len(), 1, "{found:?}");
        let chosen = if scope_reads_document {
            document(&found[0])
        } else {
            target(&found[0])
        };
        assert_eq!(chosen.get("output").unwrap(), &"a/index.html".into_value());
    }
}

#[test]
fn nesting_follows_outline_tree() {
    // Each shape: the headings it writes, and every heading's declared level with the
    // depth `outline()` draws it at.
    let shapes: [(&str, &[LevelAndNesting]); 4] = [
        (
            "  = One\n  == Two\n  === Deep\n",
            &[(1, Some(1)), (2, Some(2)), (3, Some(3))],
        ),
        (
            "  = One\n  == Two\n  #heading(depth: 2, outlined: false)[Hidden]\n  === Deep\n",
            &[(1, Some(1)), (2, Some(2)), (2, None), (3, Some(2))],
        ),
        (
            "  = One\n  #heading(depth: 2, outlined: false)[Hidden]\n  === Deep\n",
            &[(1, Some(1)), (2, None), (3, Some(2))],
        ),
        ("  = One\n  === Deep\n", &[(1, Some(1)), (3, Some(2))]),
    ];
    for (body, expected) in shapes {
        let site = QuerySite::new("/");
        let compiled = site.compile(&heading_listing(body), true);
        let listed: Vec<LevelAndNesting> = records(&compiled, "a/index.html", "headings")
            .iter()
            .map(|heading| {
                (
                    heading.get("level").unwrap().clone().cast::<u64>().unwrap(),
                    heading
                        .get("nesting")
                        .unwrap()
                        .clone()
                        .cast::<Option<u64>>()
                        .unwrap(),
                )
            })
            .collect();
        assert_eq!(listed, expected, "{body}");
    }
}

#[test]
fn heading_body_preserves_source_content() {
    let site = QuerySite::new("/");
    site.compile(
        r#"
#import "@tola/document:0.0.0": headings
#document("index.html")[
  #show heading: it => [Replaced]
  = Introduction
  == *Bold* and _em_
  #context {
    assert.eq(
      headings().map(section => section.body),
      ([Introduction], [*Bold* and _em_]),
    )
  }
]
"#,
        true,
    );
}

/// A show rule decides which link reaches the output, and that link is what is reported.
#[test]
fn show_rule_picks_reported_link() {
    let site = QuerySite::new("/");
    let compiled = site.compile(
        r#"
#import "@tola/document:0.0.0": references
#document("from.html")[
  #show link.where(dest: "/original/"): _ => link("/generated/")[Generated]
  #html.main[#link("/original/")[Original]] <page>
  #context [#metadata(references(from-within: <page>)) <outgoing>]
]
#document("original.html")[
  = Original <original>
  #context [#metadata(references(to: auto)) <incoming>]
]
#document("generated/index.html")[
  Generated
  #context [#metadata(references(to: auto)) <incoming>]
]
"#,
        true,
    );
    let outgoing = records(&compiled, "from.html", "outgoing");
    assert_eq!(outgoing.len(), 1, "{outgoing:?}");
    assert_eq!(
        outgoing[0].get("destination").unwrap(),
        &"/generated/".into_value()
    );
    assert_eq!(
        target(&outgoing[0]).get("output").unwrap(),
        &"generated/index.html".into_value()
    );
    assert!(records(&compiled, "original.html", "incoming").is_empty());
    assert_eq!(
        records(&compiled, "generated/index.html", "incoming").len(),
        1
    );
}

/// A link written inside another link's body belongs to the outer one.
/// A reference a show rule replaces is reported as what replaced it.
/// Links a show rule produces are reported in the region that holds them.
/// Occurrences stay in the region that holds them, wherever a derived query reads them.
/// A chain of show rules reports the reference that finally reached the output.
#[test]
fn missing_links_stay_unreported() {
    let site = QuerySite::new("/");
    for rewritten in [
        "#show link: it => it.body",
        r#"#show link: it => html.a(href: "/generated/")[Raw]"#,
    ] {
        let compiled = site.compile(
            &format!(
                r#"
#import "@tola/document:0.0.0": references
#document("from.html")[
  {rewritten}
  #html.main[#link("/generated/")[Generated]] <page>
  #context [#metadata(references(from: auto)) <outgoing>]
]
#document("generated/index.html")[Generated]
"#
            ),
            true,
        );
        assert!(
            records(&compiled, "from.html", "outgoing").is_empty(),
            "{rewritten}"
        );
    }
}

#[test]
fn escaped_urls_preserve_output_identity() {
    let cases = [
        (
            r#"
#import "@tola/document:0.0.0": references
#document("100%-ready/index.html")[
  #context [#metadata(references(to: auto)) <incoming>]
]
#document("index.html")[#link("/100%25-ready/")[Ready]]
"#,
            "100%-ready/index.html",
            "100%-ready/index.html",
        ),
        (
            r#"
#import "@tola/document:0.0.0": references
#document("a#b/index.html")[
  #context [#metadata(references(to: auto)) <incoming>]
]
#document("index.html")[#link("/a%23b/")[Hash]]
"#,
            "a#b/index.html",
            "a#b/index.html",
        ),
        (
            // `%2541` escapes a literal `%41`, which must not decode to `aA`.
            r#"
#import "@tola/document:0.0.0": references
#document("a%41", format: "html")[
  #context [#metadata(references(to: auto)) <incoming>]
]
#document("aA/index.html")[A]
#document("index.html")[#link("/a%2541")[Escaped percent]]
"#,
            "a%41",
            "a%41",
        ),
    ];
    for (program, document_path, output_name) in cases {
        let site = QuerySite::new("/");
        let compiled = site.compile(program, true);
        let incoming = records(&compiled, document_path, "incoming");
        assert_eq!(incoming.len(), 1, "{incoming:?}");
        assert_eq!(
            incoming[0].get("resolution").unwrap(),
            &"found".into_value()
        );
        assert_eq!(
            target(&incoming[0]).get("output").unwrap(),
            &output_name.into_value()
        );
    }
}

#[test]
fn url_targets_carry_output_identity() {
    let site = QuerySite::new("/");
    let compiled = site.compile(
        r#"
#import "@tola/document:0.0.0": references
#document("index.html")[
  #link(<target>)[Native target]
  #link("/target.html#a%2541")[URL target]
  #context [#metadata(references(from: auto)) <outgoing>]
]
#document("target.html")[
  #html.main[#html.div(id: "a%41")[Target] <target>] <body>
  #context [
    #metadata(references(to: <target>)) <by-label>
    #metadata(references(to: query(<target>).first().location())) <by-location>
    #metadata(references(to: <target-doc>)) <by-document>
    #metadata(references(to: <body>)) <body-endpoint>
    #metadata(references(to-within: <body>)) <body-targets>
    #metadata(references(to-within: <target>)) <target-descendants>
  ]
] <target-doc>
"#,
        true,
    );
    let native_location = records(&compiled, "target.html", "by-label")[0]
        .get("target")
        .unwrap()
        .clone()
        .cast::<Dict>()
        .unwrap()
        .get("location")
        .unwrap()
        .clone()
        .cast::<Location>()
        .unwrap();
    for filter in ["by-label", "by-location", "body-targets"] {
        let incoming = records(&compiled, "target.html", filter);
        assert_eq!(incoming.len(), 1, "{filter}: {incoming:?}");
        let target = target(&incoming[0]);
        assert_eq!(target.get("kind").unwrap(), &"element".into_value());
        assert_eq!(
            target.get("location").unwrap(),
            &native_location.into_value()
        );
        assert_eq!(target.get("fragment").unwrap(), &"a%41".into_value());
    }
    for filter in ["body-endpoint", "target-descendants"] {
        assert!(records(&compiled, "target.html", filter).is_empty());
    }
    for (output, label) in [("index.html", "outgoing"), ("target.html", "by-document")] {
        let incoming = records(&compiled, output, label);
        assert_eq!(incoming.len(), 2);
        let url = target(&incoming[1]);
        assert_eq!(url.get("kind").unwrap(), &"url".into_value());
        assert_eq!(url.get("location").unwrap(), &Value::None);
        assert_eq!(url.get("fragment").unwrap(), &"a%41".into_value());
    }
}

#[test]
fn records_resolve_mounted_addresses() {
    let site = QuerySite::new("/docs/");
    let compiled = site.compile(
        r#"
#import "@tola/document:0.0.0": references
#document("index.html")[
  #link("/docs/guide/")[Clean]
  #link("/docs/guide/index.html?lang=en#part%20one")[Output alias]
  #link("/docs/page.html/")[Not a directory]
  #link("/docs/payload.json?version=2#entry")[Asset]
  #context [
    #metadata(references()) <outgoing>
  ]
]
#document("guide/index.html")[Guide]
#document("page.html")[Page]
#asset("payload.json", "{}")
"#,
        true,
    );
    let outgoing = records(&compiled, "index.html", "outgoing");
    assert_eq!(outgoing.len(), 4);
    assert_eq!(
        target(&outgoing[0]).get("output").unwrap(),
        &"guide/index.html".into_value()
    );
    assert_eq!(
        target(&outgoing[1]).get("route").unwrap(),
        &"/guide/".into_value()
    );
    assert_eq!(
        target(&outgoing[1]).get("fragment").unwrap(),
        &"part one".into_value()
    );
    assert_eq!(
        outgoing[2].get("resolution").unwrap(),
        &"unresolved".into_value()
    );
    assert_eq!(
        target(&outgoing[3]).get("output").unwrap(),
        &"payload.json".into_value()
    );
    assert_eq!(
        target(&outgoing[3]).get("query").unwrap(),
        &"version=2".into_value()
    );
}

#[test]
fn origin_matched_url_stays_external() {
    let site = QuerySite::new("/docs/");
    let compiled = site.compile(
        r#"
#import "@tola/document:0.0.0": references
#document("a/index.html")[
  #link("https://example.test/docs/b/")[Literal]
  #link("https://example.test/elsewhere/")[Outside the mount]
  #context [#metadata(references(from: auto)) <outgoing>]
]
#document("b/index.html")[
  #context [#metadata(references(to: auto)) <incoming>]
]
"#,
        true,
    );
    // No destination names this build's outputs, so neither reaches them.
    assert!(records(&compiled, "b/index.html", "incoming").is_empty());
    let written = records(&compiled, "a/index.html", "outgoing");
    assert_eq!(written.len(), 2, "{written:?}");
    for reference in &written {
        assert_eq!(
            reference.get("resolution").unwrap(),
            &"external".into_value()
        );
        assert_eq!(reference.get("target").unwrap(), &Value::None);
    }
}

#[test]
fn reference_queries_converge_without_warning() {
    let site = QuerySite::new("/");
    let compiled = site.compile(
        r#"
#import "@tola/document:0.0.0": references
#document("index.html")[
  #show link.where(dest: "/linked/"): _ => link("/other/")[Rewritten]
  #html.main[#link("/linked/")[Linked]] <body>
  #context {
    let written = references(from: auto, from-within: <body>)
    assert.eq(written.len(), 1)
    assert.eq(written.first().target.output, "other/index.html")
  }
]
#document("other/index.html")[
  Other
  #context {
    let incoming = references(to: auto, from-within: <body>)
    assert.eq(incoming.len(), 1)
  }
]
"#,
        true,
    );
    let messages = compiled
        .diagnostics()
        .raw()
        .iter()
        .map(|diagnostic| diagnostic.source().message.to_string())
        .collect::<Vec<_>>();
    assert!(
        !messages
            .iter()
            .any(|message| message.contains("did not converge")),
        "{messages:?}"
    );
}
