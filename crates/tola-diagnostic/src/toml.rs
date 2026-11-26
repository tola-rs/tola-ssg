//! Messages about the TOML an author wrote.

use std::borrow::Cow;

/// A next step when no likely spelling is known.
pub const UNKNOWN_FIELD_HELP: &str =
    "Check the field name, or remove the field if this version of Tola does not support it";

pub fn field_replacement<'a>(path: &str, fields: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let (tbl, written) = path.rsplit_once('.').unwrap_or(("", path));
    tola_typst::spelling::closest(written, fields.filter_map(|f| immediate_key(f, tbl)))
}

pub fn immediate_key<'a>(path: &'a str, tbl_path: &str) -> Option<&'a str> {
    let rem = if tbl_path.is_empty() {
        (!path.contains('.')).then_some(path)?
    } else {
        path.strip_prefix(tbl_path)?.strip_prefix('.')?
    };
    (!rem.contains('.')).then_some(rem)
}

pub struct TomlMessage<'a> {
    pub message: Cow<'a, str>,
    pub help: Option<Cow<'a, str>>,
}

impl<'a> TomlMessage<'a> {
    fn new(message: Cow<'a, str>) -> Self {
        Self {
            message,
            help: None,
        }
    }

    fn with_help(mut self, help: impl Into<Cow<'a, str>>) -> Self {
        self.help = Some(help.into());
        self
    }

    pub fn from_error(msg: &'a str) -> Self {
        let msg = msg.trim();
        if let Some(rest) = msg
            .strip_prefix("invalid type: ")
            .or_else(|| msg.strip_prefix("invalid value: "))
            && let Some((found, exp)) = rest.split_once(", expected ")
        {
            return Self::new(Cow::Owned(format!(
                "expected {}, found {}",
                accepted_form(exp),
                written_value(found)
            )));
        }
        if let Some(rest) = msg.strip_prefix("unknown variant `")
            && let Some((var, acc)) = rest.split_once("`, expected ")
        {
            return Self::new(Cow::Owned(format!("`{var}` is not a supported value")))
                .with_help(format!("Choose {acc}"));
        }
        if let Some(rest) = msg.strip_prefix("unknown field `")
            && let Some((field, acc)) = rest.split_once("`, expected ")
        {
            let candidates = acc.split('`').skip(1).step_by(2);
            let help = tola_typst::spelling::closest(field, candidates)
                .map_or(Cow::Borrowed(UNKNOWN_FIELD_HELP), |k| {
                    Cow::Owned(format!("Check whether you meant `{k}`"))
                });
            return Self::new(Cow::Owned(format!("unknown field `{field}`"))).with_help(help);
        }
        if let Some(field) = msg.strip_prefix("missing field `")
            && let Some(field) = field.strip_suffix('`')
        {
            return Self::new(Cow::Owned(format!(
                "the required field `{field}` is missing"
            )))
            .with_help(format!("Add a value for `{field}` in this table"));
        }
        if let Some(rest) = msg.strip_prefix("unexpected keys in table: ")
            && let Some((unknown, allowed)) = rest.split_once(", available keys: ")
        {
            return Self::new(Cow::Owned(format!("unknown fields {unknown}")))
                .with_help(format!("Use a supported field from {allowed}"));
        }
        if let Some(found) = msg.strip_prefix("expected table, found ") {
            return Self::new(Cow::Owned(format!(
                "expected a table, found {}",
                written_value(found)
            )));
        }
        if msg.starts_with("data did not match any variant of untagged enum ") {
            return Self::new(Cow::Borrowed(
                "this value does not match any supported form",
            ));
        }
        if msg == "expected table with exactly 1 entry, found empty table" {
            return Self::new(Cow::Borrowed("this table must contain exactly one entry"));
        }
        if let Some(rest) = msg.strip_prefix("expected table key `")
            && let Some((k, found)) = rest.split_once("`, but was `")
        {
            return Self::new(Cow::Owned(format!(
                "expected the table key `{k}`, found `{}`",
                found.trim_end_matches('`')
            )));
        }
        if msg.starts_with("missing value for key") {
            return Self::new(Cow::Borrowed("this key needs an `=` and a value"))
                .with_help("Write the assignment as `key = value`");
        }
        if msg.starts_with("missing value") {
            return Self::new(Cow::Borrowed("this key is missing a value"))
                .with_help("Add a value after `=`");
        }
        if msg.starts_with("unclosed array table") {
            return Self::new(Cow::Borrowed(
                "this table header is missing its closing brackets",
            ))
            .with_help("Close the header with `]]`");
        }
        if msg.starts_with("unclosed table") {
            return Self::new(Cow::Borrowed(
                "this table header is missing its closing bracket",
            ))
            .with_help("Close the header with `]`");
        }
        if let Some((reason, exp)) = msg.split_once('\n')
            && let Some(exp) = exp.strip_prefix("expected ")
        {
            return Self::new(Cow::Borrowed(reason))
                .with_help(format!("Use {}", super::collapse_whitespace(exp)));
        }
        if msg.contains('\n') {
            return Self::new(Cow::Owned(super::collapse_whitespace(msg)));
        }
        Self::new(Cow::Borrowed(msg))
    }
}

fn accepted_form(exp: &str) -> Cow<'_, str> {
    match exp {
        "u8" => Cow::Owned(format!("an integer from {} to {}", u8::MIN, u8::MAX)),
        "u16" => Cow::Owned(format!("an integer from {} to {}", u16::MIN, u16::MAX)),
        "u32" => Cow::Owned(format!("an integer from {} to {}", u32::MIN, u32::MAX)),
        "u64" => Cow::Owned(format!("an integer from {} to {}", u64::MIN, u64::MAX)),
        "u128" => Cow::Owned(format!("an integer from {} to {}", u128::MIN, u128::MAX)),
        "usize" => Cow::Owned(format!("an integer from {} to {}", usize::MIN, usize::MAX)),
        "i8" => Cow::Owned(format!("an integer from {} to {}", i8::MIN, i8::MAX)),
        "i16" => Cow::Owned(format!("an integer from {} to {}", i16::MIN, i16::MAX)),
        "i32" => Cow::Owned(format!("an integer from {} to {}", i32::MIN, i32::MAX)),
        "i64" => Cow::Owned(format!("an integer from {} to {}", i64::MIN, i64::MAX)),
        "i128" => Cow::Owned(format!("an integer from {} to {}", i128::MIN, i128::MAX)),
        "isize" => Cow::Owned(format!("an integer from {} to {}", isize::MIN, isize::MAX)),
        "f32" | "f64" => Cow::Borrowed("a number"),
        "bool" | "a boolean" => Cow::Borrowed("true or false"),
        "str" | "string" | "a string" | "char" | "a character" => Cow::Borrowed("a string"),
        "path string" => Cow::Borrowed("a path"),
        "a sequence" => Cow::Borrowed("an array"),
        "a map" => Cow::Borrowed("a table"),
        // An enum expectation lists the accepted values as quoted keys.
        exp if exp.contains('`') => Cow::Borrowed(exp),
        exp if exp.starts_with("struct ") || exp.starts_with("tuple struct ") => {
            Cow::Borrowed("a table")
        }
        exp if exp.starts_with("a tuple") => Cow::Borrowed("an array"),
        _ => Cow::Borrowed("a supported value"),
    }
}

fn written_value(found: &str) -> Cow<'_, str> {
    match found {
        "array" | "sequence" => return Cow::Borrowed("an array"),
        "map" | "table" | "inline table" => return Cow::Borrowed("a table"),
        "datetime" => return Cow::Borrowed("a date"),
        "unit value" => return Cow::Borrowed("an empty value"),
        _ => {}
    }
    let (kind, val) = found
        .strip_prefix("floating point ")
        .map(|v| ("floating point", v))
        .or_else(|| found.split_once(' '))
        .unwrap_or(("", found));
    let named = match kind {
        "string" => "the string",
        "character" => "the character",
        "boolean" => "the boolean",
        "integer" | "floating point" => "the number",
        _ => return Cow::Borrowed("a value"),
    };
    Cow::Owned(format!("{named} `{}`", val.trim_matches('`')))
}
