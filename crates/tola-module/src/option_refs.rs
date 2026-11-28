//! Site option references accepted by fields in a module manifest.

use std::collections::BTreeMap;

use crate::options::OptionPath;

#[derive(Debug)]
pub(crate) struct InvalidOptionRef {
    pub(crate) pointer: String,
    pub(crate) reason: String,
}

const HOOK_FIELDS: &[(&str, &[&str])] = &[
    (
        "before-build",
        &["dev", "order", "rerun-on", "args", "generates"],
    ),
    (
        "generate-outputs",
        &["dev", "order", "rerun-on", "args", "outputs"],
    ),
    ("after-publish", &["dev", "order", "rerun-on", "args"]),
];

const ASSET_FIELDS: &[(&str, &[&str])] = &[
    ("files", &["minify"]),
    ("trees", &["include", "exclude", "minify"]),
];

/// Substitute `{ option = "..." }` fields before typed manifest declarations are read.
pub(crate) fn bind(
    manifest: &mut toml::Table,
    opts: &toml::Table,
    opt_root: &str,
) -> Result<BTreeMap<String, String>, InvalidOptionRef> {
    let mut bindings = BTreeMap::new();
    for (section, kinds) in [("hooks", HOOK_FIELDS), ("assets", ASSET_FIELDS)] {
        let Some(tbl) = manifest
            .get_mut(section)
            .and_then(toml::Value::as_table_mut)
        else {
            continue;
        };
        for &(kind, fields) in kinds {
            let Some(entries) = tbl.get_mut(kind).and_then(toml::Value::as_array_mut) else {
                continue;
            };
            for (idx, entry) in entries.iter_mut().enumerate() {
                if let Some(entry_tbl) = entry.as_table_mut() {
                    bind_fields(
                        entry_tbl,
                        &format!("/{section}/{kind}/{idx}"),
                        fields,
                        opts,
                        opt_root,
                        &mut bindings,
                    )?;
                }
            }
        }
    }
    if let Some(cmds) = manifest
        .get_mut("commands")
        .and_then(toml::Value::as_table_mut)
    {
        for (name, cmd) in cmds {
            if let Some(tbl) = cmd.as_table_mut() {
                let mut prefix = "/commands".to_owned();
                tola_diagnostic::push_pointer(&mut prefix, name);
                bind_fields(
                    tbl,
                    &prefix,
                    &["args", "projection"],
                    opts,
                    opt_root,
                    &mut bindings,
                )?;
            }
        }
    }
    if let Some(mods) = manifest
        .get_mut("modules")
        .and_then(toml::Value::as_table_mut)
    {
        for (name, child) in mods {
            let Some(tbl) = child.as_table_mut() else {
                continue;
            };
            let Some(options_val) = tbl.get_mut("options") else {
                continue;
            };
            let mut prefix = "/modules".to_owned();
            tola_diagnostic::push_pointer(&mut prefix, name);
            let options_ptr = format!("{prefix}/options");
            if let Some(opt_ref) = options_val.as_table().filter(|t| t.contains_key("option")) {
                let resolved = resolve_ref(opt_ref, &options_ptr, opts, opt_root, &mut bindings)?;
                if !resolved.is_table() {
                    return Err(InvalidOptionRef {
                        pointer: options_ptr,
                        reason: format!("referenced option for `{name}.options` must be a table"),
                    });
                }
                *options_val = resolved;
            } else {
                bind_table_values(options_val, &options_ptr, opts, opt_root, &mut bindings)?;
            }
        }
    }
    Ok(bindings)
}

fn resolve_ref(
    opt_ref: &toml::Table,
    pointer: &str,
    opts: &toml::Table,
    opt_root: &str,
    bindings: &mut BTreeMap<String, String>,
) -> Result<toml::Value, InvalidOptionRef> {
    let invalid = |reason| InvalidOptionRef {
        pointer: pointer.to_owned(),
        reason,
    };
    let raw = opt_ref["option"]
        .as_str()
        .filter(|_| opt_ref.len() == 1)
        .ok_or_else(|| invalid("write the reference as `{ option = \"key\" }`".into()))?;
    let path = OptionPath::try_from(raw.to_owned()).map_err(invalid)?;
    let key = format!("{opt_root}.{path}");
    let resolved = path
        .read(opts)
        .map_err(|reason| invalid(format!("could not read `{key}`: {reason}")))?
        .clone();
    bindings.insert(pointer.to_owned(), key);
    Ok(resolved)
}

fn bind_table_values(
    value: &mut toml::Value,
    pointer: &str,
    opts: &toml::Table,
    opt_root: &str,
    bindings: &mut BTreeMap<String, String>,
) -> Result<(), InvalidOptionRef> {
    match value {
        toml::Value::Table(tbl) => {
            if tbl.contains_key("option") {
                *value = resolve_ref(tbl, pointer, opts, opt_root, bindings)?;
            } else {
                for (key, item) in tbl.iter_mut() {
                    let mut next = pointer.to_owned();
                    tola_diagnostic::push_pointer(&mut next, key);
                    bind_table_values(item, &next, opts, opt_root, bindings)?;
                }
            }
        }
        toml::Value::Array(items) => {
            for (idx, item) in items.iter_mut().enumerate() {
                let next = format!("{pointer}/{idx}");
                bind_table_values(item, &next, opts, opt_root, bindings)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn bind_fields(
    tbl: &mut toml::Table,
    prefix: &str,
    fields: &[&str],
    opts: &toml::Table,
    opt_root: &str,
    bindings: &mut BTreeMap<String, String>,
) -> Result<(), InvalidOptionRef> {
    for &field in fields {
        let Some(val) = tbl.get_mut(field) else {
            continue;
        };
        let Some(opt_ref) = val.as_table().filter(|t| t.contains_key("option")) else {
            continue;
        };
        let pointer = format!("{prefix}/{field}");
        *val = resolve_ref(opt_ref, &pointer, opts, opt_root, bindings)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::bind;

    #[test]
    fn child_module_options_bind_table_and_nested_fields() {
        let mut tbl: toml::Table = toml::from_str(
            r#"
[modules.one]
path = "tailwind"
options = { option = "tailwind" }

[modules.two]
path = "tailwind"
[modules.two.options]
minify = { option = "css.minify" }
nested = [{ option = "css.entry" }]
"#,
        )
        .unwrap();
        let opts: toml::Table = toml::from_str(
            r#"
tailwind = { input = "styles/site.css", minify = true }
[css]
minify = false
entry = "a.css"
"#,
        )
        .unwrap();
        bind(&mut tbl, &opts, "modules.docs.options").unwrap();
        let mods = tbl["modules"].as_table().unwrap();
        assert_eq!(
            mods["one"]["options"]["input"].as_str(),
            Some("styles/site.css")
        );
        assert_eq!(mods["one"]["options"]["minify"].as_bool(), Some(true));
        assert_eq!(mods["two"]["options"]["minify"].as_bool(), Some(false));
        assert_eq!(mods["two"]["options"]["nested"][0].as_str(), Some("a.css"));
    }
}
