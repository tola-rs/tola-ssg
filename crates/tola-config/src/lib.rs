//! Configuration values, field paths, diagnostics, and the `Config` derive macro.
//!
//! Derives use this crate directly; no local `config` module or re-exports are needed.
//! For a renamed dependency, set `#[config(crate = renamed_dependency)]`.

extern crate self as tola_config;

use serde::Serialize;
use std::borrow::Cow;
use thiserror::Error;

mod diagnostic;
mod field;
mod presence;
pub mod status;

pub use diagnostic::{
    ConfigDiagnostic, ConfigDiagnosticSeverity, ConfigDiagnosticTag, ConfigDiagnostics,
    ConfigError, ConfigSourceRefusal,
};
pub use field::FieldPath;
pub use presence::ConfigPresence;
pub use status::FieldStatus;

pub use tola_config_macros::Config;

/// A failure to serialize a configuration value as TOML.
#[derive(Debug, Error)]
#[error("the configuration value could not be written as TOML")]
pub struct ConfigSerializationError(#[source] toml::ser::Error);

impl ConfigSerializationError {
    pub fn source_error(&self) -> &toml::ser::Error {
        &self.0
    }
}

/// A template field that could not be serialized, including enclosing array-table indices.
#[derive(Debug, Error)]
#[error("the configuration value for `{field}` could not be written as TOML")]
pub struct ConfigTemplateError {
    field: Cow<'static, str>,
    #[source]
    source: ConfigSerializationError,
}

impl ConfigTemplateError {
    pub fn new(field: FieldPath, source: ConfigSerializationError) -> Self {
        Self {
            field: Cow::Borrowed(field.as_str()),
            source,
        }
    }

    pub fn field(&self) -> &str {
        &self.field
    }

    pub fn serialization_error(&self) -> &ConfigSerializationError {
        &self.source
    }

    /// Add an enclosing array-table index while propagating a child's failure.
    pub fn with_array_element(mut self, array: FieldPath, index: usize) -> Self {
        if let Some(field) = field::indexed_field_path(&self.field, &[(array.as_str(), index)]) {
            self.field = Cow::Owned(field);
        }
        self
    }
}

/// Serialize a configuration value as TOML.
///
/// Used by `Config` derives so callers do not need a direct `toml` dependency.
pub fn serialize_toml_value<T: Serialize>(value: &T) -> Result<String, ConfigSerializationError> {
    toml::Value::try_from(value)
        .map(|value| value.to_string())
        .map_err(ConfigSerializationError)
}

/// The documentation a section's declaration wrote, or `None` when it wrote none.
///
/// A field that opens a section documents that key with the section's own declaration, so a
/// section with no doc comment answers nothing rather than an empty hover.
pub const fn section_documentation(documentation: &'static str) -> Option<&'static str> {
    if documentation.is_empty() {
        None
    } else {
        Some(documentation)
    }
}

/// Append one documentation paragraph to TOML output, one comment line per line.
///
/// A blank line between paragraphs becomes `#` alone, so a generated comment never has
/// trailing whitespace.
pub fn push_documentation(out: &mut String, documentation: &str) {
    for line in documentation.lines() {
        let line = line.trim();
        out.push('#');
        if !line.is_empty() {
            out.push(' ');
            out.push_str(line);
        }
        out.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::serialize_toml_value;

    #[derive(Default, serde::Serialize, super::Config)]
    #[config(section = "service")]
    struct Service {
        #[config(status = experimental)]
        enabled: bool,
    }

    #[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize, super::Config)]
    #[config(section = "")]
    #[serde(bound(serialize = "", deserialize = ""))]
    struct Renamed {
        #[config(name = "display-name")]
        #[serde(rename(serialize = "display-name", deserialize = "display-name"))]
        title: String,
        #[serde(rename(serialize = "count"))]
        count: u16,
        #[serde(rename(deserialize = "enabled"))]
        enabled: bool,
        #[serde(bound(serialize = "", deserialize = ""))]
        r#type: String,
        #[config(collection = inline)]
        values: std::option::Option<std::vec::Vec<u16>>,
    }

    #[test]
    fn renamed_keys_round_trip_template() {
        let value = Renamed {
            title: "renamed".into(),
            count: 7,
            enabled: true,
            r#type: "raw identifier".into(),
            values: Some(vec![2, 3]),
        };
        let template = Renamed::try_template_from(&value).unwrap();
        let decoded: Renamed = toml::from_str(&template).unwrap();
        assert_eq!(decoded, value);
        assert_eq!(Renamed::FIELDS.title.as_str(), "display-name");
        assert_eq!(Renamed::FIELDS.r#type.as_str(), "type");
    }

    #[derive(Default, serde::Serialize, serde::Deserialize, super::Config)]
    #[config(section = "")]
    struct ScalarDefaults {
        #[config(default = "-1_024")]
        signed: i32,
        #[config(default = "0xDEAD_BEEF")]
        radix: u64,
        #[config(default = "+1.25e2")]
        exponent: f64,
        #[config(default = "-inf")]
        infinity: f64,
        #[config(default = "+nan")]
        not_a_number: f64,
        #[config(default = "a\\b\"c\nd")]
        text: String,
    }

    #[test]
    fn scalar_defaults_round_trip_as_toml() {
        let value = ScalarDefaults::default();
        let template = ScalarDefaults::try_template_from(&value).unwrap();
        let decoded: ScalarDefaults = toml::from_str(&template).unwrap();
        assert_eq!(decoded.signed, -1024);
        assert_eq!(decoded.radix, 0xDEAD_BEEF);
        assert_eq!(decoded.exponent, 125.0);
        assert_eq!(decoded.infinity, f64::NEG_INFINITY);
        assert!(decoded.not_a_number.is_nan());
        assert_eq!(decoded.text, "a\\b\"c\nd");
        assert_eq!(value.signed, 0);
        assert_eq!(value.text, "");
    }

    #[test]
    fn blank_documentation_line_has_no_trailing_space() {
        let mut out = String::new();
        super::push_documentation(&mut out, "First sentence.\n\nSecond sentence.");
        assert_eq!(out, "# First sentence.\n#\n# Second sentence.\n");
    }

    #[test]
    fn derive_binds_schema_to_runtime() {
        assert_eq!(Service::FIELDS.enabled.as_str(), "service.enabled");
        assert!(Service::try_template().unwrap().contains("enabled = false"));
        let mut diagnostics = super::ConfigDiagnostics::new();
        diagnostics.set_presence(
            super::ConfigPresence::from_toml("[service]\nenabled = false\n").unwrap(),
        );
        Service::default().validate_field_status(&mut diagnostics);
        assert_eq!(
            diagnostics.experimental_fields(),
            &[Service::FIELDS.enabled]
        );
    }

    #[test]
    fn template_round_trips_subtable_parents() {
        #[derive(
            Debug, Default, PartialEq, serde::Serialize, serde::Deserialize, super::Config,
        )]
        #[serde(default, deny_unknown_fields)]
        #[config(section = "")]
        struct Parent {
            #[config(sub)]
            child: Child,
            count: u8,
            #[config(sub, collection = array_table)]
            rows: Vec<Row>,
            #[serde(rename = "wire-label")]
            label: String,
        }

        #[derive(
            Debug, Default, PartialEq, serde::Serialize, serde::Deserialize, super::Config,
        )]
        #[serde(default, deny_unknown_fields)]
        #[config(section = "child")]
        struct Child {
            enabled: bool,
        }

        #[derive(
            Debug, Default, PartialEq, serde::Serialize, serde::Deserialize, super::Config,
        )]
        #[serde(default, deny_unknown_fields)]
        #[config(section = "rows", collection = array_table)]
        struct Row {
            value: u8,
        }

        let value = Parent {
            child: Child { enabled: true },
            count: 7,
            rows: vec![Row { value: 3 }, Row { value: 5 }],
            label: "a parent value".into(),
        };
        let template = Parent::try_template_from(&value).unwrap();
        assert_eq!(toml::from_str::<Parent>(&template).unwrap(), value);
    }

    #[test]
    fn subtable_headers_preserve_values() {
        #[derive(
            Debug, Default, PartialEq, serde::Serialize, serde::Deserialize, super::Config,
        )]
        #[serde(default)]
        #[config(section = "")]
        struct Site {
            #[config(sub)]
            typst: Typst,
            #[config(sub, collection = array_table)]
            groups: Vec<Group>,
        }

        #[derive(
            Debug, Default, PartialEq, serde::Serialize, serde::Deserialize, super::Config,
        )]
        #[serde(default)]
        #[config(section = "typst")]
        struct Typst {
            #[config(sub)]
            fonts: Fonts,
        }

        #[derive(
            Debug, Default, PartialEq, serde::Serialize, serde::Deserialize, super::Config,
        )]
        #[serde(default)]
        #[config(section = "typst.fonts")]
        struct Fonts {
            system: bool,
        }

        #[derive(
            Debug, Default, PartialEq, serde::Serialize, serde::Deserialize, super::Config,
        )]
        #[serde(default)]
        #[config(section = "groups", collection = array_table)]
        struct Group {
            #[config(sub)]
            settings: Settings,
        }

        #[derive(
            Debug, Default, PartialEq, serde::Serialize, serde::Deserialize, super::Config,
        )]
        #[serde(default)]
        #[config(section = "groups.settings")]
        struct Settings {
            enabled: bool,
        }

        let value = Site {
            typst: Typst {
                fonts: Fonts { system: true },
            },
            groups: vec![
                Group {
                    settings: Settings { enabled: true },
                },
                Group {
                    settings: Settings { enabled: false },
                },
            ],
        };
        for template in [
            Site::try_template_from(&value).unwrap(),
            Site::try_template_with_header_from(&value).unwrap(),
        ] {
            assert!(!template.lines().any(|line| line == "[typst]"));
            assert!(template.lines().any(|line| line == "[typst.fonts]"));
            assert_eq!(
                template
                    .lines()
                    .filter(|line| *line == "[[groups]]")
                    .count(),
                2
            );
            assert_eq!(toml::from_str::<Site>(&template).unwrap(), value);
        }
        for template in [
            Site::try_template().unwrap(),
            Site::try_template_with_header().unwrap(),
        ] {
            assert_eq!(toml::from_str::<Site>(&template).unwrap(), Site::default());
            assert!(!template.lines().any(|line| line == "[typst]"));
        }
        let typst = Typst::try_template_with_header_from(&value.typst).unwrap();
        let parsed: toml::Value = toml::from_str(&typst).unwrap();
        assert_eq!(
            parsed["typst"],
            toml::Value::try_from(&value.typst).unwrap()
        );
    }

    #[test]
    fn array_element_status_follows_explicit_presence() {
        #[derive(Default, serde::Serialize, serde::Deserialize, super::Config)]
        #[serde(default)]
        #[config(section = "")]
        struct Parent {
            #[config(sub, collection = array_table)]
            groups: Vec<Group>,
        }

        #[derive(Default, serde::Serialize, serde::Deserialize, super::Config)]
        #[serde(default)]
        #[config(section = "groups", collection = array_table, status = deprecated)]
        struct Group {
            #[config(sub, collection = array_table)]
            entries: Vec<Entry>,
            #[config(status = experimental)]
            preview: bool,
        }

        #[derive(Default, serde::Serialize, serde::Deserialize, super::Config)]
        #[serde(default)]
        #[config(section = "groups.entries", collection = array_table)]
        struct Entry {
            #[config(status = not_implemented)]
            future: bool,
        }

        let source = r#"
[[groups]]
[[groups.entries]]
[[groups.entries]]
future = false
[[groups]]
preview = false
[[groups.entries]]
[[groups.entries]]
"#;
        let mut value: Parent = toml::from_str(source).unwrap();
        // Values supplied by invocation code must not become explicit.
        value.groups.push(Group {
            entries: vec![Entry { future: true }],
            preview: true,
        });
        let mut diagnostics = super::ConfigDiagnostics::new();
        diagnostics.set_presence(super::ConfigPresence::from_toml(source).unwrap());
        value.validate_field_status(&mut diagnostics);

        assert_eq!(diagnostics.errors().len(), 1);
        assert_eq!(diagnostics.errors()[0].field, Entry::FIELDS.future);
        assert_eq!(diagnostics.warnings().len(), 2);
        assert_eq!(diagnostics.experimental_fields(), &[Group::FIELDS.preview]);
        assert!(diagnostics.is_present("groups.entries.future"));
        assert!(diagnostics.into_result().is_err());
    }

    #[test]
    fn inline_arrays_serialize_as_toml() {
        let value = vec!["first", "second"];
        assert_eq!(
            serialize_toml_value(&value).unwrap(),
            "[\"first\", \"second\"]"
        );
    }

    #[test]
    fn serializer_errors_keep_their_cause() {
        let value = std::collections::HashMap::from([(1_u8, "value")]);
        let error = serialize_toml_value(&value).expect_err("integer map keys are not TOML");

        let source = std::error::Error::source(&error).expect("serializer error remains the cause");
        assert!(source.is::<toml::ser::Error>());
    }

    struct Unserializable(bool);

    impl Default for Unserializable {
        fn default() -> Self {
            Self(true)
        }
    }

    impl serde::Serialize for Unserializable {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            if self.0 {
                Err(serde::ser::Error::custom("unsupported configuration value"))
            } else {
                serializer.serialize_str("available")
            }
        }
    }

    #[test]
    fn template_errors_keep_field_and_cause() {
        #[derive(Default, serde::Serialize, super::Config)]
        #[config(crate = crate, section = "site.child")]
        struct Child {
            value: Unserializable,
        }

        #[derive(Default, serde::Serialize, super::Config)]
        #[config(section = "")]
        struct Parent {
            #[config(sub)]
            child: Child,
        }

        let value = Child::default();
        for error in [
            Child::try_template().unwrap_err(),
            Child::try_template_from(&value).unwrap_err(),
            Child::try_template_with_header().unwrap_err(),
            Child::try_template_with_header_from(&value).unwrap_err(),
            Parent::try_template().unwrap_err(),
        ] {
            assert_eq!(error.field(), "site.child.value");
            let serialization = std::error::Error::source(&error).unwrap();
            assert!(serialization.is::<super::ConfigSerializationError>());
            assert!(
                std::error::Error::source(serialization)
                    .unwrap()
                    .is::<toml::ser::Error>()
            );
        }
    }

    #[test]
    fn template_errors_identify_array_elements() {
        #[derive(Default, serde::Serialize, super::Config)]
        #[config(section = "")]
        struct Parent {
            #[config(sub, collection = array_table)]
            groups: Vec<Group>,
        }

        #[derive(Default, serde::Serialize, super::Config)]
        #[config(section = "groups", collection = array_table)]
        struct Group {
            #[config(sub, collection = array_table)]
            entries: Vec<Entry>,
        }

        #[derive(Default, serde::Serialize, super::Config)]
        #[config(section = "groups.entries", collection = array_table)]
        struct Entry {
            value: Unserializable,
        }

        let value = Parent {
            groups: vec![
                Group {
                    entries: vec![Entry {
                        value: Unserializable(false),
                    }],
                },
                Group {
                    entries: vec![
                        Entry {
                            value: Unserializable(false),
                        },
                        Entry {
                            value: Unserializable(true),
                        },
                    ],
                },
            ],
        };
        for error in [
            Parent::try_template_from(&value).unwrap_err(),
            Parent::try_template_with_header_from(&value).unwrap_err(),
        ] {
            assert_eq!(error.field(), "groups.1.entries.1.value");
            assert!(
                std::error::Error::source(error.serialization_error())
                    .unwrap()
                    .is::<toml::ser::Error>()
            );
        }
    }
}
