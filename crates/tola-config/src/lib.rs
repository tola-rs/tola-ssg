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
