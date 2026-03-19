use std::fs;
use std::io::Write;

use anyhow::Result;
use serde_json::{Map, Value as JsonValue};

use crate::cli::args::QueryArgs;
use crate::log;

use super::types::{PageQueryResult, QueryResult};

pub(super) fn output_results(results: &QueryResult, args: &QueryArgs) -> Result<()> {
    // Skip output if no results
    if results.pages.is_empty() {
        return Ok(());
    }

    let output = if let Some(ref fields) = args.fields {
        let normalized_fields = normalize_fields(fields);
        filter_fields(results, &normalized_fields, args.filter_empty)
    } else {
        format_results(results, args.filter_empty)
    };

    // Format output: JSON for --raw, simplified JSON for default
    let output_json = if args.raw {
        output
    } else {
        use typst_batch::codegen::json_to_simple_text;
        json_to_simple_text(&output)
    };

    let formatted = if args.pretty {
        serde_json::to_string_pretty(&output_json)?
    } else {
        serde_json::to_string(&output_json)?
    };

    // Output to file or stdout
    if let Some(ref output_path) = args.output {
        let mut file = fs::File::create(output_path)?;
        writeln!(file, "{}", formatted)?;
        log!("query"; "wrote output to {}", output_path.display());
    } else {
        println!("{}", formatted);
    }

    Ok(())
}

/// Format all results, optionally filtering empty fields
fn format_results(results: &QueryResult, filter_empty: bool) -> JsonValue {
    let pages: Vec<JsonValue> = results
        .pages
        .iter()
        .map(|page| format_page(page, filter_empty))
        .collect();

    JsonValue::Array(pages)
}

/// Format a single page result with path/permalink first
fn format_page(page: &PageQueryResult, filter_empty: bool) -> JsonValue {
    let mut obj = Map::new();

    // path and permalink always first
    obj.insert("path".to_string(), JsonValue::String(page.path.clone()));
    obj.insert(
        "permalink".to_string(),
        JsonValue::String(page.permalink.clone()),
    );

    // Add meta fields
    let meta_value = serde_json::to_value(&page.meta).unwrap_or_default();
    if let JsonValue::Object(meta_obj) = meta_value {
        for (key, value) in meta_obj {
            // Keep canonical top-level keys stable.
            if matches!(key.as_str(), "path" | "permalink") {
                continue;
            }
            if !filter_empty || !is_empty_value(&value) {
                obj.insert(key, value);
            }
        }
    }

    JsonValue::Object(obj)
}

/// Check if a JSON value is considered "empty" (null, "", or [])
fn is_empty_value(value: &JsonValue) -> bool {
    match value {
        JsonValue::Null => true,
        JsonValue::String(s) => s.is_empty(),
        JsonValue::Array(arr) => arr.is_empty(),
        _ => false,
    }
}

/// Filter to specific fields, with path/permalink always included first
fn filter_fields(results: &QueryResult, fields: &[String], filter_empty: bool) -> JsonValue {
    let pages: Vec<JsonValue> = results
        .pages
        .iter()
        .map(|page| {
            let mut obj = Map::new();

            // path and permalink always first
            obj.insert("path".to_string(), JsonValue::String(page.path.clone()));
            obj.insert(
                "permalink".to_string(),
                JsonValue::String(page.permalink.clone()),
            );

            let meta_value = serde_json::to_value(&page.meta).unwrap_or_default();
            if let JsonValue::Object(meta_obj) = meta_value {
                for field in fields {
                    if matches!(field.as_str(), "path" | "permalink") {
                        continue;
                    }

                    if let Some(value) = meta_obj.get(field) {
                        if !filter_empty || !is_empty_value(value) {
                            obj.insert(field.clone(), value.clone());
                        }
                    } else if !filter_empty {
                        // Field explicitly requested but doesn't exist - show null when not filtering
                        obj.insert(field.clone(), JsonValue::Null);
                    }
                }
            }

            JsonValue::Object(obj)
        })
        .collect();

    JsonValue::Array(pages)
}

/// Normalize field list from CLI.
///
/// Supports both comma-separated and whitespace-separated inputs, e.g.
/// - `--fields title,summary,date`
/// - `--fields title summary date`
fn normalize_fields(fields: &[String]) -> Vec<String> {
    fields
        .iter()
        .flat_map(|field| field.split(','))
        .flat_map(|field| field.split_whitespace())
        .map(str::trim)
        .filter(|field| !field.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::query::types::QueryMeta;
    use serde_json::json;

    fn query_result(meta: JsonValue) -> QueryResult {
        QueryResult {
            pages: vec![PageQueryResult {
                path: "content/post.typ".to_string(),
                permalink: "/post/".to_string(),
                meta: QueryMeta::Raw(meta),
            }],
        }
    }

    #[test]
    fn requested_url_field_is_not_a_permalink_alias() {
        let result = query_result(json!({ "title": "Post" }));
        let fields = vec!["url".to_string()];

        let output = filter_fields(&result, &fields, false);
        let page = output.as_array().unwrap()[0].as_object().unwrap();

        assert_eq!(page.get("permalink"), Some(&json!("/post/")));
        assert_eq!(page.get("url"), Some(&JsonValue::Null));
    }
}
