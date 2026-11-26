//! Site option references accepted by fields in a module manifest.

use std::collections::BTreeMap;

use crate::options::OptionPath;

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
    Ok(bindings)
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
        let invalid = |reason| InvalidOptionRef {
            pointer: pointer.clone(),
            reason,
        };
        let raw = opt_ref["option"]
            .as_str()
            .filter(|_| opt_ref.len() == 1)
            .ok_or_else(|| invalid("write the reference as `{ option = \"key\" }`".into()))?;
        let path = OptionPath::try_from(raw.to_owned()).map_err(invalid)?;
        let key = format!("{opt_root}.{path}");
        *val = path
            .read(opts)
            .map_err(|reason| invalid(format!("could not read `{key}`: {reason}")))?
            .clone();
        bindings.insert(pointer, key);
    }
    Ok(())
}
