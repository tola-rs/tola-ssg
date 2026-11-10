//! Aligned human-readable output for JSON values.
//!
//! The default CLI representation is text for people: a table for record rows, key/value
//! lines for one record, and the scalar itself otherwise. Machine output uses `--json`.

use serde_json::Value;
use unicode_width::UnicodeWidthStr;

const CELL_LIMIT: usize = 80;

pub(crate) fn render(value: &Value) -> String {
    match value {
        Value::Array(rows) if rows.is_empty() => "No rows".to_owned(),
        Value::Array(rows) => rows_for(rows),
        Value::Object(object) if object.is_empty() => "No fields".to_owned(),
        Value::Object(object) => object_for(object),
        scalar => scalar_cell(scalar),
    }
}

fn rows_for(rows: &[Value]) -> String {
    if !rows.iter().all(Value::is_object) {
        return rows.iter().map(scalar_cell).collect::<Vec<_>>().join("\n");
    }

    let mut columns: Vec<String> = Vec::new();
    for row in rows {
        for key in row.as_object().expect("checked above").keys() {
            if !columns.iter().any(|column| column == key) {
                columns.push(key.clone());
            }
        }
    }
    if columns.is_empty() {
        return "No rows".to_owned();
    }

    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            let object = row.as_object().expect("checked above");
            columns
                .iter()
                .map(|column| object.get(column).map(cell).unwrap_or_default())
                .collect()
        })
        .collect();

    let mut widths: Vec<usize> = columns.iter().map(|column| column.width()).collect();
    for row in &cells {
        for (index, value) in row.iter().enumerate() {
            widths[index] = widths[index].max(value.width());
        }
    }

    let mut out = String::new();
    out.push_str(&line(columns.iter().map(String::as_str), &widths));
    for row in &cells {
        out.push('\n');
        out.push_str(&line(row.iter().map(String::as_str), &widths));
    }
    out
}

fn object_for(object: &serde_json::Map<String, Value>) -> String {
    let width = object.keys().map(|key| key.width()).max().unwrap_or(0);
    object
        .iter()
        .map(|(key, value)| format!("{}  {}", pad(key, width), cell(value)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn line<'a>(cells: impl Iterator<Item = &'a str>, widths: &[usize]) -> String {
    cells
        .enumerate()
        .map(|(index, cell)| {
            if index + 1 == widths.len() {
                cell.to_owned()
            } else {
                pad(cell, widths[index])
            }
        })
        .collect::<Vec<_>>()
        .join("  ")
}

fn pad(text: &str, width: usize) -> String {
    let padding = width.saturating_sub(text.width());
    format!("{text}{}", " ".repeat(padding))
}

fn cell(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => escape(value),
        other => truncate(&serde_json::to_string(other).unwrap_or_else(|_| "?".to_owned())),
    }
}

fn scalar_cell(value: &Value) -> String {
    match value {
        Value::String(value) => escape(value),
        other => cell(other),
    }
}

fn escape(value: &str) -> String {
    value.replace('\n', "\\n").replace('\r', "\\r")
}

fn truncate(value: &str) -> String {
    if value.width() <= CELL_LIMIT {
        return value.to_owned();
    }
    let mut out = String::new();
    let mut width = 0;
    for character in value.chars() {
        let next = character.to_string();
        let character_width = next.width();
        if width + character_width > CELL_LIMIT.saturating_sub(1) {
            break;
        }
        width += character_width;
        out.push(character);
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::render;
    use serde_json::json;

    #[test]
    fn record_rows_share_a_header() {
        let text = render(&json!([
            {"id": "a", "label": "Alpha"},
            {"id": "bb", "label": "Beta"},
        ]));
        assert!(text.starts_with("id  label"));
        assert!(text.contains("a   Alpha"));
    }

    #[test]
    fn one_record_is_key_value() {
        let text = render(&json!({"id": "a", "label": "Alpha"}));
        assert_eq!(text, "id     a\nlabel  Alpha");
    }

    #[test]
    fn empty_rows_are_named() {
        assert_eq!(render(&json!([])), "No rows");
    }
}
