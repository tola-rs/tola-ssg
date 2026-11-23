//! Site option references accepted by fields in a module manifest.

use std::collections::BTreeMap;

use crate::options::OptionPath;

pub(crate) struct InvalidOptionReference {
    pub(crate) field_pointer: String,
    pub(crate) reason: String,
}

/// Substitute whole fields before the manifest's typed declarations are read.
pub(crate) fn bind(
    manifest: &mut toml::Table,
    options: &toml::Table,
    option_root: &str,
) -> Result<BTreeMap<String, String>, InvalidOptionReference> {
    let mut bindings = BTreeMap::new();
    let mut bind_fields = |table: &mut toml::Table, prefix: &str, fields: &[&str]| {
        for &field in fields {
            let Some(value) = table.get_mut(field) else {
                continue;
            };
            let Some(reference) = value
                .as_table()
                .filter(|table| table.contains_key("option"))
            else {
                continue;
            };
            let pointer = format!("{prefix}/{field}");
            let invalid = |reason| InvalidOptionReference {
                field_pointer: pointer.clone(),
                reason,
            };
            let path = reference["option"]
                .as_str()
                .filter(|_| reference.len() == 1)
                .ok_or_else(|| invalid("write the reference as `{ option = \"key\" }`".into()))?;
            let path = OptionPath::try_from(path.to_owned()).map_err(invalid)?;
            let key = format!("{option_root}.{path}");
            *value = path
                .read(options)
                .map_err(|reason| invalid(format!("could not read `{key}`: {reason}")))?
                .clone();
            bindings.insert(pointer, key);
        }
        Ok(())
    };

    for (section, declarations) in [
        (
            "hooks",
            &[
                (
                    "before-build",
                    &["dev", "order", "rerun-on", "args", "generates"][..],
                ),
                (
                    "generate-outputs",
                    &["dev", "order", "rerun-on", "args", "outputs"],
                ),
                ("after-publish", &["dev", "order", "rerun-on", "args"]),
            ][..],
        ),
        (
            "assets",
            &[
                ("files", &["minify"][..]),
                ("trees", &["include", "exclude", "minify"]),
            ][..],
        ),
    ] {
        let Some(section_table) = manifest
            .get_mut(section)
            .and_then(toml::Value::as_table_mut)
        else {
            continue;
        };
        for &(kind, fields) in declarations {
            let Some(entries) = section_table
                .get_mut(kind)
                .and_then(toml::Value::as_array_mut)
            else {
                continue;
            };
            for (index, entry) in entries.iter_mut().enumerate() {
                if let Some(table) = entry.as_table_mut() {
                    bind_fields(table, &format!("/{section}/{kind}/{index}"), fields)?;
                }
            }
        }
    }
    if let Some(commands) = manifest
        .get_mut("commands")
        .and_then(toml::Value::as_table_mut)
    {
        for (name, command) in commands {
            if let Some(table) = command.as_table_mut() {
                let mut prefix = "/commands".to_owned();
                tola_diagnostic::push_pointer(&mut prefix, name);
                bind_fields(table, &prefix, &["args", "projection"])?;
            }
        }
    }
    Ok(bindings)
}
