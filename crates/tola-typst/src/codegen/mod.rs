//! JSON conversion, Typst literal parsing, and Content reconstruction.
#![allow(deprecated)]

mod deserialize;
mod error;
mod inputs;
mod literal;
mod lookup;
mod serialize;

const TYPST_TYPE_TAG_FIELD: &str = "_typst_type";

pub use serialize::{content_to_json, json_to_simple_text, value_to_json};

pub use deserialize::{json_to_content, json_to_value};

pub use literal::{parse_angle, parse_color, parse_length, parse_ratio, parse_typst_literal};

pub use inputs::{inputs_from_json, inputs_from_json_with_content, json_to_simple_value};

pub use error::ConvertError;

#[cfg(test)]
mod tests {
    mod common {
        use std::fs;
        use tempfile::TempDir;

        use serde_json::Value as JsonValue;
        use typst::World;
        use typst::comemo::Track;
        use typst::engine::{Engine, Route, Sink, Traced};
        use typst::foundations::{Content, Context};
        use typst::introspection::EmptyIntrospector;
        use typst::utils::Protected;

        use crate::codegen::{content_to_json, json_to_content};
        use crate::world::TypstWorld;

        fn fontless_world(dir: &TempDir, source: &str) -> TypstWorld {
            let file = dir.path().join("test.typ");
            fs::write(&file, source).unwrap();
            TypstWorld::builder(&file, dir.path())
                .with_local_cache()
                .no_fonts()
                .build(&crate::BundleCancellation::default())
                .expect("valid test world")
        }

        pub(super) struct ConversionWorld {
            _dir: TempDir,
            world: TypstWorld,
        }

        impl ConversionWorld {
            pub(super) fn new() -> Self {
                let dir = TempDir::new().unwrap();
                let world = fontless_world(&dir, "");
                Self { _dir: dir, world }
            }

            pub(super) fn run<F, R>(&self, f: F) -> R
            where
                F: FnOnce(&mut Engine, typst::comemo::Tracked<Context>, &typst::Library) -> R,
            {
                let introspector = EmptyIntrospector;
                let traced = Traced::default();
                let mut sink = Sink::new();

                let mut engine = Engine {
                    world: (&self.world as &dyn typst::World).track(),
                    introspector: Protected::new(introspector.track()),
                    traced: traced.track(),
                    sink: sink.track_mut(),
                    route: Route::default(),
                    library: self.world.library(),
                };

                let library = self.world.library();
                let context = Context::none();

                f(&mut engine, context.track(), library)
            }
        }

        pub(super) fn assert_roundtrip(env: &ConversionWorld, json: JsonValue) {
            env.run(|engine, context, library| {
                let content = json_to_content(engine, context, library, &json)
                    .expect("json_to_content failed");
                let generated = content_to_json(&content).expect("content_to_json failed");
                assert_eq!(json, generated, "converted JSON is not the input shape");
                let restored = json_to_content(engine, context, library, &generated)
                    .expect("generated JSON could not be restored");
                let result = content_to_json(&restored).expect("second content_to_json failed");

                assert_eq!(generated, result, "generated JSON is not stable");
            });
        }

        pub(super) fn assert_content_roundtrip(env: &ConversionWorld, content: Content) {
            env.run(|engine, context, library| {
                let json1 = content_to_json(&content).expect("content_to_json failed");
                let content2 =
                    json_to_content(engine, context, library, &json1).unwrap_or_else(|error| {
                        panic!(
                            "json_to_content failed: {error}\n{}",
                            serde_json::to_string_pretty(&json1).unwrap()
                        )
                    });
                let json2 = content_to_json(&content2).expect("second content_to_json failed");
                assert_eq!(json1, json2, "content roundtrip mismatch");
            });
        }

        #[cfg(feature = "scan")]
        pub(super) fn compile_typst(source: &str) -> Content {
            let dir = TempDir::new().unwrap();
            let world = fontless_world(&dir, source);
            crate::scan_world(&world)
                .expect("Typst compilation failed")
                .content()
                .clone()
        }

        #[cfg(feature = "scan")]
        pub(super) fn assert_typst_roundtrip(env: &ConversionWorld, source: &str) {
            let content = compile_typst(source);
            assert_content_roundtrip(env, content);
        }
    }
    mod primitive {
        use serde_json::json;
        use typst::foundations::NativeElement;
        use typst::model::ParbreakElem;
        use typst::text::LinebreakElem;

        use super::common::{ConversionWorld, assert_content_roundtrip, assert_roundtrip};

        #[test]
        fn primitive_elements_roundtrip() {
            let env = ConversionWorld::new();

            for text in ["hello", "", "你好 🌍", "a\"b<c>&d"] {
                assert_roundtrip(&env, json!({"func": "text", "text": text}));
            }

            assert_roundtrip(&env, json!({"func": "space"}));

            assert_roundtrip(&env, json!({"func": "sequence", "children": []}));

            for children in [
                json!([
                    {"func": "text", "text": "A"},
                    {"func": "space"},
                    {"func": "text", "text": "B"}
                ]),
                json!([
                    {"func": "text", "text": "outer"},
                    {
                        "func": "sequence",
                        "children": [
                            {"func": "text", "text": "inner1"},
                            {"func": "text", "text": "inner2"}
                        ]
                    }
                ]),
            ] {
                assert_roundtrip(&env, json!({"func": "sequence", "children": children}));
            }

            assert_content_roundtrip(&env, LinebreakElem::new().pack());
            assert_content_roundtrip(&env, ParbreakElem::shared().clone());
        }
    }
    mod value {
        use serde_json::json;
        use typst::foundations::Value;

        use crate::codegen::json_to_value;

        use super::common::ConversionWorld;

        #[test]
        fn json_shapes_become_typst_values() {
            let env = ConversionWorld::new();

            env.run(|engine, context, library| {
                let mut convert = |json: &serde_json::Value| {
                    json_to_value(engine, context, library, json).unwrap()
                };

                assert!(matches!(convert(&json!(null)), Value::None));
                assert!(matches!(convert(&json!(true)), Value::Bool(true)));
                assert!(matches!(convert(&json!(42)), Value::Int(42)));
                assert!(
                    matches!(convert(&json!(3.125)), Value::Float(f) if (f - 3.125).abs() < 0.001)
                );
                assert!(matches!(convert(&json!("hello")), Value::Str(s) if s.as_str() == "hello"));

                for text in ["12pt", "auto", "none", "true", "#ff0000"] {
                    assert!(
                        matches!(convert(&json!(text)), Value::Str(_)),
                        "{text:?} was not a string"
                    );
                }

                assert!(matches!(convert(&json!([1, 2, 3])), Value::Array(a) if a.len() == 3));

                assert!(
                    matches!(convert(&json!({"a": 1, "b": "two"})), Value::Dict(d) if d.len() == 2)
                );
                assert!(matches!(
                    convert(&json!({"func": "text", "text": "hi"})),
                    Value::Content(_)
                ));
            });
        }

        #[test]
        fn rebuilt_content_renders_through_inputs() {
            use std::fs;
            use std::sync::Arc;

            use tempfile::TempDir;
            use typst::foundations::{Dict, IntoValue};

            use crate::codegen::json_to_content;
            use crate::compile::compile_world;
            use crate::world::TypstWorld;
            use crate::world::font::FontStore;

            let env = ConversionWorld::new();
            let json = json!({
                "func": "sequence",
                "children": [
                    {"func": "text", "text": "Check out "},
                    {"func": "link", "dest": "https://example.com", "body": {"func": "text", "text": "this link"}},
                    {"func": "text", "text": "!"}
                ]
            });
            let content = env.run(|engine, context, library| {
                json_to_content(engine, context, library, &json).expect("json_to_content failed")
            });

            let mut inputs = Dict::new();
            inputs.insert("summary".into(), content.into_value());

            // Injection compiles with fonts, which the shared test world leaves out.
            let dir = TempDir::new().unwrap();
            let file = dir.path().join("test.typ");
            fs::write(
                &file,
                "#let summary = sys.inputs.summary\nSummary: #summary\n",
            )
            .unwrap();

            let world = TypstWorld::builder(&file, dir.path())
                .with_local_cache()
                .with_fonts(Arc::new(FontStore::new()))
                .with_inputs_dict(inputs)
                .build(&crate::BundleCancellation::default())
                .expect("valid temporary conversion world");

            let result = compile_world(&world);
            assert!(result.is_ok(), "Compilation failed: {:?}", result.err());

            let html_bytes = result.unwrap().html().expect("HTML export failed");
            let html_str = String::from_utf8_lossy(&html_bytes);

            assert!(
                html_str.contains("Check out")
                    && html_str.contains("this link")
                    && html_str.contains("example.com"),
                "Output should contain the summary with link, got: {}",
                html_str
            );
        }
    }
    #[cfg(feature = "scan")]
    mod math {
        use typst::foundations::NativeElement;
        use typst::math::{AttachElem, EquationElem, FracElem, RootElem};
        use typst::text::TextElem;

        use super::common::{ConversionWorld, assert_content_roundtrip, assert_typst_roundtrip};

        #[test]
        fn math_elements_roundtrip() {
            let env = ConversionWorld::new();

            for source in [
                "$x$",
                "$1/2$",
                "$sqrt(x)$",
                "$root(3, x)$",
                "$x^2$",
                "$x_1^2$",
            ] {
                assert_typst_roundtrip(&env, source);
            }

            assert_content_roundtrip(&env, EquationElem::new(TextElem::packed("x")).pack());

            assert_content_roundtrip(
                &env,
                FracElem::new(TextElem::packed("1"), TextElem::packed("2")).pack(),
            );

            assert_content_roundtrip(&env, RootElem::new(TextElem::packed("x")).pack());

            assert_content_roundtrip(
                &env,
                AttachElem::new(TextElem::packed("x"))
                    .with_t(Some(TextElem::packed("2")))
                    .pack(),
            );
        }
    }
    #[cfg(feature = "scan")]
    mod model {
        use serde_json::json;

        use crate::codegen::content_to_json;

        use super::common::{ConversionWorld, assert_typst_roundtrip, compile_typst};

        #[test]
        fn model_sources_roundtrip() {
            let env = ConversionWorld::new();

            for source in [
                "*bold*",
                "_italic_",
                "*_bold italic_*",
                r#"#link("https://example.com")[click]"#,
                r#"#link("https://example.com")[*bold link*]"#,
                r#"See #link("https://example.com")[*this*] for details."#,
                "= Title",
                "== Subtitle",
                "=== Level 3",
                "= *Bold* Heading",
                "Some *bold* and _italic_ text.",
                r#"*_#link("https://example.com")[deep nested]_*"#,
                "/ Term: Definition\n/ Another: Description",
                r#"#list([Item 1], [Item 2])"#,
                r#"#enum([First], [Second])"#,
                r#"#terms(terms.item([Term], [Definition]))"#,
                r#"#grid(
  columns: 2,
  [A], [B],
  [C], [D]
)"#,
                r#"#table(
  [A], [B]
)"#,
                r#"#grid(
  grid.cell[A],
  grid.cell[B]
)"#,
                r#"#table(
  table.cell[A],
  table.cell[B]
)"#,
                r#"#grid(
  columns: 2,
  grid.header(
    grid.cell[H1], grid.cell[H2]
  ),
  [A], [B]
)"#,
                r#"#table(
  columns: 2,
  table.header(
    table.cell[H1], table.cell[H2]
  ),
  [A], [B]
)"#,
                r#"#grid(
  columns: 2,
  [A], [B],
  grid.footer(
    grid.cell[F1], grid.cell[F2]
  )
)"#,
                r#"#table(
  columns: 2,
  [A], [B],
  table.footer(
    table.cell[F1], table.cell[F2]
  )
)"#,
                r#"#grid(
  columns: 2,
  grid.hline(),
  [A], [B],
  grid.hline(),
  [C], [D],
  grid.vline(x: 1)
)"#,
                r#"#table(
  columns: 2,
  table.hline(),
  [A], [B],
  table.hline(),
  [C], [D],
  table.vline(x: 1)
)"#,
                r#"#table(
  table.cell[$x^2$],
  table.cell[$sqrt(y)$]
)"#,
                r#"#grid(
  columns: 2,
  grid.header(
    grid.cell[*Header 1*],
    grid.cell[$alpha$]
  ),
  grid.cell[A],
  grid.cell[$x + y$]
)"#,
            ] {
                assert_typst_roundtrip(&env, source);
            }
        }

        #[test]
        fn cell_resolves_through_ancestor_scope() {
            // `grid.cell` and `table.cell` both serialize to `cell`, and their `header`
            // elements to `header`, so the serialized name alone is ambiguous.
            let env = ConversionWorld::new();

            for (parent, children) in [
                (
                    "grid",
                    json!([{"func": "cell", "body": {"func": "text", "text": "A"}}]),
                ),
                (
                    "table",
                    json!([{"func": "cell", "body": {"func": "text", "text": "B"}}]),
                ),
                (
                    "grid",
                    json!([
                        {
                            "func": "header",
                            "children": [
                                {"func": "cell", "body": {"func": "text", "text": "Header Cell"}}
                            ]
                        },
                        {"func": "cell", "body": {"func": "text", "text": "Body Cell"}}
                    ]),
                ),
                (
                    "table",
                    json!([
                        {
                            "func": "header",
                            "children": [
                                {"func": "cell", "body": {"func": "text", "text": "Header Cell"}}
                            ]
                        },
                        {"func": "cell", "body": {"func": "text", "text": "Body Cell"}}
                    ]),
                ),
            ] {
                let input = json!({"func": parent, "children": children});

                env.run(|engine, context, library| {
                    use crate::codegen::json_to_content;

                    let restored = json_to_content(engine, context, library, &input).unwrap();
                    assert_eq!(restored.elem().name(), parent);

                    let back_json = content_to_json(&restored).unwrap();
                    assert_eq!(input, back_json);
                });
            }
        }

        #[test]
        fn cell_outside_grid_or_table_is_ambiguous() {
            let env = ConversionWorld::new();
            let json = json!({
                "func": "strong",
                "body": {"func": "cell", "body": {"func": "text", "text": "test"}}
            });

            env.run(|engine, context, library| {
                use crate::codegen::json_to_content;

                let error = json_to_content(engine, context, library, &json)
                    .expect_err("cell without a grid or table parent is ambiguous");
                assert!(matches!(
                    error,
                    crate::codegen::ConvertError::Ambiguous { path, func }
                        if path == "/body" && func == "cell"
                ));
            });
        }

        #[test]
        fn bare_markdown_item_is_ambiguous() {
            let env = ConversionWorld::new();

            let md_list = compile_typst(
                r#"- Item 1
- Item 2"#,
            );
            let md_json = content_to_json(&md_list).unwrap();
            env.run(|engine, context, library| {
                use crate::codegen::json_to_content;
                let error = json_to_content(engine, context, library, &md_json)
                    .expect_err("bare serialized item must not be guessed");
                assert!(matches!(
                    error,
                    crate::codegen::ConvertError::Ambiguous { func, .. } if func == "item"
                ));
            });
        }
    }
    #[cfg(feature = "scan")]
    mod edge {
        use serde_json::json;

        use crate::codegen::{ConvertError, content_to_json, json_to_content};

        use super::common::{ConversionWorld, assert_typst_roundtrip};

        #[test]
        fn literal_shaped_strings_remain_strings() {
            let env = ConversionWorld::new();

            for text in ["12pt", "auto", "none", "true", "#ff0000", "#中123"] {
                assert_typst_roundtrip(&env, &format!("#raw({text:?})"));
            }
        }

        #[test]
        fn metadata_native_values_keep_type_tags() {
            use typst::foundations::{Datetime, Dict, NativeElement, Value};
            use typst::introspection::MetadataElem;
            use typst::layout::{Abs, Angle, Em, Length, Ratio};
            use typst::text::TextElem;
            use typst::visualize::Color;

            let env = ConversionWorld::new();
            let mut dict = Dict::new();
            dict.insert(
                "length".into(),
                Value::Length(Length {
                    abs: Abs::pt(12.0),
                    em: Em::new(1.5),
                }),
            );
            dict.insert("angle".into(), Value::Angle(Angle::deg(90.0)));
            dict.insert("ratio".into(), Value::Ratio(Ratio::new(0.5)));
            dict.insert(
                "color".into(),
                Value::Color(Color::from_u8(255, 32, 0, 128)),
            );
            dict.insert(
                "datetime".into(),
                Value::Datetime(Datetime::from_ymd_hms(2026, 8, 17, 12, 34, 56).unwrap()),
            );
            dict.insert("content".into(), Value::Content(TextElem::packed("nested")));
            dict.insert("literal-string".into(), Value::Str("12pt".into()));
            dict.insert(
                "nested-dict".into(),
                Value::Dict({
                    let mut nested = Dict::new();
                    nested.insert("value".into(), Value::Str("12pt".into()));
                    nested
                }),
            );

            let metadata = MetadataElem::new(Value::Dict(dict)).pack();
            super::common::assert_content_roundtrip(&env, metadata.clone());

            let json = content_to_json(&metadata).unwrap();
            let value = json.get("value").unwrap();
            assert_eq!(value["length"]["_typst_type"], "length");
            assert_eq!(value["datetime"]["_typst_type"], "datetime");
            assert_eq!(value["literal-string"], "12pt");
            assert_eq!(value["nested-dict"]["value"], "12pt");
        }

        #[test]
        fn unsupported_values_are_rejected() {
            use typst::foundations::{Bytes, Value};

            let error = crate::codegen::value_to_json(&Value::Bytes(Bytes::new(*b"opaque")))
                .expect_err("bytes are outside the supported JSON subset");
            assert!(error.to_string().contains("bytes"), "{error}");
        }

        #[test]
        fn unconvertible_json_reports_its_path() {
            let env = ConversionWorld::new();

            env.run(|engine, context, library| {
                assert!(matches!(
                    json_to_content(engine, context, library, &json!({"text": "hello"})),
                    Err(ConvertError::MissingFunc { path }) if path == "/"
                ));

                for invalid in [json!("string"), json!(123), json!(null)] {
                    assert!(json_to_content(engine, context, library, &invalid).is_err());
                }

                for (func, extra) in [
                    ("nonexistent", json!(null)),
                    ("context", json!(null)),
                    ("state-update", json!("preview")),
                ] {
                    let mut input = json!({"func": func});
                    if !extra.is_null() {
                        input["key"] = extra;
                    }
                    let err = json_to_content(engine, context, library, &input)
                        .expect_err("unreconstructible function must stay unsupported");
                    assert!(matches!(
                        err,
                        ConvertError::Unsupported { path, func: reported }
                            if path == "/" && reported == func
                    ));
                }
            });
        }

        #[test]
        fn lossy_content_is_rejected() {
            let env = ConversionWorld::new();

            env.run(|engine, context, library| {
                let styled = json!({
                    "func": "styled",
                    "child": {"func": "text", "text": "text"},
                    "styles": ".."
                });
                assert!(matches!(
                    json_to_content(engine, context, library, &styled),
                    Err(ConvertError::Unsupported { func, .. }) if func == "styled"
                ));

                for text in ["", "ab"] {
                    assert!(matches!(
                        json_to_content(
                            engine,
                            context,
                            library,
                            &json!({"func": "symbol", "text": text}),
                        ),
                        Err(ConvertError::InvalidSymbol { .. })
                    ));
                }
            });
        }

        #[test]
        fn explicit_none_is_preserved() {
            use typst::foundations::{Dict, NativeElement, Value};
            use typst::introspection::MetadataElem;
            use typst::math::RootElem;
            use typst::text::TextElem;

            let root = RootElem::new(TextElem::packed("x")).pack();
            let json = content_to_json(&root).unwrap();

            assert!(json.get("index").is_none());
            assert!(json.get("radicand").is_some());

            let mut dict = Dict::new();
            dict.insert("present".into(), Value::None);
            let json = crate::codegen::value_to_json(&Value::Dict(dict.clone())).unwrap();
            assert_eq!(json.get("present"), Some(&serde_json::Value::Null));

            let metadata = MetadataElem::new(Value::Dict(dict)).pack();
            let json = content_to_json(&metadata).unwrap();
            assert_eq!(
                json.get("value").and_then(|value| value.get("present")),
                Some(&serde_json::Value::Null)
            );
        }
    }
}
