use super::*;
use crate::build::tests::*;
use crate::build::*;

use crate::config::section::{AssetFileDeclaration, AssetUrl};
use std::fs;
use tempfile::TempDir;

fn published_identity(graph: &crate::output::graph::OutputGraph, path: &str) -> String {
    tola_typst::ContentDigest::of(output_bytes(graph, path)).to_hex()
}

#[test]
fn asset_url_tracks_bytes() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("content")).unwrap();
    let script = root.join("app.js");
    fs::write(&script, "export const version = 1;\n").unwrap();
    let css = root.join("site.css");
    fs::write(&css, "body { color: red; }\n").unwrap();
    let entry = root.join("site.typ");
    fs::write(
        &entry,
        "#import \"@tola/address:0.0.0\": asset-url\n#document(\"index.html\")[#asset-url(\"/app.js\")]",
    )
    .unwrap();
    let mut cfg = site_config(root);
    cfg.assets.cache_busting = true;
    cfg.assets.files = vec![AssetFileDeclaration::new(
        &script,
        AssetUrl::parse("/app.js").unwrap(),
    )];

    let mut session = BuildSession::new();
    let host = compiler_host(&cfg).unwrap();
    let first = build_site_with_host(
        &cfg,
        &host,
        BuildMode::Production,
        &mut BuildAttemptProducers::default(),
    )
    .unwrap();
    let first_id = published_identity(&first.graph, "app.js");
    let first_html = String::from_utf8_lossy(output_bytes(&first.graph, "index.html")).into_owned();
    assert!(
        first_html.contains(&format!("/app.js?h={first_id}")),
        "{first_html}"
    );
    install_build(&mut session, first);

    let mut grown = cfg.clone();
    grown.assets.files.push(AssetFileDeclaration::new(
        &css,
        AssetUrl::parse("/site.css").unwrap(),
    ));
    let decision = session.rebuild_decision(std::slice::from_ref(&css), false);
    let reuse = session.reusable_compilation(&decision, true).unwrap();
    let second = build_site_with_host(
        &grown,
        &compiler_host(&grown).unwrap(),
        BuildMode::Production,
        &mut BuildAttemptProducers {
            reuse: Some(reuse),
            previous_bundle_entries: session.bundle_entries(),
            asset_reuse: Some((
                session.configured_assets().unwrap().clone(),
                [css.clone()].into(),
            )),
            ..BuildAttemptProducers::default()
        },
    )
    .unwrap();
    assert_eq!(
        output_bytes(&second.graph, "index.html"),
        first_html.as_bytes()
    );
    install_build(&mut session, second);

    fs::write(&script, "export const version = 2;\n").unwrap();
    let decision = session.rebuild_decision(std::slice::from_ref(&script), false);
    let reuse = session.reusable_compilation(&decision, true).unwrap();
    let third = build_site_with_host(
        &grown,
        &compiler_host(&grown).unwrap(),
        BuildMode::Production,
        &mut BuildAttemptProducers {
            reuse: Some(reuse),
            previous_bundle_entries: session.bundle_entries(),
            asset_reuse: Some((
                session.configured_assets().unwrap().clone(),
                [script.clone()].into(),
            )),
            ..BuildAttemptProducers::default()
        },
    )
    .unwrap();
    let third_id = published_identity(&third.graph, "app.js");
    let third_html = String::from_utf8_lossy(output_bytes(&third.graph, "index.html")).into_owned();

    assert_ne!(third_id, first_id, "changed bytes, changed identity");
    assert!(
        third_html.contains(&format!("/app.js?h={third_id}")),
        "{third_html}"
    );
    assert!(
        !third_html.contains(&format!("/app.js?h={first_id}")),
        "{third_html}"
    );
    assert!(has_output(&third.graph, "app.js"));
    assert!(has_output(&third.graph, "site.css"));
}

#[test]
fn failed_build_records_reads() {
    struct Case {
        name: &'static str,
        files: &'static [(&'static str, &'static str)],
        recorded: &'static [&'static str],
        failure: &'static str,
    }
    let cases = [
        Case {
            name: "program evaluation",
            files: &[("site.typ", "#include \"templates/missing.typ\"")],
            recorded: &["site.typ", "templates/missing.typ"],
            failure: "missing.typ",
        },
        Case {
            name: "source analysis",
            files: &[
                ("site.typ", "#document(\"index.html\")[Site]"),
                ("content/index.typ", "#include \"../templates/missing.typ\""),
            ],
            recorded: &["templates/missing.typ"],
            failure: "missing.typ",
        },
        Case {
            name: "realization",
            files: &[
                ("templates/shared.typ", "#let title = [Shared]"),
                (
                    "site.typ",
                    "#import \"templates/shared.typ\": title\n#document(\"index.html\")[#title #context read(\"templates/missing.txt\")]",
                ),
            ],
            recorded: &["site.typ", "templates/shared.typ", "templates/missing.txt"],
            failure: "missing.txt",
        },
    ];

    for case in cases {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        for (rel, body) in case.files {
            let path = root.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, body).unwrap();
        }
        let cfg = site_config(root);
        let mut producers = BuildAttemptProducers::default();
        let err = match build_site_with_host(
            &cfg,
            &compiler_host(&cfg).unwrap(),
            BuildMode::Production,
            &mut producers,
        ) {
            Ok(_) => panic!("{} unexpectedly built", case.name),
            Err(err) => err,
        };

        let reads = producers.into_inputs().compiler.into_parts().0;
        for rel in case.recorded {
            let path = root.join(rel);
            let recorded = if path.exists() {
                tola_input::normalize_path(&path)
            } else {
                tola_input::normalize_existing_prefix(&path)
            };
            assert!(
                reads.contains(&recorded),
                "{}: {rel} not recorded",
                case.name
            );
        }
        assert!(
            format!("{err:#}").contains(case.failure),
            "{}: {err:#}",
            case.name
        );
    }
}

#[test]
fn cancelled_build_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let content = root.join("content");
    fs::create_dir_all(&content).unwrap();
    fs::write(content.join("post.typ"), "Post").unwrap();
    fs::write(root.join("site.typ"), "#document(\"index.html\")[Hello]").unwrap();

    let cfg = site_config(root);
    let canceller = crate::cancellation::BuildCanceller::new();
    let cancel = canceller.token();
    canceller.cancel();
    let mut producers = BuildAttemptProducers {
        cancellation: cancel,
        ..BuildAttemptProducers::default()
    };

    let err = match build_site_with_host(
        &cfg,
        &compiler_host(&cfg).unwrap(),
        BuildMode::Production,
        &mut producers,
    ) {
        Ok(_) => panic!("cancelled build unexpectedly produced a candidate"),
        Err(err) => err,
    };

    assert!(crate::cancellation::is_cancelled(&err));
}

#[test]
fn slugify_uses_site_language() {
    let program = r#"#import "@tola/address:0.0.0": join-route, output-of, slugify
#import "@tola/site:0.0.0": site
#import "@tola/source:0.0.0": all-sources

#for source in all-sources() {
  document(output-of(join-route(source.route-segments.map(segment => slugify(segment, mode: "ascii", language: site.language.lang)))))[#source.id]
}"#;
    let cases = [
        (
            "",
            &[
                ("重庆.typ", "= 重庆"),
                ("しんぶん.typ", "= しんぶん"),
                ("한국어.typ", "= 한국어"),
            ][..],
            &[
                "chong-qing/index.html",
                "shinbun/index.html",
                "hangugeo/index.html",
            ][..],
        ),
        (
            "[site]\nlanguage = \"ja\"",
            &[("東京.typ", "= 東京")][..],
            &["toukyou/index.html"][..],
        ),
    ];
    for (src, files, expected) in cases {
        let dir = TempDir::new().unwrap();
        let cfg = site_config_from(dir.path(), src);
        fs::write(&cfg.build.entry, program).unwrap();
        for (name, body) in files {
            fs::write(cfg.build.content_dir.join(name), body).unwrap();
        }

        let built = build_site(&cfg, BuildMode::Production).unwrap();
        let published = built
            .graph
            .outputs()
            .iter()
            .map(|out| out.path().as_str())
            .collect::<Vec<_>>();
        for route in expected {
            assert!(published.contains(route), "{route} missing: {published:?}");
        }
    }
}

#[test]
fn generated_routes_publish() {
    {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("content")).unwrap();
        fs::create_dir_all(root.join("routes")).unwrap();
        fs::write(
            root.join("site.typ"),
            "#import \"routes/routes.typ\": emit-routes\n#emit-routes()\n",
        )
        .unwrap();
        fs::write(
            root.join("routes/routes.typ"),
            "#let emit-routes() = include \"generated.typ\"",
        )
        .unwrap();
        fs::write(
            root.join("routes/generated.typ"),
            "#for slug in (\"alpha\", \"beta\") {\n  document(slug + \"/index.html\", [Route #slug])\n}\n#asset(\"generated/routes.txt\", \"alpha,beta\")\n",
        )
        .unwrap();
        let cfg = site_config(root);
        let out = cfg.build.publish_dir.clone();
        let built = build_and_publish(&cfg).unwrap();
        for path in ["alpha/index.html", "beta/index.html"] {
            assert!(has_output(&built.graph, path), "missing {path}");
            assert!(out.join(path).is_file(), "unpublished {path}");
        }
        assert_eq!(
            output_bytes(&built.graph, "generated/routes.txt"),
            b"alpha,beta"
        );
    }

    {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("content")).unwrap();
        fs::create_dir_all(root.join("routes")).unwrap();
        fs::write(
            root.join("content/alpha.typ"),
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: [Alpha], tags: (\"rust\", \"typst\")))",
        )
        .unwrap();
        fs::write(
            root.join("content/beta.typ"),
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: [Beta], tags: (\"rust\",)))",
        )
        .unwrap();
        fs::write(
            root.join("routes/taxonomy.typ"),
            "#let emit-taxonomies(sources) = {\n  for tag in (\"rust\", \"typst\") {\n    let members = sources.filter(source => source.meta.tags.contains(tag))\n    if members.len() > 0 {\n      let names = members.map(source => source.meta.title).join(\", \")\n      document(\"tags/\" + tag + \"/index.html\", [#names])\n    }\n  }\n}\n",
        )
        .unwrap();
        fs::write(
            root.join("site.typ"),
            "#import \"@tola/source:0.0.0\": all-sources\n#import \"routes/taxonomy.typ\": emit-taxonomies\n#emit-taxonomies(all-sources())\n",
        )
        .unwrap();
        let cfg = site_config(root);
        let out = cfg.build.publish_dir.clone();
        let built = build_and_publish(&cfg).unwrap();
        let rust = fs::read_to_string(out.join("tags/rust/index.html")).unwrap();
        assert!(rust.contains("Alpha") && rust.contains("Beta"), "{rust}");
        let typst = fs::read_to_string(out.join("tags/typst/index.html")).unwrap();
        assert!(
            typst.contains("Alpha") && !typst.contains("Beta"),
            "{typst}"
        );
        assert_eq!(built.index.address().pages().len(), 2);
    }
}

#[test]
fn page_properties_from_document_meta() {
    struct Case {
        program: &'static str,
        url: &'static str,
        title: Option<&'static str>,
        description: Option<&'static str>,
        author: &'static [&'static str],
    }
    for case in [
        Case {
            program: r#"#document("plain/index.html")[Plain]"#,
            url: "/plain/",
            title: None,
            description: None,
            author: &[],
        },
        Case {
            program: r#"#document("native/index.html", title: [Native Title], author: "Alice", description: [Native Summary])[Native]"#,
            url: "/native/",
            title: Some("Native Title"),
            description: Some("Native Summary"),
            author: &["Alice"],
        },
        Case {
            program: "#document(\"tags/rust/index.html\", title: [Native Rust])[\n  #import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: [Rust], pin: true))\n  = Rust\n]\n",
            url: "/tags/rust/",
            title: Some("Native Rust"),
            description: None,
            author: &[],
        },
    ] {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("content")).unwrap();
        fs::write(root.join("site.typ"), case.program).unwrap();
        let cfg = site_config(root);

        let built = build_and_publish(&cfg).unwrap();
        let url = tola_address::UrlPath::parse(case.url).unwrap();
        let crate::site::Resource::Page { document: doc } =
            built.index.address().get_by_url(&url).unwrap()
        else {
            panic!("expected generated document at {}", case.url);
        };

        assert_eq!(
            doc.properties.title.as_ref().map(|t| t.to_string()),
            case.title.map(str::to_owned),
            "{}",
            case.url
        );
        assert_eq!(
            doc.properties.description.as_ref().map(|d| d.to_string()),
            case.description.map(str::to_owned),
            "{}",
            case.url
        );
        assert_eq!(
            doc.properties.author.as_slice(),
            case.author,
            "{}",
            case.url
        );
    }
}
