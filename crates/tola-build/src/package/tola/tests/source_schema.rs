use std::collections::BTreeMap;

use super::*;

fn source_world(program: &str, srcs: &[(&str, &str)]) -> (TempDir, tola_typst::TypstWorld) {
    source_world_with_files(program, srcs, &[])
}

fn source_world_with_files(
    program: &str,
    srcs: &[(&str, &str)],
    files: &[(&str, &str)],
) -> (TempDir, tola_typst::TypstWorld) {
    let dir = TempDir::new().unwrap();
    let cfg = crate::config::tests::load_test_config(dir.path(), "");
    let root = cfg.root();
    for (path, body) in files {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }
    let content = root.join("content");
    let files = Arc::new(tola_typst::FileStore::new().with_provider(IconFiles::default()));
    let mut units = Vec::new();
    let mut decls = BTreeMap::new();
    for (id, body) in srcs {
        let path = content.join(id);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, body).unwrap();
        decls.insert(path.clone(), super::declared_metadata(&path, root));
        units.push(ContentUnit {
            root: content.clone(),
            id: ContentId::new((*id).into()),
            source: path,
            layout: ContentSourceLayout::from_entry_path(Path::new(id)),
        });
    }
    let records = SourceSet::without_metadata(&units, &cfg)
        .unwrap()
        .with_metadata(&decls)
        .inputs()
        .to_source_records();
    let lib = SiteBindings::from_config(&cfg, Default::default(), None).library(records);
    let main = root.join("site.typ");
    fs::write(&main, program).unwrap();
    let world = TypstWorld::builder(&main, root)
        .with_files(files)
        .with_local_cache()
        .no_fonts()
        .with_shared_library(lib.shared())
        .build(&tola_typst::BundleCancellation::default())
        .expect("valid source world");
    (dir, world)
}

fn source_errors(program: &str, srcs: &[(&str, &str)]) -> Vec<tola_typst::diagnostic::Diagnostic> {
    let (_dir, world) = source_world(program, srcs);
    tola_typst::compile_bundle_world(&world, &Default::default())
        .expect_err("invalid source metadata must fail compilation")
        .diagnostics()
        .expect("Typst failure has diagnostics")
        .errors()
        .cloned()
        .collect()
}

#[test]
fn shared_declarations_propagate() {
    let (_dir, world) = source_world_with_files(
        r#"
#import "@tola/source:0.0.0": all-sources, parse-sources
#import "@tola/schema:0.0.0": schema
#let sources = all-sources()
#assert.eq(sources.first().meta, (title: "Shared"))
#assert.eq(parse-sources(sources, schema((title: str))).first().meta.title, "Shared")
#document("index.html")[]
"#,
        &[(
            "note.typ",
            "#import \"../theme.typ\": note\n#note(title: \"Shared\")",
        )],
        &[(
            "theme.typ",
            "#import \"@tola/source:0.0.0\": tola-meta\n#let note(..fields) = tola-meta(fields.named())",
        )],
    );
    tola_typst::compile_bundle_world(&world, &Default::default()).unwrap();
}

#[test]
fn shared_declaration_errors_owner() {
    let helper = "#import \"@tola/source:0.0.0\": tola-meta\n#let note(..fields) = tola-meta(fields.named())";
    let (_dir, world) = source_world_with_files(
        r#"
#import "@tola/source:0.0.0": all-sources, parse-sources
#import "@tola/schema:0.0.0": schema
#parse-sources(all-sources(), schema((title: str)))
#document("index.html")[]
"#,
        &[(
            "note.typ",
            "#import \"../theme.typ\": note\n#note(title: 1)",
        )],
        &[("theme.typ", helper)],
    );
    let failure = tola_typst::compile_bundle_world(&world, &Default::default()).unwrap_err();
    let errs: Vec<_> = failure.diagnostics().unwrap().errors().collect();
    assert_eq!(errs.len(), 1);
    let err = errs[0];
    assert!(err.message.contains("content/note.typ"), "{err:?}");
    assert_eq!(err.location.path.as_deref(), Some("theme.typ"));
    let range = err.location.range.as_ref().unwrap();
    assert_eq!(range.start.line, range.end.line);
    let line = helper.lines().nth(range.start.line).unwrap();
    assert_eq!(
        &line[range.start.character..range.end.character],
        "tola-meta"
    );
}

#[test]
fn parsed_metadata_immutable() {
    let (_dir, world) = source_world(
        r#"
#import "@tola/source:0.0.0": all-sources, parse-sources
#import "@tola/schema:0.0.0": schema, map, trim
#let original = all-sources()
#let selected = (original.at(1), original.at(0))
#let declaration = schema((title: map(trim(str), title => title + "!")), unknown: "keep")
#let parsed = parse-sources(selected, declaration)
#assert.eq(parsed.map(source => source.id), ("posts/note.typ", "about.typ"))
#assert.eq(parsed.map(source => source.meta), ((title: "Note!", draft: false), (title: "About!", draft: true)))
#for (before, after) in selected.zip(parsed) {
  for key in ("id", "file", "path", "filename", "route-segments") {
    assert.eq(after.at(key), before.at(key))
  }
}
#assert.eq(all-sources(), original)
#assert.eq(original.map(source => source.meta.title), (" About ", " Note "))
#document("index.html")[]
"#,
        &[
            (
                "about.typ",
                "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \" About \", draft: true))",
            ),
            (
                "posts/note.typ",
                "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \" Note \", draft: false))",
            ),
        ],
    );
    tola_typst::compile_bundle_world(&world, &Default::default()).unwrap();
}

#[test]
fn source_schemas_order() {
    let (_dir, world) = source_world(
        r#"
#import "@tola/source:0.0.0": all-sources, parse-sources
#import "@tola/schema:0.0.0": schema, map, trim
#let original = all-sources()
#let selected = (original.at(1), original.at(0))
#let page-schema = schema((title: trim(str)))
#let post-schema = schema((title: map(trim(str), title => title + "!"), rank: int))
#let parsed = parse-sources(selected, source => {
  if source.route-segments.first() == "posts" { post-schema } else { page-schema }
})
#assert.eq(parsed.map(source => source.id), ("posts/note.typ", "about.typ"))
#assert.eq(parsed.map(source => source.meta), ((title: "Note!", rank: 3), (title: "About")))
#for (before, after) in selected.zip(parsed) {
  assert.eq(after + (meta: before.meta), before)
}
#assert.eq(all-sources(), original)
#document("index.html")[]
"#,
        &[
            (
                "about.typ",
                "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \" About \"))",
            ),
            (
                "posts/note.typ",
                "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \" Note \", rank: 3))",
            ),
        ],
    );
    tola_typst::compile_bundle_world(&world, &Default::default()).unwrap();
}

#[test]
fn mixed_issues_source_locations() {
    let errs = source_errors(
        r#"
#import "@tola/source:0.0.0": all-sources, parse-sources
#import "@tola/schema:0.0.0": schema
#let parsed = parse-sources(all-sources(), source => {
  if source.route-segments.first() == "posts" {
    schema((title: str, rank: int))
  } else {
    schema((title: str))
  }
})
#document("index.html")[]
"#,
        &[
            (
                "about.typ",
                "\n#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: 1))",
            ),
            (
                "posts/note.typ",
                "\n\n#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: false, rank: \"bad\"))",
            ),
        ],
    );
    assert_eq!(
        errs.iter()
            .map(|e| (e.location.path.as_deref(), e.location.line))
            .collect::<Vec<_>>(),
        [
            (Some("content/about.typ"), Some(3)),
            (Some("content/posts/note.typ"), Some(4)),
            (Some("content/posts/note.typ"), Some(4)),
        ]
    );
    assert!(errs[0].message.contains("$.title"));
    assert!(errs[1].message.contains("$.title"));
    assert!(errs[2].message.contains("$.rank"));
}

#[test]
fn converted_field_error_span() {
    let meta = "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((raw: \"\", slug: \"already-valid\"))";
    let errs = source_errors(
        r#"
#import "@tola/source:0.0.0": all-sources, parse-sources
#import "@tola/schema:0.0.0": check, issue, map, schema
#let converted = map(
  schema((raw: str, slug: str)),
  input => (slug: input.raw),
  output: schema((slug: str)),
)
#let checked = check(converted, value => {
  if value.slug == "" { (issue("must not be empty", path: ("slug",)),) }
  else { () }
})
#let parsed = parse-sources(all-sources(), checked)
#document("index.html")[]
"#,
        &[("note.typ", meta)],
    );
    assert_eq!(errs.len(), 1);
    let err = &errs[0];
    assert_eq!(err.location.path.as_deref(), Some("content/note.typ"));
    assert!(err.message.contains("$.slug"), "{err:?}");
    assert!(err.message.contains("must not be empty"), "{err:?}");
    let range = err.location.range.as_ref().unwrap();
    assert_eq!(range.start.line, range.end.line);
    let line = meta.lines().nth(range.start.line).unwrap();
    assert_eq!(
        &line[range.start.character..range.end.character],
        "tola-meta"
    );
}

#[test]
fn schema_failures_type_names() {
    for (decl, expected, actual) in [
        ("none", "parse-sources schema", "none"),
        ("(title: str)", "parse-sources schema", "dictionary"),
        ("source => none", "schema function", "none"),
        ("source => () => none", "schema function", "function"),
    ] {
        let errs = source_errors(
            &format!(
                r#"#import "@tola/source:0.0.0": all-sources, parse-sources
#let parsed = parse-sources(all-sources(), {decl})
#document("index.html")[]"#
            ),
            &[("note.typ", "")],
        );
        assert_eq!(errs.len(), 1, "{errs:?}");
        let err = &errs[0];
        assert_eq!(err.location.path.as_deref(), Some("site.typ"));
        assert_eq!(err.location.line, Some(2));
        assert!(err.message.contains(expected), "{err:?}");
        assert!(err.message.contains("content/note.typ"), "{err:?}");
        assert!(err.message.contains(&format!("`{actual}`")), "{err:?}");
        assert!(
            err.message
                .contains("expected a Typst type or schema declaration"),
            "{err:?}"
        );
        if actual == "dictionary" {
            assert!(
                err.hints.iter().any(|h| h.message.contains("schema(...)")),
                "{err:?}"
            );
        }
    }
}

#[test]
fn callback_failures_locations() {
    for decl in ["source => fail()", "source => { try-parse(none, none) }"] {
        let errs = source_errors(
            &format!(
                r#"#import "@tola/source:0.0.0": all-sources, parse-sources
#import "@tola/schema:0.0.0": try-parse
#let fail() = panic("schema unavailable")
#let parsed = parse-sources(all-sources(), {decl})
#document("index.html")[]"#
            ),
            &[("note.typ", "")],
        );
        let err = &errs[0];
        assert_eq!(err.location.path.as_deref(), Some("site.typ"));
        assert!(!err.message.contains("function for"), "{err:?}");
        assert!(
            err.hints
                .iter()
                .any(|h| h.message.contains("content/note.typ")),
            "{err:?}"
        );
        if decl.contains("fail()") {
            assert_eq!(err.location.line, Some(3));
            assert!(err.message.contains("schema unavailable"));
            assert!(
                err.traces
                    .iter()
                    .any(|t| t.location.path.as_deref() == Some("site.typ")),
                "{err:?}"
            );
        }
    }
}

#[test]
fn absent_metadata_defaults() {
    let (_dir, world) = source_world(
        r#"
#import "@tola/source:0.0.0": all-sources, parse-sources
#import "@tola/schema:0.0.0": schema, optional, nullable, try-parse
#let declaration = schema((title: optional(nullable(str), default: "Untitled")))
#assert.eq(parse-sources(all-sources(), declaration).map(source => source.meta), (
  (title: "Untitled"), (title: none),
))
#assert.eq(try-parse(none, declaration).ok, false)
#assert.eq(all-sources().map(source => source.meta), (none, (title: none)))
#document("index.html")[]
"#,
        &[
            ("absent.typ", ""),
            (
                "explicit.typ",
                "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: none))",
            ),
        ],
    );
    tola_typst::compile_bundle_world(&world, &Default::default()).unwrap();
}

#[test]
fn batch_issues_per_source() {
    let errs = source_errors(
        r#"
#import "@tola/source:0.0.0": all-sources, parse-sources
#import "@tola/schema:0.0.0": schema
#let parsed = parse-sources(all-sources(), schema((title: str, rank: int)))
#document("index.html")[]
"#,
        &[
            (
                "about.typ",
                "\n\n#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: 1, rank: false))",
            ),
            (
                "posts/note.typ",
                "\n\n\n#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: false, rank: \"bad\"))",
            ),
            ("absent.typ", ""),
        ],
    );
    let locs: Vec<_> = errs
        .iter()
        .map(|e| (e.location.path.as_deref(), e.location.line))
        .collect();
    assert_eq!(
        locs,
        [
            (Some("content/about.typ"), Some(4)),
            (Some("content/about.typ"), Some(4)),
            (Some("content/posts/note.typ"), Some(5)),
            (Some("content/posts/note.typ"), Some(5)),
            (Some("content/absent.typ"), Some(1)),
            (Some("content/absent.typ"), Some(1)),
        ]
    );
    for err in &errs[4..] {
        assert_eq!(err.location.column, Some(1));
    }
}

#[test]
fn metadata_failure_code() {
    let (dir, world) = source_world(
        r#"
#import "@tola/source:0.0.0": all-sources, parse-sources
#import "@tola/schema:0.0.0": check, issue, nullable, optional, schema, timestamp
#let published = optional(
  check(nullable(timestamp), value => {
    if value == none {
      (issue("Published blog pages need a publication date; use draft: true while writing pages."),)
    } else {
      ()
    }
  }),
  default: none,
)
#let parsed = parse-sources(all-sources(), schema((published: published)))
#document("index.html")[]
"#,
        &[(
            "note.typ",
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((:))",
        )],
    );
    let err: anyhow::Error = tola_typst::compile_bundle_world(&world, &Default::default())
        .expect_err("the schema default must fail")
        .into();
    let diags = crate::compiler::diagnostic::error_diagnostics(&err, dir.path())
        .expect("a compile failure has diagnostics");
    assert_eq!(diags[0].code, tola_diagnostic::codes::source::METADATA);
    assert!(
        diags[0].message.starts_with("`content/note.typ`: "),
        "{}",
        diags[0].message
    );
    assert!(!diags[0].trace.is_empty());
}
