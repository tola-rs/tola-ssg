use tempfile::TempDir;

use crate::site::HtmlPage;

use super::*;

fn site_refs(site: &SiteIndex, mount: &SiteUrlMount, world: &tola_typst::TypstWorld) -> References {
    References::from_site(site, mount, world, &BuildCancellation::new(), None).unwrap()
}

fn candidate(
    src: &str,
) -> anyhow::Result<(
    TempDir,
    SiteIndex,
    crate::output::graph::OutputGraph,
    tola_typst::TypstWorld,
)> {
    let dir = TempDir::new().unwrap();
    let main = dir.path().join("site.typ");
    std::fs::write(&main, src).unwrap();
    let world = tola_typst::TypstWorld::builder(&main, dir.path())
        .with_local_cache()
        .no_fonts()
        .build(&tola_typst::BundleCancellation::default())
        .unwrap();
    let cancel = tola_typst::BundleCancellation::default();
    let compilation = tola_typst::compile_bundle_world(&world, &cancel).unwrap();
    let docs = crate::compiler::documents::from_compilation(&compilation, &cancel)?;
    let export = compilation.export_default(&cancel, None).unwrap();
    let mut graph = crate::output::graph::OutputGraphBuilder::new();
    crate::compiler::outputs::insert_bundle(
        &mut graph,
        export.entries().iter().cloned(),
        tola_minify::MinifiedLanguages::default(),
        &BuildCancellation::default(),
    )?;
    let graph = graph.finish();
    let site_index = SiteIndex::from_output_graph(&graph, docs)?;
    Ok((dir, site_index, graph, world))
}

fn reindex(
    src: &SiteIndex,
    graph: &crate::output::graph::OutputGraph,
    mut update_route: impl FnMut(&mut HtmlPage),
) -> SiteIndex {
    let html_outs: std::collections::BTreeSet<_> = graph
        .outputs()
        .iter()
        .filter(|out| out.kind() == crate::output::graph::OutputKind::HtmlDocument)
        .map(|out| out.path())
        .collect();
    let docs = src
        .html_pages()
        .filter(|(page, _)| html_outs.contains(&page.output))
        .map(|(page, inv)| {
            let mut page = page.clone();
            update_route(&mut page);
            crate::compiler::documents::RealizedPage {
                page,
                inventory: Arc::clone(inv),
            }
        });
    SiteIndex::from_output_graph(graph, docs).unwrap()
}

fn cached_and_fresh(
    site: &SiteIndex,
    mount: &SiteUrlMount,
    prev: &References,
    world: &tola_typst::TypstWorld,
) -> (References, References) {
    let cancel = BuildCancellation::new();
    let fresh = References::from_site(site, mount, world, &cancel, None).unwrap();
    let cached = References::from_site(site, mount, world, &cancel, Some(prev)).unwrap();
    (cached, fresh)
}

#[test]
fn cached_refs_match_fresh() {
    let (_dir, site, _graph, world) =
        candidate("#document(\"index.html\")[\n  #html.a(href: \"/missing/\")[Missing]\n]")
            .unwrap();
    let mount = SiteUrlMount::root();
    let prev = site_refs(&site, &mount, &world);
    assert_eq!(
        prev.references().next().unwrap().origin().unwrap().line,
        Some(2)
    );
    let shifted =
        "\n#document(\"index.html\")[\n  #html.a(href: \"/missing/\")[Missing]\n]".to_owned();
    let (_dir, changed, _graph, world) = candidate(&shifted).unwrap();
    let (cached, fresh) = cached_and_fresh(&changed, &mount, &prev, &world);
    assert_eq!(
        cached.references().collect::<Vec<_>>(),
        fresh.references().collect::<Vec<_>>()
    );
    assert_eq!(
        cached.references().next().unwrap().origin().unwrap().line,
        Some(3)
    );

    let src = |fragments: &str| {
        format!(
            r#"#document("kept.html")[#html.a(href: "/target/#kept")[Kept]]
#document("removed.html")[#html.a(href: "/target/#gone")[Gone]]
#document("missing.html")[
  #html.a(href: "/target/#missing")[First]
  #html.a(href: "/target/#missing")[Second]
]
#document("stable.html")[#html.a(href: "https://example.test/")[External]]
#document("target/index.html")[{fragments}]"#
        )
    };
    let (_dir, site, _graph, world) = candidate(&src(
        r#"#html.h1(id: "kept")[Kept] #html.h2(id: "gone")[Gone]"#,
    ))
    .unwrap();
    let mut prev = site_refs(&site, &mount, &world);
    for fragments in [
        r#"#html.h1(id: "kept")[Kept] #html.h2(id: "gone")[Gone] #html.h2(id: "extra")[Extra]"#,
        r#"#html.h1(id: "kept")[Kept] #html.h2(id: "extra")[Extra]"#,
        r#"#html.h1(id: "kept")[Kept] #html.h2(id: "missing")[Repaired]"#,
    ] {
        let (_dir, changed, _graph, world) = candidate(&src(fragments)).unwrap();
        let (cached, fresh) = cached_and_fresh(&changed, &mount, &prev, &world);
        assert_eq!(
            cached.references().collect::<Vec<_>>(),
            fresh.references().collect::<Vec<_>>()
        );
        prev = cached;
    }
    let missing = OutputPath::parse("missing.html").unwrap();
    assert!(
        prev.outgoing(&missing)
            .all(|r| matches!(r.resolution(), ReferenceResolution::Found { .. }))
    );
    let removed = OutputPath::parse("removed.html").unwrap();
    assert!(matches!(
        prev.outgoing(&removed).next().unwrap().resolution(),
        ReferenceResolution::Unresolved {
            reason: UnresolvedReferenceReason::FragmentMissing { .. }
        }
    ));

    use crate::config::ReferenceLevel;
    use tola_diagnostic::Severity;
    let (dir, site, _graph, world) = candidate(
        r#"#document("index.html")[
  #html.a(href: "/missing/")[Missing]
  #html.img(src: "/missing.png")
  #html.a(href: "/target/#missing")[Fragment]
]
#document("target/index.html")[#html.h1(id: "present")[Present]]"#,
    )
    .unwrap();
    let mut cfg = crate::config::tests::load_test_config(dir.path(), "");
    let prev = site_refs(&site, &mount, &world);
    let errors = diagnostic::diagnostics(&prev, &cfg, &BuildCancellation::new()).unwrap();
    assert_eq!(errors.len(), 3);
    assert!(errors.iter().all(|d| d.severity == Severity::Error));
    cfg.build.references.navigation = ReferenceLevel::Warn;
    cfg.build.references.resources = ReferenceLevel::Warn;
    cfg.build.references.fragments = ReferenceLevel::Warn;
    let (cached, fresh) = cached_and_fresh(&site, &mount, &prev, &world);
    assert_eq!(
        cached.references().collect::<Vec<_>>(),
        fresh.references().collect::<Vec<_>>()
    );
    let warnings = diagnostic::diagnostics(&cached, &cfg, &BuildCancellation::new()).unwrap();
    assert_eq!(warnings.len(), 3);
    assert!(warnings.iter().all(|d| d.severity == Severity::Warning));
}

#[test]
fn absolute_url_same_origin_external() {
    let (dir, site, _graph, world) = candidate(
        r#"#document("index.html")[#html.a(href: "https://example.test/guide/")[Absolute]]
#document("guide/index.html")[Guide]"#,
    )
    .unwrap();
    let mount = SiteUrlMount::root();
    let page = OutputPath::parse("index.html").unwrap();
    let cancel = BuildCancellation::new();
    let refs = References::from_site(&site, &mount, &world, &cancel, None).unwrap();
    assert_eq!(
        refs.outgoing(&page).next().unwrap().resolution(),
        &ReferenceResolution::External
    );
    let cfg = crate::config::tests::load_test_config(
        dir.path(),
        "[site]\norigin = \"https://example.test\"\n",
    );
    let diags = diagnostic::diagnostics(&refs, &cfg, &BuildCancellation::new()).unwrap();
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn uncheckable_and_missing_diagnosed() {
    let (dir, site, _graph, world) = candidate(
        r#"#document("index.html", html.html(
  html.head(html.base(href: "https://other.test/"))
  + html.body(html.a(href: "/missing/")[Missing])
))"#,
    )
    .unwrap();
    let index = site_refs(&site, &SiteUrlMount::root(), &world);
    let page = OutputPath::parse("index.html").unwrap();
    assert_eq!(
        index.outgoing(&page).next().unwrap().resolution(),
        &ReferenceResolution::External
    );
    let cfg = crate::config::tests::load_test_config(dir.path(), "");
    let diags = diagnostic::diagnostics(&index, &cfg, &BuildCancellation::new()).unwrap();
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(
        diags[0].code,
        tola_diagnostic::codes::reference::BASE_HREF_EXTERNAL_ORIGIN
    );

    let (dir, site, _graph, world) = candidate(
        r#"#document("index.html")[
  #html.a(href: "/nowhere.html")[Missing]
]"#,
    )
    .unwrap();
    let cfg = crate::config::tests::load_test_config(dir.path(), "");
    let index = site_refs(&site, &SiteUrlMount::root(), &world);
    let diags = diagnostic::diagnostics(&index, &cfg, &BuildCancellation::new()).unwrap();
    let loc = diags
        .first()
        .expect("missing page reported")
        .location
        .as_ref()
        .expect("reference located");
    assert_eq!(loc.path, "site.typ");
    assert_eq!((loc.line, loc.column), (Some(2), Some(4)));
    let range = loc.range.expect("authored range kept");
    assert_eq!((range.start.line, range.start.character), (1, 3));
    assert_eq!((range.end.line, range.end.character), (1, 41));
}

#[test]
fn cached_groups_alias_precedence() {
    use crate::output::graph::{OutputFile, OutputGraphBuilder};
    use crate::output::semantics::OutputDeclaration;

    let (_dir, src, orig, world) = candidate(
        r#"#document("links/index.html")[
  #html.a(href: "/target/index.html?view=card")[Alias]
  #html.a(href: "/target/")[Clean]
  #html.img(src: "/target/index.html")
]
#document("stable.html")[#html.a(href: "/stable-target/")[Stable]]
#document("stable-target/index.html")[Stable]
#document("target/index.html")[Target]"#,
    )
    .unwrap();
    let mount = SiteUrlMount::root();
    for (decl, route, expected) in [
        (None, None, None),
        (
            Some(OutputDeclaration::html_document()),
            None,
            Some("/target/"),
        ),
        (
            Some(OutputDeclaration::html_document()),
            Some("/moved/"),
            Some("/moved/"),
        ),
        (
            Some(OutputDeclaration::opaque(ResponseMediaType::HTML)),
            None,
            Some("/target/index.html"),
        ),
        (None, None, None),
    ] {
        let mut graph = OutputGraphBuilder::new();
        for out in orig.outputs() {
            match out.path().as_str() {
                "target/index.html" => {
                    if let Some(decl) = &decl {
                        graph
                            .insert(OutputFile::from_byte_owner(
                                out.path().clone(),
                                decl.clone(),
                                out.owner().clone(),
                                out.bytes_owner(),
                            ))
                            .unwrap();
                    }
                }
                _ => graph.insert(out.clone()).unwrap(),
            }
        }
        let graph = graph.finish();
        let site = reindex(&src, &graph, |page| {
            if page.output.as_str() == "target/index.html"
                && let Some(route) = route
            {
                page.permalink = UrlPath::parse(route).unwrap();
            }
        });
        let index = site_refs(&site, &mount, &world);
        let links = OutputPath::parse("links/index.html").unwrap();
        let alias = index.outgoing(&links).next().unwrap();
        match expected {
            Some(expected) => {
                let ReferenceResolution::Found { target } = alias.resolution() else {
                    panic!("alias did not resolve: {alias:?}");
                };
                assert_eq!(target.url().as_str(), expected);
                assert_eq!(target.query(), Some("view=card"));
            }
            None => assert_eq!(
                alias.resolution(),
                &ReferenceResolution::Unresolved {
                    reason: UnresolvedReferenceReason::TargetMissing { suggestion: None },
                },
            ),
        }
    }

    let mut graph = OutputGraphBuilder::new();
    for out in orig.outputs().iter().filter(|out| {
        matches!(
            out.path().as_str(),
            "stable.html" | "stable-target/index.html"
        )
    }) {
        graph.insert(out.clone()).unwrap();
    }
    let graph = graph.finish();
    let site = reindex(&src, &graph, |_| {});
    let without_links = site_refs(&site, &mount, &world);
    let links = OutputPath::parse("links/index.html").unwrap();
    assert_eq!(without_links.outgoing(&links).count(), 0);
    assert!(site_refs(&SiteIndex::default(), &mount, &world).is_empty());
}

#[test]
fn typed_uses_media_changes() {
    use crate::output::graph::OutputGraphBuilder;
    use crate::output::semantics::OutputDeclaration;

    let (_dir, src, orig, world) = candidate(
        r#"#document("typed.html", html.html(html.head(
  html.link(rel: "stylesheet", href: "/typed.bin")
)))
#document("plain.html")[#html.a(href: "/typed.bin")[Download]]"#,
    )
    .unwrap();
    let mount = SiteUrlMount::root();
    let bytes: Arc<[u8]> = Arc::from(b"same response bytes".as_slice());
    for media_type in [
        ResponseMediaType::CSS,
        ResponseMediaType::JAVASCRIPT,
        ResponseMediaType::parse("text/javascript; charset=utf-8").unwrap(),
        ResponseMediaType::CSS,
    ] {
        let mut graph = OutputGraphBuilder::new();
        graph.insert_graph(&orig).unwrap();
        graph
            .insert_configured_asset(
                "mapped-resource",
                OutputPath::parse("typed.bin").unwrap(),
                OutputDeclaration::opaque(media_type.clone()),
                Arc::clone(&bytes),
            )
            .unwrap();
        let graph = graph.finish();
        let out = graph
            .outputs()
            .iter()
            .find(|out| out.path().as_str() == "typed.bin")
            .unwrap();
        assert_eq!(out.bytes(), bytes.as_ref());
        let site = reindex(&src, &graph, |_| {});
        let index = site_refs(&site, &mount, &world);
        let typed = OutputPath::parse("typed.html").unwrap();
        let res = index.outgoing(&typed).next().unwrap().resolution();
        if media_type == ResponseMediaType::CSS {
            assert!(matches!(res, ReferenceResolution::Found { .. }));
        } else {
            assert!(matches!(
                res,
                ReferenceResolution::Unresolved {
                    reason: UnresolvedReferenceReason::MediaTypeMismatch { declared, .. }
                } if declared == &media_type
            ));
        }
    }
}

#[test]
fn cancellation_preempts_checks() {
    let (dir, site, _graph, world) =
        candidate(r#"#document("index.html")[#html.a(href: "/missing/")[Missing]]"#).unwrap();
    let cfg = crate::config::tests::load_test_config(dir.path(), "");
    let mount = SiteUrlMount::root();
    let cached = site_refs(&site, &mount, &world);
    let canceller = crate::cancellation::BuildCanceller::new();
    let cancel = canceller.token();
    canceller.cancel();

    for prev in [None, Some(&cached)] {
        assert!(matches!(
            References::from_site(&site, &mount, &world, &cancel, prev),
            Err(BuildCancelled)
        ));
    }
    assert!(matches!(
        diagnostic::diagnostics(&cached, &cfg, &cancel),
        Err(BuildCancelled)
    ));
}

#[test]
fn destination_final_dom() {
    let (_dir, site_index, _graph, world) = candidate(
        r#"#document("index.html")[
  #html.a(href: "/post/#intro")[Post]
  #html.a(href: "/100%25.html")[Percent-named page]
  #html.a(href: "/c%23notes.html")[Hash-named page]
]
#document("post/index.html")[#html.h1(id: "intro")[Intro]]
#document("100%.html")[Percent-named page]
#document("c#notes.html")[#html.h1(id: "intro")[Intro]]"#,
    )
    .unwrap();
    let refs = site_refs(&site_index, &SiteUrlMount::root(), &world);
    let page = OutputPath::parse("index.html").unwrap();

    assert_eq!(refs.references().len(), 3);
    assert_eq!(
        refs.outgoing(&page)
            .map(|r| {
                r.html_context().to_owned()
                    + " -> "
                    + match r.resolution() {
                        ReferenceResolution::Found { target } => target.url().as_str(),
                        res => panic!("did not resolve: {res:?}"),
                    }
            })
            .collect::<Vec<_>>(),
        [
            "a[href] -> /post/",
            "a[href] -> /100%.html",
            "a[href] -> /c#notes.html",
        ]
    );

    let (_dir, site_index, _graph, world) = candidate(
        r#"
#document("index.html")[
  #link(<pdf-part>)[PDF section]
  #html.a(href: "/clip.mp4#t=10")[Video]
]
#document("paper.pdf")[#rect(width: 1pt, height: 1pt) <pdf-part>]
#asset("clip.mp4", "video")
"#,
    )
    .unwrap();
    let refs = site_refs(&site_index, &SiteUrlMount::root(), &world);
    for (r, path, frag) in [
        (refs.references().next().unwrap(), "/paper.pdf", "pdf-part"),
        (refs.references().nth(1).unwrap(), "/clip.mp4", "t=10"),
    ] {
        let ReferenceResolution::Found { target } = r.resolution() else {
            panic!("expected resolved media target: {r:?}");
        };
        assert_eq!(target.url().as_str(), path);
        assert_eq!(target.fragment(), Some(frag));
    }
}

#[test]
fn backlinks_canonical_target() {
    let (_dir, site, _graph, world) = candidate(
        r#"#document("index.html")[
  #html.a(href: "/docs/post/index.html?view=card#part%2520x")[Alias]
  #html.a(href: "/docs/post/?view=list#part%2520x")[Clean]
  #html.a(href: "/docs/missing/")[Missing]
]
#document("post/index.html")[#html.h1(id: "part%20x")[Post]]"#,
    )
    .unwrap();
    let refs = site_refs(
        &site,
        &SiteUrlMount::from_base_path("/docs/").unwrap(),
        &world,
    );
    let url = UrlPath::parse("/post/").unwrap();
    let incoming: Vec<_> = refs.incoming(&url).collect();
    assert_eq!(incoming.len(), 2);
    for (r, query) in incoming.into_iter().zip(["view=card", "view=list"]) {
        let ReferenceResolution::Found { target } = r.resolution() else {
            panic!("expected resolved backlink");
        };
        assert_eq!(target.url(), &url);
        assert_eq!(target.query(), Some(query));
        assert_eq!(target.fragment(), Some("part%20x"));
        assert_eq!(r.page().as_str(), "index.html");
    }
    assert_eq!(
        refs.outgoing(&OutputPath::parse("index.html").unwrap())
            .count(),
        3
    );
    assert!(
        refs.incoming(&UrlPath::parse("/missing/").unwrap())
            .next()
            .is_none()
    );
}

#[test]
fn fragment_failures_target_requirements() {
    use crate::config::ReferenceLevel;
    use tola_diagnostic::Severity;
    use tola_diagnostic::codes::reference::{
        FRAGMENT_MISSING, NAVIGATION_MISSING, RESOURCE_MISSING,
    };

    for (src, cat, code, severity) in [
        (
            r##"#document("index.html")[#html.script(src: "#intro") #html.h1(id: "intro")[Intro]]"##,
            ReferenceCategory::Resource,
            RESOURCE_MISSING,
            Severity::Error,
        ),
        (
            r##"#document("index.html")[#html.script(src: "#missing")]"##,
            ReferenceCategory::Resource,
            RESOURCE_MISSING,
            Severity::Error,
        ),
        (
            r##"#document("index.html", html.html(
  html.head(html.base(href: "/missing/"))
  + html.body(html.a(href: "#intro")[Missing page])
))"##,
            ReferenceCategory::Navigation,
            NAVIGATION_MISSING,
            Severity::Error,
        ),
        (
            r#"#document("index.html")[#html.a(href: "/missing/#fragment")[Missing page]]"#,
            ReferenceCategory::Navigation,
            NAVIGATION_MISSING,
            Severity::Error,
        ),
        (
            r#"#document("index.html")[#html.a(href: "/post/#missing")[Missing fragment]]
#document("post/index.html")[#html.h1(id: "available")[Post]]"#,
            ReferenceCategory::Fragment,
            FRAGMENT_MISSING,
            Severity::Warning,
        ),
    ] {
        let (dir, site_index, _graph, world) = candidate(src).unwrap();
        let mut cfg = crate::config::tests::load_test_config(dir.path(), "");
        cfg.build.references.fragments = ReferenceLevel::Warn;
        let index = site_refs(&site_index, &SiteUrlMount::root(), &world);
        assert_eq!(index.references().next().unwrap().category(), cat);
        let diags = diagnostic::diagnostics(&index, &cfg, &BuildCancellation::new()).unwrap();
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, code);
        assert_eq!(diags[0].severity, severity);
    }
}

#[test]
fn missing_target_suggestion() {
    for (mount, href, suggestion) in [
        (SiteUrlMount::root(), "/guide", "/guide/"),
        (
            SiteUrlMount::from_base_path("/docs/").unwrap(),
            "/docs/guide",
            "/docs/guide/",
        ),
        (
            SiteUrlMount::from_base_path("/docs/").unwrap(),
            "/docs/guide?view=card#intro%2520part",
            "/docs/guide/?view=card#intro%2520part",
        ),
        (SiteUrlMount::root(), "/100%25-ready", "/100%25-ready/"),
        (SiteUrlMount::root(), "/a%2541", "/a%2541/"),
    ] {
        let (_dir, site_index, _graph, world) = candidate(&format!(
            r#"#document("index.html")[#html.a(href: "{href}")[Page]]
#document("guide/index.html")[#html.h1(id: "intro%20part")[Guide]]
#document("100%-ready/index.html")[Ready]
#document("a%41/index.html")[Named]"#
        ))
        .unwrap();
        let index = site_refs(&site_index, &mount, &world);
        let r = index.references().next().unwrap();
        let ReferenceResolution::Unresolved { reason } = r.resolution() else {
            panic!("{href} resolved: {r:?}");
        };
        let UnresolvedReferenceReason::TargetMissing {
            suggestion: offered,
        } = reason
        else {
            panic!("{href} lost its suggestion: {reason:?}");
        };
        assert_eq!(offered.as_deref(), Some(suggestion), "{href}");
    }
}

#[test]
fn missing_fragment_reported() {
    let (_dir, site_index, _graph, world) = candidate(
        r#"#document("index.html")[
  #html.a(href: "/post/#missing")[First]
  #html.a(href: "/post/#missing")[Second]
]
#document("post/index.html")[#html.h1(id: "z")[Last] #html.h2(id: "a")[First]]"#,
    )
    .unwrap();
    let index = site_refs(&site_index, &SiteUrlMount::root(), &world);
    assert_eq!(index.references().len(), 2);
    for r in index.references() {
        let ReferenceResolution::Unresolved { reason } = r.resolution() else {
            panic!("missing fragment resolved");
        };
        let UnresolvedReferenceReason::FragmentMissing {
            fragment,
            available,
        } = reason
        else {
            panic!("missing fragment lost its diagnostic");
        };
        assert_eq!(fragment, "missing");
        assert_eq!(available.as_ref(), ["a", "z"]);
    }
}

#[test]
fn base_href_target() {
    for (base, href, query, frag) in [
        ("/docs/", "target/", None, None),
        (
            "/docs/target/?view=base",
            "#intro",
            Some("view=base"),
            Some("intro"),
        ),
    ] {
        let (_dir, site_index, _graph, world) = candidate(&format!(
            r#"#document("guide/index.html", html.html(
  html.head(html.base(href: "{base}"))
  + html.body(html.a(href: "{href}")[Target])
))
#document("docs/target/index.html")[#html.h1(id: "intro")[Intro]]"#
        ))
        .unwrap();
        let index = site_refs(&site_index, &SiteUrlMount::root(), &world);
        let ReferenceResolution::Found { target } = index.references().next().unwrap().resolution()
        else {
            panic!("{href} did not resolve against {base}");
        };
        assert_eq!(target.url().as_str(), "/docs/target/");
        assert_eq!(target.query(), query);
        assert_eq!(target.fragment(), frag);
    }
}

#[test]
fn mount_rejects_outside_prefix() {
    let (_dir, site_index, _graph, world) = candidate(
        r#"#document("index.html")[
  #html.a(href: "/target/")[Outside mount]
  #html.a(href: "/docs/target/")[Mounted]
  #html.a(href: "target/")[Relative]
]
#document("target/index.html")[Target]"#,
    )
    .unwrap();

    let index = site_refs(
        &site_index,
        &SiteUrlMount::from_base_path("/docs/").unwrap(),
        &world,
    );

    assert!(matches!(
        index.references().next().unwrap().resolution(),
        ReferenceResolution::Unresolved {
            reason: UnresolvedReferenceReason::OutsideSiteMount { required_prefix, .. }
        } if required_prefix == "/docs/"
    ));
    assert!(matches!(
        index.references().nth(1).unwrap().resolution(),
        ReferenceResolution::Found { .. }
    ));
    assert!(matches!(
        index.references().nth(2).unwrap().resolution(),
        ReferenceResolution::Found { .. }
    ));
}

#[test]
fn img_rejects_documents() {
    let (_dir, site_index, _graph, world) = candidate(
        r#"#document("index.html")[
  #html.object(data: "/embedded/")[]
  #html.img(src: "/embedded/")
]
#document("embedded/index.html")[Embedded document]"#,
    )
    .unwrap();

    let index = site_refs(&site_index, &SiteUrlMount::root(), &world);

    assert_eq!(
        index.references().next().unwrap().category(),
        ReferenceCategory::Resource
    );
    assert!(matches!(
        index.references().next().unwrap().resolution(),
        ReferenceResolution::Found { .. }
    ));
    assert_eq!(
        index.references().nth(1).unwrap().category(),
        ReferenceCategory::Resource
    );
    assert!(matches!(
        index.references().nth(1).unwrap().resolution(),
        ReferenceResolution::Unresolved {
            reason: UnresolvedReferenceReason::DocumentNotAllowed { document, .. }
        } if document.as_str() == "/embedded/"
    ));
}

#[test]
fn prefetch_resource_only() {
    let (_dir, site_index, _graph, world) = candidate(
        r#"#document("index.html", html.html(html.head(
  html.elem("link", attrs: (rel: "PrEfEtCh prefetch", href: "/docs/next/"))
  + html.link(rel: "prefetch", href: "/docs/download.txt")
  + html.link(rel: "prefetch", href: "/docs/missing/")
)))
#document("next/index.html")[Next]
#asset("download.txt", "Download")"#,
    )
    .unwrap();
    let index = site_refs(
        &site_index,
        &SiteUrlMount::from_base_path("/docs/").unwrap(),
        &world,
    );
    let refs: Vec<_> = index.references().collect();
    assert_eq!(refs.len(), 3);
    for (r, expected) in refs[..2].iter().zip(["/next/", "/download.txt"]) {
        assert_eq!(r.category(), ReferenceCategory::Resource);
        let ReferenceResolution::Found { target } = r.resolution() else {
            panic!("prefetch did not resolve: {r:?}");
        };
        assert_eq!(target.url().as_str(), expected);
    }
    assert_eq!(refs[2].category(), ReferenceCategory::Resource);
    assert!(matches!(
        refs[2].resolution(),
        ReferenceResolution::Unresolved {
            reason: UnresolvedReferenceReason::TargetMissing { .. }
        }
    ));

    let (_dir, site_index, _graph, world) = candidate(
        r#"#document("index.html", html.html(html.head(
  html.link(rel: ("stylesheet", "prefetch"), href: "/next/")
  + html.link(rel: ("prefetch", "modulepreload"), href: "/next/")
  + html.link(rel: ("icon", "prefetch"), href: "/next/")
  + html.link(rel: ("preload", "prefetch"), href: "/next/")
  + html.script(src: "/next/")
  + html.script(type: "module", src: "/next/")
) + html.body(html.img(src: "/next/"))))
#document("next/index.html")[Next]"#,
    )
    .unwrap();
    let index = site_refs(&site_index, &SiteUrlMount::root(), &world);
    assert_eq!(index.references().len(), 7);
    for r in index.references() {
        assert_eq!(r.category(), ReferenceCategory::Resource);
        assert!(
            matches!(
                r.resolution(),
                ReferenceResolution::Unresolved {
                    reason: UnresolvedReferenceReason::DocumentNotAllowed { document, .. }
                } if document.as_str() == "/next/"
            ),
            "{r:?}"
        );
    }
}

#[test]
fn typed_uses_media_mismatch() {
    let (_dir, site_index, _graph, world) = candidate(
        r#"#document("index.html", html.html(
  html.head(
    html.link(rel: "stylesheet", href: "/opaque.bin")
    + html.link(rel: "stylesheet", href: "/module.js")
    + html.script(src: "/site.css")
  )
))
#asset("opaque.bin", "")
#asset("module.js", "")
#asset("site.css", "")"#,
    )
    .unwrap();

    let index = site_refs(&site_index, &SiteUrlMount::root(), &world);
    let failures: Vec<_> = index
        .references()
        .map(|r| match r.resolution() {
            ReferenceResolution::Unresolved {
                reason:
                    UnresolvedReferenceReason::MediaTypeMismatch {
                        reference_use,
                        required,
                        declared,
                    },
            } => (*reference_use, required.clone(), declared.clone()),
            res => panic!("expected media-type mismatch, got {res:?}"),
        })
        .collect();

    assert_eq!(
        failures,
        [
            (
                tola_typst::HtmlReferenceUse::Stylesheet,
                ResponseMediaType::CSS,
                ResponseMediaType::OCTET_STREAM,
            ),
            (
                tola_typst::HtmlReferenceUse::Stylesheet,
                ResponseMediaType::CSS,
                ResponseMediaType::JAVASCRIPT,
            ),
            (
                tola_typst::HtmlReferenceUse::ClassicScript,
                ResponseMediaType::JAVASCRIPT,
                ResponseMediaType::CSS,
            ),
        ]
    );
}

#[test]
fn external_uses_no_declaration() {
    let (_dir, site_index, _graph, world) = candidate(
        r#"#document("index.html", html.html(
  html.head(
    html.link(rel: "stylesheet", href: "https://cdn.example/site")
    + html.script(type: "module", src: "//cdn.example/module")
  )
))"#,
    )
    .unwrap();

    let index = site_refs(&site_index, &SiteUrlMount::root(), &world);

    assert_eq!(index.references().len(), 2);
    assert!(index.references().all(|r| {
        r.category() == ReferenceCategory::External
            && r.resolution() == &ReferenceResolution::External
    }));
}
