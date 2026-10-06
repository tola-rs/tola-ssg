//! JSON conversion for `sys.inputs` dictionaries.

use std::path::Path;
use std::sync::Arc;

use serde_json::Value as JsonValue;
use typst::World;
use typst::comemo::Track;
use typst::engine::{Engine, Route, Sink, Traced};
use typst::foundations::{Array, Context, Dict, IntoValue, Value};
use typst::introspection::EmptyIntrospector;
use typst::utils::Protected;

use super::ConvertError;
use super::deserialize::{
    field_path, index_path, json_to_content_at, json_type_name, number_to_value,
};
use crate::world::TypstWorld;

/// Convert a JSON object into a native Typst dictionary for `sys.inputs`.
///
/// JSON strings, numbers, booleans, arrays, objects, and null map directly to
/// their Typst value counterparts. Objects containing a `func` field remain
/// dictionaries; use [`inputs_from_json_with_content`] when serialized Typst
/// content must be reconstructed.
pub fn inputs_from_json(json: &JsonValue) -> Result<Dict, ConvertError> {
    convert_json_dict(json, "/", None)
}

/// Convert a JSON object into a Typst dictionary and reconstruct Content.
///
/// Objects containing a `func` field are treated as serialized Typst Content.
/// Ambiguous, contextual, or unsupported Content is rejected with a field path.
/// Prefer native [`Dict`] values when data remains in the same process.
pub fn inputs_from_json_with_content(json: &JsonValue, root: &Path) -> Result<Dict, ConvertError> {
    let converter = ContentConverter::new(root);
    convert_json_dict(json, "/", Some(&converter))
}

/// Convert JSON to Typst values, keeping objects with `func` as dictionaries.
pub fn json_to_simple_value(json: &JsonValue) -> Result<Value, ConvertError> {
    convert_json(json, "/", None)
}

fn convert_json(
    json: &JsonValue,
    path: &str,
    content: Option<&ContentConverter>,
) -> Result<Value, ConvertError> {
    match json {
        JsonValue::Null => Ok(Value::None),
        JsonValue::Bool(value) => Ok(value.into_value()),
        JsonValue::Number(number) => number_to_value(number, path),
        JsonValue::String(value) => Ok(value.as_str().into_value()),
        JsonValue::Array(values) => {
            let mut array = Array::new();
            for (index, value) in values.iter().enumerate() {
                array.push(convert_json(value, &index_path(path, index), content)?);
            }
            Ok(array.into_value())
        }
        JsonValue::Object(object) => {
            if let Some(content) = content
                && object.contains_key("func")
            {
                return content.rebuild_content(json, path);
            }
            Ok(convert_json_dict(json, path, content)?.into_value())
        }
    }
}

fn convert_json_dict(
    json: &JsonValue,
    path: &str,
    content: Option<&ContentConverter>,
) -> Result<Dict, ConvertError> {
    let obj = json
        .as_object()
        .ok_or_else(|| ConvertError::ExpectedObject {
            path: path.to_string(),
            actual: json_type_name(json),
        })?;

    let mut dict = Dict::new();
    for (key, value) in obj {
        dict.insert(
            key.as_str().into(),
            convert_json(value, &field_path(path, key), content)?,
        );
    }
    Ok(dict)
}

struct ContentConverter {
    world: Arc<TypstWorld>,
}

impl ContentConverter {
    fn new(root: &Path) -> Self {
        // Content reconstruction needs a world but never reads its main source.
        let dummy_path = root.join("__content_converter_dummy.typ");
        let world = TypstWorld::builder(&dummy_path, root)
            .with_local_cache()
            .no_fonts()
            .build(&crate::BundleCancellation::default())
            .expect("generated content-converter main must be inside its compilation root");

        Self {
            world: Arc::new(world),
        }
    }

    fn rebuild_content(&self, json: &JsonValue, path: &str) -> Result<Value, ConvertError> {
        let introspector = EmptyIntrospector;
        let traced = Traced::default();
        let mut sink = Sink::new();

        let mut engine = Engine {
            world: (&*self.world as &dyn World).track(),
            introspector: Protected::new(introspector.track()),
            traced: traced.track(),
            sink: sink.track_mut(),
            route: Route::default(),
            library: self.world.library(),
        };

        let library = self.world.library();
        let context = Context::none();

        let content = json_to_content_at(&mut engine, context.track(), library, json, path)?;
        Ok(content.into_value())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;
    use typst::foundations::Str;

    #[test]
    fn json_values_become_typst_inputs() {
        let inputs = inputs_from_json(&json!({
            "title": "My Blog",
            "count": 42,
            "draft": false,
            "value": null,
            "tags": ["rust", "typst", "blog"],
            "extra": {"author": "Alice", "twitter": "@alice"}
        }))
        .unwrap();

        assert_eq!(
            inputs
                .get(&Str::from("title"))
                .unwrap()
                .clone()
                .cast::<Str>()
                .unwrap()
                .as_str(),
            "My Blog"
        );
        assert_eq!(inputs.get(&Str::from("count")).unwrap(), &Value::Int(42));
        assert_eq!(
            inputs.get(&Str::from("draft")).unwrap(),
            &Value::Bool(false)
        );
        assert_eq!(inputs.get(&Str::from("value")).unwrap(), &Value::None);
        assert_eq!(
            inputs
                .get(&Str::from("tags"))
                .unwrap()
                .clone()
                .cast::<Array>()
                .unwrap()
                .len(),
            3
        );
        assert_eq!(
            inputs
                .get(&Str::from("extra"))
                .unwrap()
                .clone()
                .cast::<Dict>()
                .unwrap()
                .get(&Str::from("author"))
                .unwrap(),
            &Value::Str("Alice".into())
        );
    }

    #[test]
    fn non_object_root_reports_json_type() {
        assert!(matches!(
            inputs_from_json(&json!("not an object")),
            Err(ConvertError::ExpectedObject { path, actual })
                if path == "/" && actual == "string"
        ));

        let dir = TempDir::new().unwrap();
        assert!(matches!(
            inputs_from_json_with_content(&json!([{"title": "Post"}]), dir.path()),
            Err(ConvertError::ExpectedObject { path, actual })
                if path == "/" && actual == "array"
        ));
    }

    #[test]
    fn integers_respect_typst_range() {
        assert_eq!(
            json_to_simple_value(&json!(i64::MAX)).unwrap(),
            Value::Int(i64::MAX)
        );

        assert!(matches!(
            inputs_from_json(&json!({"pages": [{"id": u64::MAX}]})),
            Err(ConvertError::IntegerOutOfRange { path, value })
                if path == "/pages/0/id" && value == u64::MAX.to_string()
        ));
    }

    #[test]
    fn content_inputs_rebuild_at_every_depth() {
        let dir = TempDir::new().unwrap();
        let json = json!({
            "pages": [
                {
                    "url": "/post/1",
                    "title": "First Post",
                    "summary": {"func": "text", "text": "Hello world"}
                },
                {
                    "url": "/post/2",
                    "title": "Second Post",
                    "summary": {
                        "func": "sequence",
                        "children": [
                            {"func": "text", "text": "Check out "},
                            {
                                "func": "link",
                                "dest": "https://example.com",
                                "body": {"func": "text", "text": "this link"}
                            },
                            {"func": "text", "text": "!"}
                        ]
                    }
                }
            ]
        });

        let inputs = inputs_from_json_with_content(&json, dir.path()).unwrap();
        let pages = inputs
            .get(&Str::from("pages"))
            .unwrap()
            .clone()
            .cast::<Array>()
            .unwrap();
        assert_eq!(pages.len(), 2);

        for index in 0..pages.len() as i64 {
            let page = pages
                .at(index, None)
                .unwrap()
                .clone()
                .cast::<Dict>()
                .unwrap();
            assert!(
                matches!(page.get(&Str::from("summary")), Ok(Value::Content(_))),
                "page {index} summary must be Content, not Dict"
            );
        }
    }

    #[test]
    fn content_inputs_report_unsupported_path() {
        let dir = TempDir::new().unwrap();
        let json = json!({
            "pages": [
                {
                    "permalink": "/a/",
                    "summary": {"func": "context"}
                }
            ]
        });

        let err = match inputs_from_json_with_content(&json, dir.path()) {
            Ok(_) => panic!("context content should be unsupported"),
            Err(err) => err,
        };

        assert!(matches!(
            err,
            ConvertError::Unsupported { path, func }
                if path == "/pages/0/summary" && func == "context"
        ));
    }
}
