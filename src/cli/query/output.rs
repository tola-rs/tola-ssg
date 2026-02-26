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
        filter_fields(results, fields, args.filter_empty)
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

/// Format a single page result with path/url first
fn format_page(page: &PageQueryResult, filter_empty: bool) -> JsonValue {
    let mut obj = Map::new();

    // path and url always first
    obj.insert("path".to_string(), JsonValue::String(page.path.clone()));
    obj.insert("url".to_string(), JsonValue::String(page.url.clone()));

    // Add meta fields
    let meta_value = serde_json::to_value(&page.meta).unwrap_or_default();
    if let JsonValue::Object(meta_obj) = meta_value {
        for (key, value) in meta_obj {
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

/// Filter to specific fields, with path/url always included first
fn filter_fields(results: &QueryResult, fields: &[String], filter_empty: bool) -> JsonValue {
    let pages: Vec<JsonValue> = results
        .pages
        .iter()
        .map(|page| {
            let mut obj = Map::new();

            // path and url always first
            obj.insert("path".to_string(), JsonValue::String(page.path.clone()));
            obj.insert("url".to_string(), JsonValue::String(page.url.clone()));

            let meta_value = serde_json::to_value(&page.meta).unwrap_or_default();
            if let JsonValue::Object(meta_obj) = meta_value {
                for field in fields {
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
