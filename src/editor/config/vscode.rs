//! VS Code settings with preservation of existing JSONC formatting.
//!
//! Merge keeps the author's comments, spacing, and key order, so it locates each top-level
//! property's text in the original document instead of reserializing a parsed value. No JSONC
//! parser is available to do that here — `serde_json` rejects comments and `toml_edit` is TOML —
//! so the scanner below owns the JSONC syntax `serde_json` does not read.

use anyhow::{Context, Result, bail};
use serde_json::Value;

use super::EditorDirectory;

pub(super) fn merge(source: Option<&str>, directory: &EditorDirectory) -> Result<String> {
    let Some(source) = source else {
        let mut settings = serde_json::to_string_pretty(&expected(directory))
            .expect("editor settings are serializable");
        settings.push('\n');
        return Ok(settings);
    };
    let object = scan_object(source)
        .context("`.vscode/settings.json` is not valid JSONC; fix the syntax, then rerun `tola editor setup`")?;
    let expected = expected(directory);
    let expected = expected.as_object().expect("expected settings object");
    let mut missing = Vec::new();
    for (key, value) in expected {
        if let Some(property) = object
            .properties
            .iter()
            .find(|property| property.key == *key)
        {
            let raw = &source[property.value_start..property.value_end];
            let actual: Value = serde_json::from_str(&strip_comments(raw)?).with_context(|| {
                format!(
                    "the `{key}` value in `.vscode/settings.json` is not valid JSON; fix it, then rerun `tola editor setup`"
                )
            })?;
            if actual != *value {
                return Err(super::setting_conflict(
                    "VS Code",
                    key,
                    serde_json::to_string(&actual)?,
                    serde_json::to_string(value)?,
                ));
            }
        } else {
            missing.push((key, value));
        }
    }
    if missing.is_empty() {
        return Ok(crate::writes::with_final_newline(source.to_owned()));
    }

    let mut insertion = String::new();
    if !source[..object.close].ends_with('\n') {
        insertion.push('\n');
    }
    for (index, (key, value)) in missing.iter().enumerate() {
        insertion.push_str("  ");
        insertion.push_str(&serde_json::to_string(key)?);
        insertion.push_str(": ");
        let rendered = serde_json::to_string_pretty(value)?;
        insertion.push_str(&rendered.replace('\n', "\n  "));
        if index + 1 != missing.len() {
            insertion.push(',');
        }
        insertion.push('\n');
    }

    let mut prefix = source[..object.close].to_owned();
    if !object.properties.is_empty() && !object.trailing_comma {
        let end = object.properties.last().expect("property exists").value_end;
        prefix.insert(end, ',');
    }
    let adjusted_close =
        object.close + usize::from(!object.properties.is_empty() && !object.trailing_comma);
    prefix.insert_str(adjusted_close, &insertion);
    prefix.push_str(&source[object.close..]);
    Ok(crate::writes::with_final_newline(prefix))
}

fn expected(directory: &EditorDirectory) -> Value {
    let mut settings = serde_json::Map::new();
    for (key, value) in directory
        .tola_settings()
        .as_object()
        .expect("Tola settings are an object")
    {
        settings.insert(format!("tola.{key}"), value.clone());
    }
    Value::Object(settings)
}

struct JsonObject {
    properties: Vec<JsonProperty>,
    close: usize,
    trailing_comma: bool,
}

struct JsonProperty {
    key: String,
    value_start: usize,
    value_end: usize,
}

fn scan_object(source: &str) -> Result<JsonObject> {
    let bytes = source.as_bytes();
    let mut index = skip_space_and_comments(bytes, 0)?;
    if bytes.get(index) != Some(&b'{') {
        bail!("`.vscode/settings.json` is not a JSON object at the top level");
    }
    index += 1;
    let mut properties = Vec::new();
    let mut trailing_comma = false;
    loop {
        index = skip_space_and_comments(bytes, index)?;
        match bytes.get(index) {
            Some(b'}') => {
                let end = skip_space_and_comments(bytes, index + 1)?;
                if end != bytes.len() {
                    bail!("`.vscode/settings.json` has content after its top-level JSON object");
                }
                return Ok(JsonObject {
                    properties,
                    close: index,
                    trailing_comma,
                });
            }
            Some(b'"') => {}
            _ => bail!("a setting name in `.vscode/settings.json` is not quoted"),
        }
        let (key_end, key) = scan_string(source, index)?;
        if properties
            .iter()
            .any(|property: &JsonProperty| property.key == key)
        {
            bail!(
                "`.vscode/settings.json` defines `{key}` twice; keep one value, then rerun `tola editor setup`"
            );
        }
        index = skip_space_and_comments(bytes, key_end)?;
        if bytes.get(index) != Some(&b':') {
            bail!("`{key}` in `.vscode/settings.json` is missing its `:` separator");
        }
        index = skip_space_and_comments(bytes, index + 1)?;
        let value_start = index;
        let value_end = scan_value(bytes, index)?;
        properties.push(JsonProperty {
            key,
            value_start,
            value_end,
        });
        index = skip_space_and_comments(bytes, value_end)?;
        match bytes.get(index) {
            Some(b',') => {
                trailing_comma = true;
                index += 1;
            }
            Some(b'}') => trailing_comma = false,
            _ => bail!("`.vscode/settings.json` is missing a `,` or `}}` after a setting value"),
        }
    }
}

fn scan_string(source: &str, start: usize) -> Result<(usize, String)> {
    let end = scan_string_bytes(source.as_bytes(), start)?;
    Ok((end, serde_json::from_str(&source[start..end])?))
}

fn scan_string_bytes(bytes: &[u8], start: usize) -> Result<usize> {
    let mut index = start + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            b'"' => return Ok(index + 1),
            _ => index += 1,
        }
    }
    bail!("`.vscode/settings.json` has an unterminated string")
}

fn scan_value(bytes: &[u8], start: usize) -> Result<usize> {
    let mut index = start;
    let mut end = start;
    let mut nested = Vec::new();
    while index < bytes.len() {
        if bytes[index] == b'"' {
            index = scan_string_bytes(bytes, index)?;
            end = index;
            continue;
        }
        if bytes[index] == b'/'
            && let Some(comment_end) = skip_comment(bytes, index)?
        {
            index = comment_end;
            continue;
        }
        match bytes[index] {
            byte if byte.is_ascii_whitespace() => {
                index += 1;
                continue;
            }
            b'[' | b'{' => {
                nested.push(bytes[index]);
                index += 1;
            }
            b']' => {
                if nested.pop() != Some(b'[') {
                    bail!("`.vscode/settings.json` has an unbalanced `]`");
                }
                index += 1;
            }
            b'}' if nested.last() == Some(&b'{') => {
                nested.pop();
                index += 1;
            }
            b',' | b'}' if nested.is_empty() => {
                return Ok(end);
            }
            _ => index += 1,
        }
        end = index;
    }
    bail!("`.vscode/settings.json` has an unterminated JSON value")
}

fn skip_space_and_comments(bytes: &[u8], mut index: usize) -> Result<usize> {
    loop {
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            index += 1;
        }
        match skip_comment(bytes, index)? {
            Some(end) => index = end,
            None => return Ok(index),
        }
    }
}

fn skip_comment(bytes: &[u8], index: usize) -> Result<Option<usize>> {
    let (Some(&b'/'), Some(&next)) = (bytes.get(index), bytes.get(index + 1)) else {
        return Ok(None);
    };
    let mut end = index + 2;
    match next {
        b'/' => {
            while bytes
                .get(end)
                .is_some_and(|byte| !matches!(byte, b'\n' | b'\r'))
            {
                end += 1;
            }
            Ok(Some(end))
        }
        b'*' => {
            while end + 1 < bytes.len() {
                if bytes[end] == b'*' && bytes[end + 1] == b'/' {
                    return Ok(Some(end + 2));
                }
                end += 1;
            }
            bail!("`.vscode/settings.json` has an unterminated block comment")
        }
        _ => Ok(None),
    }
}

fn strip_comments(source: &str) -> Result<String> {
    let bytes = source.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'"' {
            let end = scan_string_bytes(bytes, index)?;
            output.extend_from_slice(&bytes[index..end]);
            index = end;
        } else if let Some(end) = skip_comment(bytes, index)? {
            index = end;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    Ok(String::from_utf8(output).expect("comment removal preserves UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::super::tests::{editor_directory, loaded_config};
    use super::*;
    use serde_json::json;

    #[test]
    fn merge_preserves_author_comments() {
        let directory = editor_directory(".");

        let source = "{\n  // keep\n  \"editor.formatOnSave\": true\n}\n";
        let merged = merge(Some(source), &directory).unwrap();
        assert!(merged.contains("// keep"));
        assert!(merged.contains("\"editor.formatOnSave\": true"));
        assert!(merged.contains("\"tola.configPath\""));
        assert!(scan_object(&merged).is_ok());

        let source = "{\r \"editor.custom\": {\"values\": [1, /* inside */ \"// text\"]} /* after */ // keep\r}\r";
        let merged = merge(Some(source), &directory).unwrap();
        let settings: Value = serde_json::from_str(&strip_comments(&merged).unwrap()).unwrap();
        assert_eq!(settings["editor.custom"], json!({"values": [1, "// text"]}));
        assert!(merged.contains("/* inside */"));
        assert!(merged.contains("}, /* after */ // keep\r"));

        let source = "{\n \"editor.fontSize\":14 // keep\n}\n";
        let merged = merge(Some(source), &directory).unwrap();
        let settings: Value = serde_json::from_str(&strip_comments(&merged).unwrap()).unwrap();
        assert_eq!(settings["editor.fontSize"], 14);
        assert!(merged.contains("14, // keep\n"));
        assert_eq!(merge(Some(&merged), &directory).unwrap(), merged);
    }

    #[test]
    fn fresh_settings_include_required_keys() {
        let settings: Value =
            serde_json::from_str(&merge(None, &editor_directory(".")).unwrap()).unwrap();
        assert_eq!(settings["tola.configPath"], "./tola.toml");
        assert!(settings.get("tola.serverPath").is_none());
    }

    /// A site whose configuration was selected, and the file that selected it.
    fn selected_directory() -> (tempfile::TempDir, std::path::PathBuf, EditorDirectory) {
        let root = tempfile::tempdir().unwrap();
        let config_path = root.path().join("selected config.toml");
        std::fs::write(&config_path, "").unwrap();
        let loaded = loaded_config(Some(&config_path));
        let directory = EditorDirectory::from_config(loaded.config()).unwrap();
        (root, config_path, directory)
    }

    #[test]
    fn selected_config_keeps_author_settings() {
        let (_root, config_path, directory) = selected_directory();
        let source =
            "{\n // a server the author installed\n \"other.serverPath\": \"/custom/other\"\n}";

        let merged = merge(Some(source), &directory).unwrap();
        let settings: Value = serde_json::from_str(&strip_comments(&merged).unwrap()).unwrap();

        assert_eq!(settings["other.serverPath"], "/custom/other");
        assert_eq!(settings["tola.configPath"], json!(config_path));
        assert!(merged.contains("// a server the author installed"));
    }

    #[test]
    fn conflicting_config_path_is_refused() {
        let (root, _config_path, directory) = selected_directory();
        let conflict = format!(
            "{{\"tola.configPath\": {}}}",
            json!(root.path().join("other.toml"))
        );

        assert!(merge(Some(&conflict), &directory).is_err());
    }

    #[test]
    fn duplicate_setting_is_rejected() {
        let source = "{\"editor.fontSize\": 12, \"editor.fontSize\": 14}";
        assert!(merge(Some(source), &editor_directory(".")).is_err());
    }
}
