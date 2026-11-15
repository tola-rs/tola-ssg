//! Messages about the TOML an author wrote.

use std::borrow::Cow;

/// A next step when no likely spelling is known.
pub const UNKNOWN_FIELD_HELP: &str =
    "Check the field name, or remove the field if this version of Tola does not support it";

/// The declared key in the same table that the written key nearly spells.
pub fn field_replacement<'a>(path: &str, fields: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let (table, written) = path.rsplit_once('.').unwrap_or(("", path));
    tola_typst::spelling::closest(
        written,
        fields.filter_map(|field| immediate_key(field, table)),
    )
}

/// The key `path` writes directly inside `table_path`, if it writes one at all.
pub fn immediate_key<'a>(path: &'a str, table_path: &str) -> Option<&'a str> {
    let remainder = if table_path.is_empty() {
        (!path.contains('.')).then_some(path)?
    } else {
        path.strip_prefix(table_path)?.strip_prefix('.')?
    };
    (!remainder.contains('.')).then_some(remainder)
}

/// A TOML error in terms of the keys and values the author can change.
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

    /// Present a TOML parsing or deserialization error in the author's vocabulary.
    pub fn from_error(message: &'a str) -> Self {
        let message = message.trim();
        if let Some(rest) = message
            .strip_prefix("invalid type: ")
            .or_else(|| message.strip_prefix("invalid value: "))
            && let Some((found, expected)) = rest.split_once(", expected ")
        {
            return Self::new(Cow::Owned(format!(
                "expected {}, found {}",
                accepted_form(expected),
                written_value(found)
            )));
        }
        if let Some(rest) = message.strip_prefix("unknown variant `")
            && let Some((variant, accepted)) = rest.split_once("`, expected ")
        {
            return Self::new(Cow::Owned(format!("`{variant}` is not a supported value")))
                .with_help(format!("Choose {accepted}"));
        }
        if let Some(rest) = message.strip_prefix("unknown field `")
            && let Some((field, accepted)) = rest.split_once("`, expected ")
        {
            let candidates = accepted.split('`').skip(1).step_by(2);
            let help = tola_typst::spelling::closest(field, candidates)
                .map_or(Cow::Borrowed(UNKNOWN_FIELD_HELP), |key| {
                    Cow::Owned(format!("Check whether you meant `{key}`"))
                });
            return Self::new(Cow::Owned(format!("unknown field `{field}`"))).with_help(help);
        }
        if let Some(field) = message.strip_prefix("missing field `")
            && let Some(field) = field.strip_suffix('`')
        {
            return Self::new(Cow::Owned(format!(
                "the required field `{field}` is missing"
            )))
            .with_help(format!("Add a value for `{field}` in this table"));
        }
        if let Some(rest) = message.strip_prefix("unexpected keys in table: ")
            && let Some((unknown, allowed)) = rest.split_once(", available keys: ")
        {
            return Self::new(Cow::Owned(format!("unknown fields {unknown}")))
                .with_help(format!("Use a supported field from {allowed}"));
        }
        if let Some(found) = message.strip_prefix("expected table, found ") {
            return Self::new(Cow::Owned(format!(
                "expected a table, found {}",
                written_value(found)
            )));
        }
        if message.starts_with("data did not match any variant of untagged enum ") {
            return Self::new(Cow::Borrowed(
                "this value does not match any supported form",
            ));
        }
        if message == "expected table with exactly 1 entry, found empty table" {
            return Self::new(Cow::Borrowed("this table must contain exactly one entry"));
        }
        if let Some(rest) = message.strip_prefix("expected table key `")
            && let Some((key, found)) = rest.split_once("`, but was `")
        {
            return Self::new(Cow::Owned(format!(
                "expected the table key `{key}`, found `{}`",
                found.trim_end_matches('`')
            )));
        }
        if message.starts_with("missing value for key") {
            return Self::new(Cow::Borrowed("this key needs an `=` and a value"))
                .with_help("Write the assignment as `key = value`");
        }
        if message.starts_with("missing value") {
            return Self::new(Cow::Borrowed("this key is missing a value"))
                .with_help("Add a value after `=`");
        }
        if message.starts_with("unclosed array table") {
            return Self::new(Cow::Borrowed(
                "this table header is missing its closing brackets",
            ))
            .with_help("Close the header with `]]`");
        }
        if message.starts_with("unclosed table") {
            return Self::new(Cow::Borrowed(
                "this table header is missing its closing bracket",
            ))
            .with_help("Close the header with `]`");
        }
        if let Some((reason, expected)) = message.split_once('\n')
            && let Some(expected) = expected.strip_prefix("expected ")
        {
            return Self::new(Cow::Borrowed(reason))
                .with_help(format!("Use {}", super::collapse_whitespace(expected)));
        }
        if message.contains('\n') {
            return Self::new(Cow::Owned(super::collapse_whitespace(message)));
        }
        Self::new(Cow::Borrowed(message))
    }
}

/// The TOML form one serde expectation asks for.
fn accepted_form(expected: &str) -> Cow<'_, str> {
    match expected {
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
        expected if expected.contains('`') => Cow::Borrowed(expected),
        expected if expected.starts_with("struct ") || expected.starts_with("tuple struct ") => {
            Cow::Borrowed("a table")
        }
        expected if expected.starts_with("a tuple") => Cow::Borrowed("an array"),
        _ => Cow::Borrowed("a supported value"),
    }
}

/// The TOML value an author wrote, named in TOML terms.
fn written_value(found: &str) -> Cow<'_, str> {
    match found {
        "array" | "sequence" => return Cow::Borrowed("an array"),
        "map" | "table" | "inline table" => return Cow::Borrowed("a table"),
        "datetime" => return Cow::Borrowed("a date"),
        "unit value" => return Cow::Borrowed("an empty value"),
        _ => {}
    }
    let (kind, value) = found
        .strip_prefix("floating point ")
        .map(|value| ("floating point", value))
        .or_else(|| found.split_once(' '))
        .unwrap_or(("", found));
    let named = match kind {
        "string" => "the string",
        "character" => "the character",
        "boolean" => "the boolean",
        "integer" | "floating point" => "the number",
        _ => return Cow::Borrowed("a value"),
    };
    Cow::Owned(format!("{named} `{}`", value.trim_matches('`')))
}
