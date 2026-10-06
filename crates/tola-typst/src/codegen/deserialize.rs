//! JSON → Typst deserialization.

use serde_json::{Map, Value as JsonValue};
use typst::Library;
use typst::comemo::Tracked;
use typst::ecow::EcoVec;
use typst::engine::Engine;
use typst::foundations::{
    Arg, Args, CastInfo, Content, Context, Dict, Func, ParamInfo, Str, Value,
};
use typst::foundations::{Datetime, Label, SymbolElem};
use typst::layout::{Abs, Angle, Em, Fr, Length, Ratio, Rel};
use typst::syntax::{Span, Spanned};
use typst::text::{SpaceElem, TextElem};

use super::error::ConvertError;
use super::literal::{parse_color, parse_typst_literal};
use super::lookup::{find_element_funcs, find_element_in_scope};

/// Reconstruct the supported subset of `typst query` Content JSON.
///
/// `text`, `space`, `sequence`, and `symbol` take custom constructors because
/// their field shape does not match the public function's parameters.
pub fn json_to_content(
    engine: &mut Engine,
    context: Tracked<Context>,
    library: &Library,
    json: &JsonValue,
) -> Result<Content, ConvertError> {
    json_to_content_at(engine, context, library, json, "/")
}

pub(super) fn json_to_content_at(
    engine: &mut Engine,
    context: Tracked<Context>,
    library: &Library,
    json: &JsonValue,
    path: &str,
) -> Result<Content, ConvertError> {
    json_to_content_with_ancestors(engine, context, library, json, &[], path)
}

/// Searches ancestor scopes nearest-first: `grid.header.cell` looks in
/// `grid.header`, then `grid`, to find `grid.cell`.
fn json_to_content_with_ancestors(
    engine: &mut Engine,
    context: Tracked<Context>,
    library: &Library,
    json: &JsonValue,
    ancestors: &[Func],
    path: &str,
) -> Result<Content, ConvertError> {
    let obj = json
        .as_object()
        .ok_or_else(|| ConvertError::ExpectedObject {
            path: path.to_string(),
            actual: json_type_name(json),
        })?;
    let func_name =
        obj.get("func")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ConvertError::MissingFunc {
                path: path.to_string(),
            })?;

    match func_name {
        "text" => {
            let text = obj
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or_else(|| missing_field(path, "text"))?;
            return Ok(TextElem::packed(text));
        }
        "space" => {
            return Ok(SpaceElem::shared().clone());
        }
        "symbol" => {
            let text = obj
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or_else(|| missing_field(path, "text"))?;
            let mut chars = text.chars();
            let symbol = chars
                .next()
                .filter(|_| chars.next().is_none())
                .ok_or_else(|| ConvertError::InvalidSymbol {
                    path: path.to_string(),
                })?;
            return Ok(SymbolElem::packed(symbol));
        }
        "sequence" => {
            let children = obj
                .get("children")
                .and_then(|v| v.as_array())
                .ok_or_else(|| missing_field(path, "children"))?;
            let contents: Result<Vec<Content>, _> = children
                .iter()
                .enumerate()
                .map(|(index, c)| {
                    json_to_content_with_ancestors(
                        engine,
                        context,
                        library,
                        c,
                        ancestors,
                        &index_path(&field_path(path, "children"), index),
                    )
                })
                .collect();
            return Ok(Content::sequence(contents?));
        }
        "styled" => {
            return Err(ConvertError::Unsupported {
                path: path.to_string(),
                func: func_name.to_string(),
            });
        }
        _ => {}
    }

    let func = if let Some(func) = ancestors
        .iter()
        .rev()
        .find_map(|ancestor| find_element_in_scope(ancestor, func_name))
    {
        func
    } else {
        find_best_matching_element(library, func_name, obj, path)?.ok_or_else(|| {
            ConvertError::Unsupported {
                path: path.to_string(),
                func: func_name.to_string(),
            }
        })?
    };

    let args = build_args(&func, obj, engine, context, library, ancestors, path)?;

    func.call(engine, context, args)
        .map_err(|e| ConvertError::CallFailed {
            path: path.to_string(),
            func: func_name.to_string(),
            reason: e
                .iter()
                .map(|d| d.message.to_string())
                .collect::<Vec<_>>()
                .join("; "),
        })?
        .cast::<Content>()
        .map_err(|_| ConvertError::CallFailed {
            path: path.to_string(),
            func: func_name.to_string(),
            reason: "function did not return Typst content".to_string(),
        })
}

/// Convert JSON to Typst Value.
pub fn json_to_value(
    engine: &mut Engine,
    context: Tracked<Context>,
    library: &Library,
    json: &JsonValue,
) -> Result<Value, ConvertError> {
    json_to_value_with_ancestors(engine, context, library, json, &[], "/")
}

/// `_typst_type` markers preserve types JSON cannot represent directly, for
/// example `{"_typst_type": "length", "abs": 12.0, "em": 0.0}`.
fn json_to_value_with_ancestors(
    engine: &mut Engine,
    context: Tracked<Context>,
    library: &Library,
    json: &JsonValue,
    ancestors: &[Func],
    path: &str,
) -> Result<Value, ConvertError> {
    match json {
        JsonValue::Null => Ok(Value::None),
        JsonValue::Bool(b) => Ok(Value::Bool(*b)),
        JsonValue::Number(number) => number_to_value(number, path),
        JsonValue::String(s) => Ok(Value::Str(s.as_str().into())),
        JsonValue::Array(arr) => {
            let items: Result<Vec<Value>, _> = arr
                .iter()
                .enumerate()
                .map(|(index, v)| {
                    json_to_value_with_ancestors(
                        engine,
                        context,
                        library,
                        v,
                        ancestors,
                        &index_path(path, index),
                    )
                })
                .collect();
            Ok(Value::Array(items?.into_iter().collect()))
        }
        JsonValue::Object(obj) => {
            if let Some(type_tag) = obj
                .get(super::TYPST_TYPE_TAG_FIELD)
                .and_then(JsonValue::as_str)
            {
                return parse_typed_value(type_tag, obj, path);
            }

            if obj.contains_key("func") {
                let content = json_to_content_with_ancestors(
                    engine, context, library, json, ancestors, path,
                )?;
                Ok(Value::Content(content))
            } else {
                let dict: Result<Dict, _> = obj
                    .iter()
                    .map(|(k, v)| {
                        let value = json_to_value_with_ancestors(
                            engine,
                            context,
                            library,
                            v,
                            ancestors,
                            &field_path(path, k),
                        )?;
                        Ok((Str::from(k.as_str()), value))
                    })
                    .collect();
                Ok(Value::Dict(dict?))
            }
        }
    }
}

/// Parse a tagged value emitted by [`super::value_to_json`].
fn parse_typed_value(
    type_tag: &str,
    obj: &Map<String, JsonValue>,
    path: &str,
) -> Result<Value, ConvertError> {
    match type_tag {
        "auto" => Ok(Value::Auto),
        "length" => Ok(Value::Length(Length {
            abs: Abs::raw(tagged_number(obj, "abs", path)?),
            em: Em::new(tagged_number(obj, "em", path)?),
        })),
        "angle" => Ok(Value::Angle(Angle::rad(tagged_number(
            obj, "radians", path,
        )?))),
        "ratio" => Ok(Value::Ratio(Ratio::new(tagged_number(obj, "ratio", path)?))),
        "relative-length" => Ok(Value::Relative(Rel::new(
            Ratio::new(tagged_number(obj, "ratio", path)?),
            Length {
                abs: Abs::raw(tagged_number(obj, "abs", path)?),
                em: Em::new(tagged_number(obj, "em", path)?),
            },
        ))),
        "fraction" => Ok(Value::Fraction(Fr::new(tagged_number(
            obj, "fraction", path,
        )?))),
        "color" => {
            let hex = tagged_string(obj, "hex", path)?;
            parse_color(hex)
                .map(Value::Color)
                .ok_or_else(|| ConvertError::InvalidLiteral {
                    path: path.to_string(),
                    type_name: "color",
                    value: hex.to_string(),
                })
        }
        "label" => Label::construct(tagged_string(obj, "name", path)?.into())
            .map(Value::Label)
            .map_err(|error| ConvertError::InvalidLiteral {
                path: path.to_string(),
                type_name: "label",
                value: error.to_string(),
            }),
        "datetime" => parse_datetime(obj, path).map(Value::Datetime),
        _ => Err(ConvertError::UnknownTypeTag {
            path: path.to_string(),
            tag: type_tag.to_string(),
        }),
    }
}

fn tagged_number(
    obj: &Map<String, JsonValue>,
    field: &'static str,
    path: &str,
) -> Result<f64, ConvertError> {
    obj.get(field)
        .and_then(JsonValue::as_f64)
        .ok_or_else(|| missing_field(path, field))
}

fn tagged_integer(obj: &Map<String, JsonValue>, field: &'static str) -> Option<i64> {
    obj.get(field).and_then(JsonValue::as_i64)
}

fn tagged_string<'a>(
    obj: &'a Map<String, JsonValue>,
    field: &'static str,
    path: &str,
) -> Result<&'a str, ConvertError> {
    obj.get(field)
        .and_then(JsonValue::as_str)
        .ok_or_else(|| missing_field(path, field))
}

fn parse_datetime(obj: &Map<String, JsonValue>, path: &str) -> Result<Datetime, ConvertError> {
    let date = || {
        Some((
            i32::try_from(tagged_integer(obj, "year")?).ok()?,
            u8::try_from(tagged_integer(obj, "month")?).ok()?,
            u8::try_from(tagged_integer(obj, "day")?).ok()?,
        ))
    };
    let time = || {
        Some((
            u8::try_from(tagged_integer(obj, "hour")?).ok()?,
            u8::try_from(tagged_integer(obj, "minute")?).ok()?,
            u8::try_from(tagged_integer(obj, "second")?).ok()?,
        ))
    };
    let value = match (date(), time()) {
        (Some((year, month, day)), Some((hour, minute, second))) => {
            Datetime::from_ymd_hms(year, month, day, hour, minute, second)
        }
        (Some((year, month, day)), None) => Datetime::from_ymd(year, month, day),
        (None, Some((hour, minute, second))) => Datetime::from_hms(hour, minute, second),
        (None, None) => None,
    };
    value.ok_or_else(|| ConvertError::InvalidLiteral {
        path: path.to_string(),
        type_name: "datetime",
        value: JsonValue::Object(obj.clone()).to_string(),
    })
}

pub(super) fn number_to_value(
    number: &serde_json::Number,
    path: &str,
) -> Result<Value, ConvertError> {
    if let Some(value) = number.as_i64() {
        return Ok(Value::Int(value));
    }
    if number.is_u64() {
        return Err(ConvertError::IntegerOutOfRange {
            path: path.to_string(),
            value: number.to_string(),
        });
    }
    Ok(Value::Float(number.as_f64().expect(
        "serde_json numbers are valid finite JSON numbers",
    )))
}

fn missing_field(path: &str, field: &'static str) -> ConvertError {
    ConvertError::MissingField {
        path: path.to_string(),
        field,
    }
}

/// Positional-only arguments keep their order, so a missing optional one that a
/// later positional argument depends on becomes a `none` placeholder; everything
/// else is passed by name and variadic parameters expand positionally.
fn build_args(
    func: &Func,
    obj: &Map<String, JsonValue>,
    engine: &mut Engine,
    context: Tracked<Context>,
    library: &Library,
    ancestors: &[Func],
    path: &str,
) -> Result<Args, ConvertError> {
    let span = Span::detached();
    let mut items: EcoVec<Arg> = EcoVec::new();

    let params: Vec<_> = func.params().collect();

    // Nested constructors must include the current function in recursion detection.
    let mut nested_ancestors = ancestors.to_vec();
    nested_ancestors.push(func.clone());

    let positional_only: Vec<_> = params
        .iter()
        .filter(|p| p.positional() && !p.named())
        .collect();

    for (position, param) in positional_only.iter().enumerate() {
        let Some(name) = param.name() else { continue };
        if let Some(value) = obj.get(name) {
            let value_path = field_path(path, name);
            if param.variadic() {
                if let Some(arr) = value.as_array() {
                    for (index, item) in arr.iter().enumerate() {
                        let typst_value = json_to_param_value(
                            param,
                            engine,
                            context,
                            library,
                            item,
                            &nested_ancestors,
                            &index_path(&value_path, index),
                        )?;
                        items.push(Arg {
                            span,
                            name: None,
                            value: Spanned::new(typst_value, span),
                        });
                    }
                } else {
                    let typst_value = json_to_param_value(
                        param,
                        engine,
                        context,
                        library,
                        value,
                        &nested_ancestors,
                        &value_path,
                    )?;
                    items.push(Arg {
                        span,
                        name: None,
                        value: Spanned::new(typst_value, span),
                    });
                }
            } else {
                let typst_value = json_to_param_value(
                    param,
                    engine,
                    context,
                    library,
                    value,
                    &nested_ancestors,
                    &value_path,
                )?;
                items.push(Arg {
                    span,
                    name: None,
                    value: Spanned::new(typst_value, span),
                });
            }
        } else if !param.required()
            && param_accepts_none(param)
            && positional_only[position + 1..]
                .iter()
                .filter_map(|later| later.name())
                .any(|name| obj.contains_key(name))
        {
            items.push(Arg {
                span,
                name: None,
                value: Spanned::new(Value::None, span),
            });
        }
    }

    let positional_only_names: std::collections::HashSet<_> =
        positional_only.iter().filter_map(|p| p.name()).collect();

    for (key, value) in obj.iter() {
        if key == "func" || positional_only_names.contains(key.as_str()) {
            continue;
        }

        let value_path = field_path(path, key);

        let param = params.iter().find(|p| p.name() == Some(key));
        if let Some(param) = param
            && param.variadic()
            && let Some(arr) = value.as_array()
        {
            for (index, item) in arr.iter().enumerate() {
                let typst_value = json_to_param_value(
                    param,
                    engine,
                    context,
                    library,
                    item,
                    &nested_ancestors,
                    &index_path(&value_path, index),
                )?;
                items.push(Arg {
                    span,
                    name: None,
                    value: Spanned::new(typst_value, span),
                });
            }
            continue;
        }

        let typst_value = match param {
            Some(param) => json_to_param_value(
                param,
                engine,
                context,
                library,
                value,
                &nested_ancestors,
                &value_path,
            )?,
            None => json_to_value_with_ancestors(
                engine,
                context,
                library,
                value,
                &nested_ancestors,
                &value_path,
            )?,
        };
        items.push(Arg {
            span,
            name: Some(Str::from(key.as_str())),
            value: Spanned::new(typst_value, span),
        });
    }

    Ok(Args { span, items })
}

fn json_to_param_value(
    param: &ParamInfo,
    engine: &mut Engine,
    context: Tracked<Context>,
    library: &Library,
    json: &JsonValue,
    ancestors: &[Func],
    path: &str,
) -> Result<Value, ConvertError> {
    if let JsonValue::String(string) = json {
        return Ok(
            parse_param_string(param, string).unwrap_or_else(|| Value::Str(string.as_str().into()))
        );
    }

    if let JsonValue::Array(items) = json {
        let values: Result<Vec<_>, _> = items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                json_to_param_value(
                    param,
                    engine,
                    context,
                    library,
                    item,
                    ancestors,
                    &index_path(path, index),
                )
            })
            .collect();
        return Ok(Value::Array(values?.into_iter().collect()));
    }

    json_to_value_with_ancestors(engine, context, library, json, ancestors, path)
}

fn parse_param_string(param: &ParamInfo, string: &str) -> Option<Value> {
    let parsed = parse_typst_literal(string)?;
    let native = param.to_native()?;
    let expected = parsed.ty().short_name();
    let mut accepts = false;

    native.input.walk(|info| {
        if matches!(info, CastInfo::Type(ty) if ty.short_name() == expected) {
            accepts = true;
        }
    });

    accepts.then_some(parsed)
}

fn param_accepts_none(param: &ParamInfo) -> bool {
    let mut accepts_none = false;
    let Some(native) = param.to_native() else {
        return false;
    };
    native.input.walk(|info| match info {
        CastInfo::Any => accepts_none = true,
        CastInfo::Type(ty) if ty.short_name() == "none" => accepts_none = true,
        _ => {}
    });
    accepts_none
}

pub(super) fn json_type_name(json: &JsonValue) -> &'static str {
    match json {
        JsonValue::Null => "null",
        JsonValue::Bool(_) => "bool",
        JsonValue::Number(_) => "number",
        JsonValue::String(_) => "string",
        JsonValue::Array(_) => "array",
        JsonValue::Object(_) => "object",
    }
}

pub(super) fn field_path(parent: &str, key: &str) -> String {
    child_path(parent, &escape_path_segment(key))
}

pub(super) fn index_path(parent: &str, index: usize) -> String {
    child_path(parent, &index.to_string())
}

fn child_path(parent: &str, segment: &str) -> String {
    if parent.is_empty() || parent == "/" {
        format!("/{segment}")
    } else {
        format!("{parent}/{segment}")
    }
}

fn escape_path_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

/// Ties among equally-scored elements return [`ConvertError::Ambiguous`].
fn find_best_matching_element(
    library: &Library,
    func_name: &str,
    obj: &Map<String, JsonValue>,
    path: &str,
) -> Result<Option<Func>, ConvertError> {
    let candidates: Vec<_> = find_element_funcs(library, func_name).collect();

    if candidates.len() <= 1 {
        return Ok(candidates.into_iter().next());
    }

    let json_fields: std::collections::HashSet<_> = obj.keys().filter(|k| *k != "func").collect();

    let score = |func: &Func| {
        let params: Vec<_> = func.params().collect();
        let param_names: std::collections::HashSet<_> =
            params.iter().filter_map(|p| p.name()).collect();

        let matches = json_fields
            .iter()
            .filter(|f| param_names.contains(f.as_str()))
            .count();

        let missing_required = params
            .iter()
            .filter(|p| {
                p.required()
                    && p.name()
                        .is_some_and(|name| !json_fields.iter().any(|field| field.as_str() == name))
            })
            .count();

        (matches as i32) - (missing_required as i32)
    };

    let mut candidates = candidates.into_iter();
    let mut candidate = candidates.next().expect("not empty");
    let mut best_score = score(&candidate);
    let mut tied = false;
    for next in candidates {
        let next_score = score(&next);
        match next_score.cmp(&best_score) {
            std::cmp::Ordering::Greater => {
                candidate = next;
                best_score = next_score;
                tied = false;
            }
            std::cmp::Ordering::Equal => tied = true,
            std::cmp::Ordering::Less => {}
        }
    }
    if tied {
        return Err(ConvertError::Ambiguous {
            path: path.to_string(),
            func: func_name.to_string(),
        });
    }

    Ok(Some(candidate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_color_tag_reports_its_path() {
        let json = serde_json::json!({ "_typst_type": "color", "hex": "#aéabc" });
        let error = parse_typed_value("color", json.as_object().unwrap(), "/nested/0").unwrap_err();
        assert!(matches!(
            error,
            ConvertError::InvalidLiteral { path, type_name: "color", value }
                if path == "/nested/0" && value == "#aéabc"
        ));
    }
}
