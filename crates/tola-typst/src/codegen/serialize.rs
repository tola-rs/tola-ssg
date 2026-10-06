//! Serializes Typst Content and values to the supported JSON subset.

use std::io;

use serde_json::{Map, Number, Value as JsonValue, json};
use typst::foundations::{Content, Datetime, Value};

use super::TYPST_TYPE_TAG_FIELD;

/// Serialize Content to the JSON subset supported by this crate.
///
/// Null element fields are omitted while null values in nested dictionaries
/// are preserved.
pub fn content_to_json(content: &Content) -> serde_json::Result<JsonValue> {
    let mut object = Map::new();
    object.insert(
        "func".into(),
        JsonValue::String(content.func().name().to_string()),
    );
    for (name, value) in content.fields() {
        let value = value_to_json(&value)?;
        if !value.is_null() {
            object.insert(name.to_string(), value);
        }
    }
    Ok(JsonValue::Object(object))
}

/// Serialize a Typst value to the JSON subset supported by this crate.
///
/// A value outside that subset is an error, never a `repr()` string.
pub fn value_to_json(value: &Value) -> serde_json::Result<JsonValue> {
    match value {
        Value::None => Ok(JsonValue::Null),
        Value::Auto => Ok(json!({(TYPST_TYPE_TAG_FIELD): "auto"})),
        Value::Bool(value) => Ok(JsonValue::Bool(*value)),
        Value::Int(value) => Ok(JsonValue::Number((*value).into())),
        Value::Float(value) => Ok(JsonValue::Number(number(*value)?)),
        Value::Length(value) => Ok(json!({
            (TYPST_TYPE_TAG_FIELD): "length",
            "abs": number(value.abs.to_raw())?,
            "em": number(value.em.get())?,
        })),
        Value::Angle(value) => Ok(json!({
            (TYPST_TYPE_TAG_FIELD): "angle",
            "radians": number(value.to_rad())?,
        })),
        Value::Ratio(value) => Ok(json!({
            (TYPST_TYPE_TAG_FIELD): "ratio",
            "ratio": number(value.get())?,
        })),
        Value::Relative(value) => Ok(json!({
            (TYPST_TYPE_TAG_FIELD): "relative-length",
            "ratio": number(value.rel.get())?,
            "abs": number(value.abs.abs.to_raw())?,
            "em": number(value.abs.em.get())?,
        })),
        Value::Fraction(value) => Ok(json!({
            (TYPST_TYPE_TAG_FIELD): "fraction",
            "fraction": number(value.get())?,
        })),
        Value::Color(value) => Ok(json!({
            (TYPST_TYPE_TAG_FIELD): "color",
            "hex": value.to_hex().as_str(),
        })),
        Value::Str(value) => Ok(JsonValue::String(value.to_string())),
        Value::Label(value) => Ok(json!({
            (TYPST_TYPE_TAG_FIELD): "label",
            "name": value.resolve().as_str(),
        })),
        Value::Datetime(value) => datetime_to_json(*value),
        Value::Content(value) => content_to_json(value),
        Value::Array(values) => values.iter().map(value_to_json).collect(),
        Value::Dict(values) => Ok(JsonValue::Object(
            values
                .iter()
                .map(|(key, value)| Ok((key.to_string(), value_to_json(value)?)))
                .collect::<serde_json::Result<_>>()?,
        )),
        other => Err(unsupported_value(other)),
    }
}

fn datetime_to_json(value: Datetime) -> serde_json::Result<JsonValue> {
    let mut object = Map::new();
    object.insert(
        TYPST_TYPE_TAG_FIELD.into(),
        JsonValue::String("datetime".into()),
    );
    for (name, value) in [
        ("year", value.year().map(i64::from)),
        ("month", value.month().map(i64::from)),
        ("day", value.day().map(i64::from)),
        ("hour", value.hour().map(i64::from)),
        ("minute", value.minute().map(i64::from)),
        ("second", value.second().map(i64::from)),
    ] {
        if let Some(value) = value {
            object.insert(name.into(), JsonValue::Number(value.into()));
        }
    }
    Ok(JsonValue::Object(object))
}

fn number(value: f64) -> serde_json::Result<Number> {
    Number::from_f64(value).ok_or_else(|| {
        serde_json::Error::io(io::Error::new(
            io::ErrorKind::InvalidData,
            "non-finite Typst number cannot be represented in JSON",
        ))
    })
}

fn unsupported_value(value: &Value) -> serde_json::Error {
    serde_json::Error::io(io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "Typst value of type {:?} is outside the supported JSON subset",
            value.ty()
        ),
    ))
}

/// Simplify JSON by extracting text from Content objects.
pub fn json_to_simple_text(json: &JsonValue) -> JsonValue {
    match json {
        JsonValue::Object(obj) => {
            if obj.contains_key("func") {
                let mut text = String::new();
                append_content_text(json, &mut text);
                JsonValue::String(text)
            } else {
                JsonValue::Object(
                    obj.iter()
                        .map(|(key, value)| (key.clone(), json_to_simple_text(value)))
                        .collect(),
                )
            }
        }
        JsonValue::Array(values) => {
            JsonValue::Array(values.iter().map(json_to_simple_text).collect())
        }
        other => other.clone(),
    }
}

fn append_content_text(json: &JsonValue, output: &mut String) {
    match json {
        JsonValue::Object(obj) => {
            if let Some(text) = obj.get("text").and_then(JsonValue::as_str) {
                output.push_str(text);
            } else if let Some(body) = obj.get("body") {
                append_content_text(body, output);
            } else if let Some(children) = obj.get("children").filter(|value| value.is_array()) {
                append_content_text(children, output);
            } else if let Some(child) = obj.get("child") {
                append_content_text(child, output);
            }
        }
        JsonValue::Array(values) => {
            for value in values {
                append_content_text(value, output);
            }
        }
        JsonValue::String(value) => output.push_str(value),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn content_objects_reduce_to_text() {
        for (input, expected) in [
            (
                json!({"func": "text", "text": "Hello World"}),
                json!("Hello World"),
            ),
            (
                json!({
                    "func": "link",
                    "dest": "/posts/hello",
                    "body": {"func": "text", "text": "Click here"}
                }),
                json!("Click here"),
            ),
            (
                json!({
                    "func": "sequence",
                    "children": [
                        {"func": "text", "text": "Hello "},
                        {"func": "strong", "body": {"func": "text", "text": "World"}}
                    ]
                }),
                json!("Hello World"),
            ),
        ] {
            assert_eq!(json_to_simple_text(&input), expected);
        }
    }

    #[test]
    fn containers_keep_shape_around_content() {
        let result = json_to_simple_text(&json!({
            "title": "My Post",
            "summary": {"func": "text", "text": "A summary"},
            "next": {
                "func": "link",
                "dest": "/next",
                "body": {"func": "text", "text": "Next Post"}
            }
        }));
        assert_eq!(result["title"], "My Post");
        assert_eq!(result["summary"], "A summary");
        assert_eq!(result["next"], "Next Post");

        let result = json_to_simple_text(&json!([
            {"title": "A", "summary": {"func": "text", "text": "Summary A"}},
            {"title": "B", "summary": {"func": "text", "text": "Summary B"}}
        ]));
        assert_eq!(result[0]["summary"], "Summary A");
        assert_eq!(result[1]["summary"], "Summary B");
    }
}
