use typst::foundations::Value;
use typst::introspection::Location;

use super::*;

struct QuerySite {
    dir: tempfile::TempDir,
    lib: tola_packages::library::SiteLibrary,
    files: Arc<tola_typst::FileStore>,
    cache: Arc<tola_typst::SharedFileCache>,
}

impl QuerySite {
    fn new(base_path: &str) -> Self {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("content")).unwrap();
        let lib = Self::library_for_mount(dir.path(), base_path);
        Self {
            dir,
            lib,
            files: Arc::new(tola_typst::FileStore::new().with_provider(IconFiles::default())),
            cache: Arc::new(tola_typst::SharedFileCache::new()),
        }
    }

    fn library_for_mount(root: &Path, base_path: &str) -> tola_packages::library::SiteLibrary {
        let cfg = crate::config::tests::load_test_config(
            root,
            &format!("[site]\norigin = \"https://example.test\"\nbase-path = \"{base_path}\"\n"),
        );
        crate::package::SiteBindings::from_config(&cfg, Default::default(), None)
            .library(Default::default())
    }

    fn world(&self, src: &str, shared_cache: bool) -> tola_typst::TypstWorld {
        let main = self.dir.path().join("site.typ");
        std::fs::write(&main, src).unwrap();
        let world = tola_typst::TypstWorld::builder(&main, self.dir.path())
            .with_files(Arc::clone(&self.files))
            .with_shared_library(self.lib.shared())
            .no_fonts();
        if shared_cache {
            world.with_shared_cache(Arc::clone(&self.cache))
        } else {
            world.with_local_cache()
        }
        .build(&tola_typst::BundleCancellation::default())
        .unwrap()
    }

    fn compile(&self, src: &str, shared_cache: bool) -> tola_typst::BundleCompilation {
        tola_typst::compile_bundle_world(&self.world(src, shared_cache), &Default::default())
            .unwrap()
    }
}

fn records(comp: &tola_typst::BundleCompilation, doc: &str, label: &str) -> Vec<Dict> {
    let path = typst::syntax::VirtualPath::new(doc).unwrap();
    comp.document(&path)
        .unwrap()
        .metadata_unique(label)
        .unwrap()
        .unwrap()
        .cast::<Array>()
        .unwrap()
        .into_iter()
        .map(|val| val.cast::<Dict>().unwrap())
        .collect()
}

fn doc_dict(ref_: &Dict) -> Dict {
    ref_.get("document")
        .unwrap()
        .clone()
        .cast::<Dict>()
        .unwrap()
}

fn target_dict(ref_: &Dict) -> Dict {
    ref_.get("target").unwrap().clone().cast::<Dict>().unwrap()
}

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

#[test]
fn links_single_classification() {
    let site = QuerySite::new("/");
    let comp = site.compile(
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
    let outgoing = records(&comp, "a/index.html", "outgoing");
    let resolutions: Vec<_> = outgoing
        .iter()
        .map(|r| r.get("resolution").unwrap().clone())
        .collect();
    assert_eq!(
        resolutions,
        ["found", "found", "external", "external", "unresolved"].map(|v| v.into_value())
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
fn to_filter_selection() {
    let site = QuerySite::new("/");
    let comp = site.compile(
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
    assert_eq!(records(&comp, "b/index.html", "section").len(), 1);
    assert_eq!(records(&comp, "b/index.html", "document").len(), 2);
    assert_eq!(records(&comp, "b/index.html", "anywhere").len(), 2);
}

#[test]
fn from_within_accepts_label_or_loc() {
    let program = |scope: Option<&str>| {
        let arg = scope.map_or_else(String::new, |s| format!("from-within: {s}"));
        format!(
            r#"
#import "@tola/document:0.0.0": references
#document("a/index.html")[
  #link("/b/")[Navigation]
  #html.main[#link("/b/")[Body]] <page>
  #context [#metadata(references({arg})) <region>]
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
        assert_eq!(scoped.len(), 1, "{scope}");
        let unscoped = records(
            &site.compile(&program(None), true),
            "a/index.html",
            "region",
        );
        assert_eq!(unscoped.len(), 2);
    }
}

#[test]
fn location_scopes_query() {
    let site = QuerySite::new("/");
    for (program, reads_doc) in [
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
        let comp = site.compile(program, true);
        let found = records(&comp, "a/index.html", "refs");
        assert_eq!(found.len(), 1, "{found:?}");
        let chosen = if reads_doc {
            doc_dict(&found[0])
        } else {
            target_dict(&found[0])
        };
        assert_eq!(chosen.get("output").unwrap(), &"a/index.html".into_value());
    }
}

#[test]
fn heading_nesting_outline() {
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
        let comp = site.compile(&heading_listing(body), true);
        let listed: Vec<LevelAndNesting> = records(&comp, "a/index.html", "headings")
            .iter()
            .map(|h| {
                (
                    h.get("level").unwrap().clone().cast::<u64>().unwrap(),
                    h.get("nesting")
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
fn heading_body_source_content() {
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

#[test]
fn show_rule_selects_link() {
    let site = QuerySite::new("/");
    let comp = site.compile(
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
    let outgoing = records(&comp, "from.html", "outgoing");
    assert_eq!(outgoing.len(), 1, "{outgoing:?}");
    assert_eq!(
        outgoing[0].get("destination").unwrap(),
        &"/generated/".into_value()
    );
    assert_eq!(
        target_dict(&outgoing[0]).get("output").unwrap(),
        &"generated/index.html".into_value()
    );
    assert!(records(&comp, "original.html", "incoming").is_empty());
    assert_eq!(records(&comp, "generated/index.html", "incoming").len(), 1);
}

#[test]
fn rewritten_links_unreported() {
    let site = QuerySite::new("/");
    for rewritten in [
        "#show link: it => it.body",
        r#"#show link: it => html.a(href: "/generated/")[Raw]"#,
    ] {
        let comp = site.compile(
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
            records(&comp, "from.html", "outgoing").is_empty(),
            "{rewritten}"
        );
    }
}

#[test]
fn escaped_urls_output_identity() {
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
    for (program, doc_path, out_name) in cases {
        let site = QuerySite::new("/");
        let comp = site.compile(program, true);
        let incoming = records(&comp, doc_path, "incoming");
        assert_eq!(incoming.len(), 1, "{incoming:?}");
        assert_eq!(
            incoming[0].get("resolution").unwrap(),
            &"found".into_value()
        );
        assert_eq!(
            target_dict(&incoming[0]).get("output").unwrap(),
            &out_name.into_value()
        );
    }
}

#[test]
fn url_targets_output_identity() {
    let site = QuerySite::new("/");
    let comp = site.compile(
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
    let native_loc = records(&comp, "target.html", "by-label")[0]
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
        let incoming = records(&comp, "target.html", filter);
        assert_eq!(incoming.len(), 1, "{filter}: {incoming:?}");
        let target = target_dict(&incoming[0]);
        assert_eq!(target.get("kind").unwrap(), &"element".into_value());
        assert_eq!(target.get("location").unwrap(), &native_loc.into_value());
        assert_eq!(target.get("fragment").unwrap(), &"a%41".into_value());
    }
    for filter in ["body-endpoint", "target-descendants"] {
        assert!(records(&comp, "target.html", filter).is_empty());
    }
    for (out, label) in [("index.html", "outgoing"), ("target.html", "by-document")] {
        let incoming = records(&comp, out, label);
        assert_eq!(incoming.len(), 2);
        let url = target_dict(&incoming[1]);
        assert_eq!(url.get("kind").unwrap(), &"url".into_value());
        assert_eq!(url.get("location").unwrap(), &Value::None);
        assert_eq!(url.get("fragment").unwrap(), &"a%41".into_value());
    }
}

#[test]
fn records_mounted_addresses() {
    let site = QuerySite::new("/docs/");
    let comp = site.compile(
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
    let outgoing = records(&comp, "index.html", "outgoing");
    assert_eq!(outgoing.len(), 4);
    assert_eq!(
        target_dict(&outgoing[0]).get("output").unwrap(),
        &"guide/index.html".into_value()
    );
    assert_eq!(
        target_dict(&outgoing[1]).get("route").unwrap(),
        &"/guide/".into_value()
    );
    assert_eq!(
        target_dict(&outgoing[1]).get("fragment").unwrap(),
        &"part one".into_value()
    );
    assert_eq!(
        outgoing[2].get("resolution").unwrap(),
        &"unresolved".into_value()
    );
    assert_eq!(
        target_dict(&outgoing[3]).get("output").unwrap(),
        &"payload.json".into_value()
    );
    assert_eq!(
        target_dict(&outgoing[3]).get("query").unwrap(),
        &"version=2".into_value()
    );
}

#[test]
fn same_origin_url_external() {
    let site = QuerySite::new("/docs/");
    let comp = site.compile(
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
    assert!(records(&comp, "b/index.html", "incoming").is_empty());
    let written = records(&comp, "a/index.html", "outgoing");
    assert_eq!(written.len(), 2, "{written:?}");
    for r in &written {
        assert_eq!(r.get("resolution").unwrap(), &"external".into_value());
        assert_eq!(r.get("target").unwrap(), &Value::None);
    }
}

#[test]
fn reference_queries_converge() {
    let site = QuerySite::new("/");
    let comp = site.compile(
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
    let msgs: Vec<_> = comp
        .diagnostics()
        .raw()
        .iter()
        .map(|d| d.source().message.to_string())
        .collect();
    assert!(
        !msgs.iter().any(|m| m.contains("did not converge")),
        "{msgs:?}"
    );
}
