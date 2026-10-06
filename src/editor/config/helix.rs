//! Helix language-server configuration in TOML.

use std::str::FromStr;

use anyhow::{Context, Result};
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value};

use super::EditorDirectory;

pub(super) fn merge(source: Option<&str>, directory: &EditorDirectory) -> Result<String> {
    let mut document = DocumentMut::from_str(source.unwrap_or("")).context(
        "`.helix/languages.toml` is not valid TOML; fix the syntax, then rerun `tola editor setup`",
    )?;
    let command = directory.server_command();
    let server = ensure_table(&mut document, &["language-server", "tola-lsp"])?;
    // A user-selected executable is authoritative; only default an absent command.
    if !server.contains_key("command") {
        server.insert("command", toml_edit::value(command.command));
    }
    let mut arguments = Array::new();
    for argument in &command.arguments {
        arguments.push(argument.as_str());
    }
    set_if_absent_or_equal(
        server,
        "language-server.tola-lsp",
        "args",
        Value::Array(arguments),
    )?;
    let tola = ensure_table(&mut document, &["language-server", "tola-lsp", "config"])?;
    set_if_absent_or_equal(
        tola,
        "language-server.tola-lsp.config",
        "packageSourceDirectory",
        Value::from(
            directory
                .root()
                .join(crate::editor::GENERATED_PACKAGE_DIRECTORY)
                .to_str()
                .context("editor package path is not UTF-8")?,
        ),
    )?;
    merge_language_servers(&mut document)?;
    Ok(crate::writes::with_final_newline(document.to_string()))
}

fn merge_language_servers(document: &mut DocumentMut) -> Result<()> {
    let servers = language_servers(document, "typst")?;
    if servers
        .iter()
        .filter(|value| server_name(value).as_deref() == Some("tola-lsp"))
        .count()
        > 1
    {
        anyhow::bail!(
            "`.helix/languages.toml` registers `tola-lsp` for Typst more than once; keep one entry, then rerun `tola editor setup`"
        );
    }
    // Tola answers the site's own sources and the ordinary Typst beside them, so it claims every
    // feature it serves, for every release: Helix merges supporting servers from 25.07 on and asks
    // the first one before that.
    let mut tola = InlineTable::new();
    tola.insert("name", Value::from("tola-lsp"));
    let mut features = Array::new();
    // Every feature Helix can ask for that Tola advertises for Typst, in Helix's own spelling.
    // Folding, selection ranges, code lenses, and semantic tokens have no Helix feature: Helix
    // reads those from its syntax tree, and asking a language server for them is not on offer.
    // Colors are the exception: Helix asks for `documentColor`, and Tola answers it.
    for feature in [
        "code-action",
        "completion",
        "diagnostics",
        "document-colors",
        "document-highlight",
        "document-symbols",
        "format",
        "goto-definition",
        "goto-reference",
        "hover",
        "inlay-hints",
        "rename-symbol",
        "signature-help",
        "workspace-symbols",
    ] {
        features.push(feature);
    }
    tola.insert("only-features", Value::Array(features));
    let expected = Value::InlineTable(tola);
    let existing = servers
        .iter()
        .position(|server| server_name(server).as_deref() == Some("tola-lsp"));
    match existing {
        Some(index) => {
            if !same_value(servers.get(index).expect("located Tola service"), &expected) {
                return Err(super::setting_conflict(
                    "Helix",
                    "language.typst.language-servers",
                    servers.to_string(),
                    format!("[{expected}, ...]"),
                ));
            }
        }
        None => servers.insert(0, expected),
    }
    // The site's configuration answers Tola's own keys: which keys exist, what each one means, and
    // which values it accepts. A TOML server the author also runs keeps every other file, because
    // these answers are the schema's and no other server has it.
    let servers = language_servers(document, "toml")?;
    let expected_toml = Value::InlineTable({
        let mut entry = InlineTable::new();
        entry.insert("name", Value::from("tola-lsp"));
        let mut features = Array::new();
        for feature in ["completion", "diagnostics", "hover"] {
            features.push(feature);
        }
        entry.insert("only-features", Value::Array(features));
        entry
    });
    if servers
        .iter()
        .filter(|server| server_name(server).as_deref() == Some("tola-lsp"))
        .count()
        > 1
    {
        anyhow::bail!(
            "`.helix/languages.toml` registers `tola-lsp` for `toml` more than once; keep one entry, then rerun `tola editor setup`"
        );
    }
    let toml_index = servers
        .iter()
        .position(|server| server_name(server).as_deref() == Some("tola-lsp"));
    match toml_index {
        Some(index) => {
            if !same_value(
                servers.get(index).expect("located Tola service"),
                &expected_toml,
            ) {
                return Err(super::setting_conflict(
                    "Helix",
                    "language.toml.language-servers",
                    servers.to_string(),
                    expected_toml.to_string(),
                ));
            }
        }
        None => servers.push(expected_toml),
    }
    Ok(())
}

/// The `language-servers` array of the one `[[language]]` table named `language`, created when
/// the document has none.
fn language_servers<'a>(document: &'a mut DocumentMut, language: &str) -> Result<&'a mut Array> {
    if !document.contains_key("language") {
        document.insert("language", Item::ArrayOfTables(ArrayOfTables::new()));
    }
    let languages = document
        .get_mut("language")
        .and_then(Item::as_array_of_tables_mut)
        .context(
            "the `language` key in `.helix/languages.toml` must hold `[[language]]` tables; fix it, then rerun `tola editor setup`",
        )?;
    let indices = languages
        .iter()
        .enumerate()
        .filter_map(|(index, table)| {
            (table.get("name").and_then(Item::as_str) == Some(language)).then_some(index)
        })
        .collect::<Vec<_>>();
    if indices.len() > 1 {
        anyhow::bail!(
            "`.helix/languages.toml` has more than one `[[language]]` table named `{language}`; keep one, then rerun `tola editor setup`"
        );
    }
    let index = match indices.first() {
        Some(index) => *index,
        None => {
            let mut table = Table::new();
            table.insert("name", toml_edit::value(language));
            languages.push(table);
            languages.len() - 1
        }
    };
    let table = languages.get_mut(index).expect("the language table exists");
    if !table.contains_key("language-servers") {
        table.insert("language-servers", Item::Value(Value::Array(Array::new())));
    }
    table
        .get_mut("language-servers")
        .and_then(Item::as_array_mut)
        .with_context(|| {
            format!(
                "the `language-servers` key for `{language}` in `.helix/languages.toml` must be an array; fix it, then rerun `tola editor setup`"
            )
        })
}

/// The name of a `language-servers` entry, which is either a bare string or a named table.
fn server_name(value: &Value) -> Option<String> {
    value
        .as_str()
        .or_else(|| value.as_inline_table()?.get("name")?.as_str())
        .map(str::to_owned)
}

fn ensure_table<'a>(document: &'a mut DocumentMut, path: &[&str]) -> Result<&'a mut Table> {
    let mut table = document.as_table_mut();
    for (index, segment) in path.iter().enumerate() {
        if !table.contains_key(segment) {
            table.insert(segment, Item::Table(Table::new()));
        }
        table = table
            .get_mut(segment)
            .and_then(Item::as_table_mut)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "the `{}` key in `.helix/languages.toml` must be a TOML table; fix it, then rerun `tola editor setup`",
                    path[..=index].join(".")
                )
            })?;
    }
    Ok(table)
}

fn set_if_absent_or_equal(
    table: &mut Table,
    section: &str,
    key: &str,
    expected: Value,
) -> Result<()> {
    match table.get(key) {
        None => {
            table.insert(key, Item::Value(expected));
            Ok(())
        }
        Some(Item::Value(actual)) if same_value(actual, &expected) => Ok(()),
        Some(actual) => Err(super::setting_conflict(
            "Helix",
            &format!("{section}.{key}"),
            actual.to_string(),
            expected.to_string(),
        )),
    }
}

fn same_value(actual: &Value, expected: &Value) -> bool {
    if let (Some(actual), Some(expected)) = (actual.as_bool(), expected.as_bool()) {
        return actual == expected;
    }
    if let (Some(actual), Some(expected)) = (actual.as_str(), expected.as_str()) {
        return actual == expected;
    }
    if let (Some(actual), Some(expected)) = (actual.as_array(), expected.as_array()) {
        return actual.len() == expected.len()
            && actual
                .iter()
                .zip(expected.iter())
                .all(|(actual, expected)| same_value(actual, expected));
    }
    if let (Some(actual), Some(expected)) = (actual.as_inline_table(), expected.as_inline_table()) {
        return actual.len() == expected.len()
            && expected.iter().all(|(key, expected)| {
                actual
                    .get(key)
                    .is_some_and(|actual| same_value(actual, expected))
            });
    }
    false
}

#[cfg(test)]
mod tests {
    use super::super::tests::editor_directory;
    use super::*;

    #[test]
    fn merge_keeps_unrelated_tables() {
        let source = "# keep\n[language-server.other]\ncommand = \"other\"\n";
        let merged = merge(Some(source), &editor_directory(".")).unwrap();
        assert!(merged.contains("# keep"));
        assert!(merged.contains("[language-server.other]"));
    }

    #[test]
    fn conflicting_settings_report_both_values() {
        let source = "[language-server.tola-lsp.config]\npackageSourceDirectory = \"elsewhere\"\n";
        let error = merge(Some(source), &editor_directory(".")).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("elsewhere"), "{message}");
        assert!(message.contains(".tola/builtin-packages"), "{message}");
    }

    #[test]
    fn merge_preserves_author_servers() {
        let source = "# keep\n[language-server.other-typst]\ncommand = '/custom/other-typst'\nargs = ['lsp']\n[[language]]\nname = 'rust'\nlanguage-servers = ['rust-analyzer']\n[[language]]\nname = 'typst'\nlanguage-servers = ['other-typst'] # assistance\n";
        let merged = merge(Some(source), &editor_directory(".")).unwrap();
        let value: toml::Value = toml::from_str(&merged).unwrap();
        assert_eq!(
            value["language-server"]["other-typst"]["command"].as_str(),
            Some("/custom/other-typst")
        );
        assert_eq!(
            value["language-server"]["tola-lsp"]["command"].as_str(),
            Some("tola")
        );
        assert_eq!(
            value["language"][0]["language-servers"][0].as_str(),
            Some("rust-analyzer")
        );
        let servers = value["language"][1]["language-servers"].as_array().unwrap();
        assert_eq!(servers[0]["name"].as_str(), Some("tola-lsp"));
        assert_eq!(servers[1].as_str(), Some("other-typst"));
        assert!(merged.contains("# assistance"));
    }

    /// Tola claims every feature it serves, so an editor that merges supporting servers asks it for
    /// each answer it can give and no answer it cannot.
    #[test]
    fn tola_claims_every_feature_it_serves() {
        let merged = merge(None, &editor_directory(".")).unwrap();
        let value: toml::Value = toml::from_str(&merged).unwrap();
        let servers = value["language"][0]["language-servers"].as_array().unwrap();
        assert_eq!(servers.len(), 1, "{servers:?}");
        assert_eq!(servers[0]["name"].as_str(), Some("tola-lsp"));
        assert_eq!(
            servers[0]["only-features"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|feature| feature.as_str())
                .collect::<Vec<_>>(),
            [
                "code-action",
                "completion",
                "diagnostics",
                "document-colors",
                "document-highlight",
                "document-symbols",
                "format",
                "goto-definition",
                "goto-reference",
                "hover",
                "inlay-hints",
                "rename-symbol",
                "signature-help",
                "workspace-symbols",
            ]
        );
    }

    /// The site's configuration answers Tola's own keys, and merging it twice changes nothing.
    #[test]
    fn toml_entry_claims_its_answers() {
        let merged = merge(None, &editor_directory(".")).unwrap();
        let value: toml::Value = toml::from_str(&merged).unwrap();
        let toml = value["language"]
            .as_array()
            .unwrap()
            .iter()
            .find(|language| language["name"].as_str() == Some("toml"))
            .expect("a `toml` language table");
        let servers = toml["language-servers"].as_array().unwrap();

        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0]["name"].as_str(), Some("tola-lsp"));
        assert_eq!(
            servers[0]["only-features"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|feature| feature.as_str())
                .collect::<Vec<_>>(),
            ["completion", "diagnostics", "hover"]
        );
        assert_eq!(
            merge(Some(&merged), &editor_directory(".")).unwrap(),
            merged
        );
    }
}
