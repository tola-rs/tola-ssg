use super::*;
use std::collections::BTreeMap;

fn source_world(program: &str, sources: &[(&str, &str)]) -> (TempDir, tola_typst::TypstWorld) {
    source_world_with_files(program, sources, &[])
}

fn source_world_with_files(
    program: &str,
    sources: &[(&str, &str)],
    files: &[(&str, &str)],
) -> (TempDir, tola_typst::TypstWorld) {
    let directory = TempDir::new().unwrap();
    let config = crate::config::tests::load_test_config(directory.path(), "");
    let root = config.get_root();
    for (path, body) in files {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }
    let content = root.join("content");
    let files = Arc::new(tola_typst::FileResolver::new().with_provider(IconFiles::default()));
    let mut units = Vec::new();
    let mut declarations = BTreeMap::new();
    for (id, body) in sources {
        let path = content.join(id);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, body).unwrap();
        declarations.insert(path.clone(), super::declared_metadata(&path, root));
        units.push(ContentUnit {
            root: content.clone(),
            id: ContentId::new((*id).into()),
            source: path,
            layout: ContentSourceLayout::from_entry_path(Path::new(id)),
        });
    }
    let records = SourceSet::without_metadata(&units, &config)
        .unwrap()
        .with_metadata(&declarations)
        .inputs()
        .to_source_records();
    let bindings = SiteBindings::from_config(&config, Default::default());
    let library = bindings.library(records);
    let main = root.join("site.typ");
    fs::write(&main, program).unwrap();
    let world = TypstWorld::builder(&main, root)
        .with_files(files)
        .with_local_cache()
        .no_fonts()
        .with_shared_library(library.shared())
        .build(&tola_typst::BundleCancellation::default())
        .expect("valid source world");
    (directory, world)
}

fn source_errors(
    program: &str,
    sources: &[(&str, &str)],
) -> Vec<tola_typst::diagnostic::Diagnostic> {
    let (_directory, world) = source_world(program, sources);
    tola_typst::compile_bundle_world(&world, &Default::default())
        .expect_err("invalid source metadata must fail compilation")
        .diagnostics()
        .expect("Typst failure has diagnostics")
        .errors()
        .cloned()
        .collect()
}

#[test]
fn shared_declarations_reach_sources() {
    let (_directory, world) = source_world_with_files(
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
fn shared_declaration_issues_name_owner() {
    let helper = "#import \"@tola/source:0.0.0\": tola-meta\n#let note(..fields) = tola-meta(fields.named())";
    let (_directory, world) = source_world_with_files(
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
    let errors = failure.diagnostics().unwrap().errors().collect::<Vec<_>>();
    assert_eq!(errors.len(), 1);
    let error = errors[0];
    assert!(error.message.contains("content/note.typ"), "{error:?}");
    assert_eq!(error.location.path.as_deref(), Some("theme.typ"));
    let range = error.location.range.as_ref().unwrap();
    assert_eq!(range.start.line, range.end.line);
    let line = helper.lines().nth(range.start.line).unwrap();
    assert_eq!(
        &line[range.start.character..range.end.character],
        "tola-meta"
    );
}

#[test]
fn parsed_metadata_leaves_sources_unchanged() {
    let (_directory, world) = source_world(
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
fn source_schemas_preserve_selected_order() {
    let (_directory, world) = source_world(
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
fn mixed_issues_keep_source_locations() {
    let errors = source_errors(
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
        errors
            .iter()
            .map(|error| (error.location.path.as_deref(), error.location.line))
            .collect::<Vec<_>>(),
        [
            (Some("content/about.typ"), Some(3)),
            (Some("content/posts/note.typ"), Some(4)),
            (Some("content/posts/note.typ"), Some(4)),
        ]
    );
    assert!(errors[0].message.contains("$.title"));
    assert!(errors[1].message.contains("$.title"));
    assert!(errors[2].message.contains("$.rank"));
}

#[test]
fn converted_field_failure_does_not_point_to_an_unrelated_input_field() {
    let metadata = "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((raw: \"\", slug: \"already-valid\"))";
    let errors = source_errors(
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
        &[("note.typ", metadata)],
    );
    assert_eq!(errors.len(), 1);
    let error = &errors[0];
    assert_eq!(error.location.path.as_deref(), Some("content/note.typ"));
    assert!(error.message.contains("$.slug"), "{error:?}");
    assert!(error.message.contains("must not be empty"), "{error:?}");
    let range = error.location.range.as_ref().unwrap();
    assert_eq!(range.start.line, range.end.line);
    let line = metadata.lines().nth(range.start.line).unwrap();
    assert_eq!(
        &line[range.start.character..range.end.character],
        "tola-meta"
    );
}

#[test]
fn schema_failures_name_source_types() {
    for (declaration, expected, actual) in [
        ("none", "parse-sources schema", "none"),
        ("(title: str)", "parse-sources schema", "dictionary"),
        ("source => none", "schema function", "none"),
        ("source => () => none", "schema function", "function"),
    ] {
        let errors = source_errors(
            &format!(
                r#"#import "@tola/source:0.0.0": all-sources, parse-sources
#let parsed = parse-sources(all-sources(), {declaration})
#document("index.html")[]"#
            ),
            &[("note.typ", "")],
        );
        assert_eq!(errors.len(), 1, "{errors:?}");
        let error = &errors[0];
        assert_eq!(error.location.path.as_deref(), Some("site.typ"));
        assert_eq!(error.location.line, Some(2));
        assert!(error.message.contains(expected), "{error:?}");
        assert!(error.message.contains("content/note.typ"), "{error:?}");
        assert!(error.message.contains(&format!("`{actual}`")), "{error:?}");
        assert!(
            error
                .message
                .contains("expected a Typst type or schema declaration"),
            "{error:?}"
        );
        if actual == "dictionary" {
            assert!(
                error
                    .hints
                    .iter()
                    .any(|hint| hint.message.contains("schema(...)")),
                "{error:?}"
            );
        }
    }
}

#[test]
fn callback_failures_keep_authored_locations() {
    for declaration in ["source => fail()", "source => { try-parse(none, none) }"] {
        let errors = source_errors(
            &format!(
                r#"#import "@tola/source:0.0.0": all-sources, parse-sources
#import "@tola/schema:0.0.0": try-parse
#let fail() = panic("schema unavailable")
#let parsed = parse-sources(all-sources(), {declaration})
#document("index.html")[]"#
            ),
            &[("note.typ", "")],
        );
        let error = &errors[0];
        assert_eq!(error.location.path.as_deref(), Some("site.typ"));
        assert!(!error.message.contains("function for"), "{error:?}");
        assert!(
            error
                .hints
                .iter()
                .any(|hint| hint.message.contains("content/note.typ")),
            "{error:?}"
        );
        if declaration.contains("fail()") {
            assert_eq!(error.location.line, Some(3));
            assert!(error.message.contains("schema unavailable"));
            assert!(
                error
                    .traces
                    .iter()
                    .any(|trace| trace.location.path.as_deref() == Some("site.typ")),
                "{error:?}"
            );
        }
    }
}

#[test]
fn absent_metadata_uses_source_defaults() {
    let (_directory, world) = source_world(
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
fn batch_issues_split_by_source() {
    let errors = source_errors(
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
    let locations = errors
        .iter()
        .map(|error| (error.location.path.as_deref(), error.location.line))
        .collect::<Vec<_>>();
    assert_eq!(
        locations,
        [
            (Some("content/about.typ"), Some(4)),
            (Some("content/about.typ"), Some(4)),
            (Some("content/posts/note.typ"), Some(5)),
            (Some("content/posts/note.typ"), Some(5)),
            (Some("content/absent.typ"), Some(1)),
            (Some("content/absent.typ"), Some(1)),
        ]
    );
    for error in &errors[4..] {
        assert_eq!(error.location.column, Some(1));
    }
}

#[test]
fn metadata_failure_carries_its_code() {
    let (directory, world) = source_world(
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
    let error: anyhow::Error = tola_typst::compile_bundle_world(&world, &Default::default())
        .expect_err("the schema default must fail")
        .into();
    let diagnostics = crate::compiler::diagnostic::error_diagnostics(&error, directory.path())
        .expect("a compile failure has diagnostics");
    assert_eq!(
        diagnostics[0].code,
        tola_diagnostic::codes::source::METADATA
    );
    assert!(
        diagnostics[0].message.starts_with("`content/note.typ`: "),
        "{}",
        diagnostics[0].message
    );
    // The record keeps the evidence; presentation decides whether to show it.
    assert!(!diagnostics[0].trace.is_empty());
}
