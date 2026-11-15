//! Adapters for site configuration values owned by other crates.

use tola_config::{
    FragmentContext, FragmentIssueKind, ValidateFragment, validate_value, wrong_type,
};
use tola_module::fragment::{FragmentError, FragmentErrorKind, FragmentStep};

type FragmentCheck = fn(&toml::Value, &mut FragmentContext<'_>);

pub(super) fn options(value: &toml::Value, context: &mut FragmentContext<'_>) {
    module_fields(value, context, tola_module::options::validate_fragment);
}

pub(super) fn icon_collections(value: &toml::Value, context: &mut FragmentContext<'_>) {
    let Some(table) = value.as_table() else {
        wrong_type(value, "table", context);
        return;
    };
    for (name, value) in table {
        context.key(name, |context| {
            module_fields(
                value,
                context,
                tola_module::icons::IconCollectionSource::validate_fragment,
            )
        });
    }
}

pub(super) fn modules(value: &toml::Value, context: &mut FragmentContext<'_>) {
    let Some(table) = value.as_table() else {
        wrong_type(value, "table", context);
        return;
    };
    for (key, value) in table {
        context.key(key, |context| {
            module_fields(
                value,
                context,
                tola_module::ModuleDeclaration::validate_fragment,
            );
        });
    }
}

pub(super) fn asset_trees(value: &toml::Value, context: &mut FragmentContext<'_>) {
    array(value, context, |value, context| {
        module_fields(
            value,
            context,
            tola_module::assets::AssetTreeDeclaration::validate_fragment,
        );
    });
}

pub(super) fn asset_files(value: &toml::Value, context: &mut FragmentContext<'_>) {
    array(value, context, |value, context| {
        module_fields(
            value,
            context,
            tola_module::assets::AssetFileDeclaration::validate_fragment,
        );
    });
}

pub(super) fn rerun_on(value: &toml::Value, context: &mut FragmentContext<'_>) {
    let Some(table) = value.as_table() else {
        wrong_type(value, "table", context);
        return;
    };
    for (key, value) in table {
        context.key(key, |context| match key.as_str() {
            "include" | "exclude" => array(value, context, rerun_path),
            _ => context.issue(FragmentIssueKind::UnknownKey),
        });
    }
}

fn rerun_path(value: &toml::Value, context: &mut FragmentContext<'_>) {
    match value {
        toml::Value::String(_) => {}
        toml::Value::Table(table) => {
            for (key, value) in table {
                context.key(key, |context| match key.as_str() {
                    "project" => String::validate_fragment(value, context),
                    _ => context.issue(FragmentIssueKind::UnknownKey),
                });
            }
        }
        _ => wrong_type(value, "string or table", context),
    }
}

pub(super) fn command_outputs(value: &toml::Value, context: &mut FragmentContext<'_>) {
    array(value, context, |value, context| {
        let Some(table) = value.as_table() else {
            wrong_type(value, "table", context);
            return;
        };
        if table.contains_key("file") && table.contains_key("tree") {
            context.issue(FragmentIssueKind::InvalidValue {
                message: "use one `file` or `tree` per output".into(),
            });
        }
        for (key, value) in table {
            context.key(key, |context| match key.as_str() {
                "file" | "tree" if value.is_str() => {
                    validate_value::<tola_address::OutputPath>(value, context);
                }
                "file" | "tree" => wrong_type(value, "string", context),
                _ => context.issue(FragmentIssueKind::UnknownKey),
            });
        }
    });
}

pub(super) fn option_path(value: &toml::Value, context: &mut FragmentContext<'_>) {
    if value.is_str() {
        validate_value::<tola_module::options::OptionPath>(value, context);
    } else {
        wrong_type(value, "string", context);
    }
}

fn array(value: &toml::Value, context: &mut FragmentContext<'_>, check: FragmentCheck) {
    let Some(values) = value.as_array() else {
        wrong_type(value, "array", context);
        return;
    };
    for (index, value) in values.iter().enumerate() {
        context.index(index, |context| check(value, context));
    }
}

pub(super) fn string_or_table<T: ValidateFragment>(
    value: &toml::Value,
    context: &mut FragmentContext<'_>,
) {
    match value {
        toml::Value::String(_) => {}
        toml::Value::Table(_) => T::validate_fragment(value, context),
        _ => wrong_type(value, "string or table", context),
    }
}

fn module_fields(
    value: &toml::Value,
    context: &mut FragmentContext<'_>,
    read: fn(&toml::Value) -> Result<(), FragmentError>,
) {
    if !value.is_table() {
        wrong_type(value, "table", context);
        return;
    }
    if let Err(FragmentError { path, kind }) = read(value) {
        let kind = match kind {
            FragmentErrorKind::UnknownKey => FragmentIssueKind::UnknownKey,
            FragmentErrorKind::InvalidValue(message) => FragmentIssueKind::InvalidValue { message },
        };
        module_issue(&path, kind, context);
    }
}

fn module_issue(path: &[FragmentStep], kind: FragmentIssueKind, context: &mut FragmentContext<'_>) {
    match path.split_first() {
        Some((FragmentStep::Key(key), rest)) => {
            context.key(key, |context| module_issue(rest, kind, context));
        }
        Some((FragmentStep::Index(index), rest)) => {
            context.index(*index, |context| module_issue(rest, kind, context));
        }
        None => context.issue(kind),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SiteConfigSchema;
    use tola_config::ValueStep;

    #[test]
    fn one_icon_source_must_accept_all_written_fields() {
        let value = toml::from_str(
            r#"
[icons.collections.local]
path = "icons"

[icons.collections.remote]
url = "https://example.org/icons.json"

[icons.collections.mixed]
path = "icons"
url = "https://example.org/icons.json"
"#,
        )
        .unwrap();
        let mut issues = Vec::new();
        SiteConfigSchema::validate_fragment(&value, &mut FragmentContext::new(&mut issues, None));
        assert_eq!(issues.len(), 1);
        assert_eq!(
            issues[0].path,
            ["icons", "collections", "mixed"].map(|key| ValueStep::Key(key.into()))
        );
    }
}
