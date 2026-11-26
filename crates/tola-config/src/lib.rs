//! Configuration values, field paths, diagnostics, and the `Config` derive macro.
//!
//! Derives use this crate directly; no local `config` module or re-exports are needed.
//! For a renamed dependency, set `#[config(crate = renamed_dependency)]`.

extern crate self as tola_config;

#[doc(hidden)]
pub use toml;

use serde::Serialize;
use std::borrow::Cow;
use thiserror::Error;

mod diagnostic;
mod field;
mod fragment;
mod presence;
pub mod status;

pub use diagnostic::{
    ConfigDiagnostic, ConfigDiagnosticSeverity, ConfigDiagnosticTag, ConfigDiagnostics,
    ConfigError, ConfigSourceRefusal,
};
pub use field::FieldPath;
pub use fragment::{
    FragmentContext, FragmentIssue, FragmentIssueKind, ValidateFragment, ValueStep, validate_value,
    wrong_type,
};
pub use presence::ConfigPresence;
pub use status::FieldStatus;

pub use tola_config_macros::{Config, Fragment};

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

pub fn serialize_toml_value<T: Serialize>(val: &T) -> Result<String, ConfigSerializationError> {
    toml::Value::try_from(val)
        .map(|v| v.to_string())
        .map_err(ConfigSerializationError)
}

pub const fn section_documentation(doc: &'static str) -> Option<&'static str> {
    if doc.is_empty() { None } else { Some(doc) }
}

/// Append documentation lines as TOML comments, keeping blank lines free of trailing whitespace.
pub fn push_documentation(out: &mut String, doc: &str) {
    for line in doc.lines() {
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
    #[derive(Default, serde::Serialize, super::Config)]
    #[config(section = "service")]
    struct Service {
        #[config(status = experimental)]
        enabled: bool,
    }

    #[test]
    fn derive_schema_runtime() {
        assert_eq!(Service::FIELDS.enabled.as_str(), "service.enabled");
        assert!(Service::try_template().unwrap().contains("enabled = false"));
        let mut diag = super::ConfigDiagnostics::new();
        diag.set_presence(
            super::ConfigPresence::from_toml("[service]\nenabled = false\n").unwrap(),
        );
        Service::default().validate_field_status(&mut diag);
        assert_eq!(diag.experimental_fields(), &[Service::FIELDS.enabled]);
    }

    #[test]
    fn array_element_presence_status() {
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

        let src = r#"
[[groups]]
[[groups.entries]]
[[groups.entries]]
future = false
[[groups]]
preview = false
[[groups.entries]]
[[groups.entries]]
"#;
        let mut val: Parent = toml::from_str(src).unwrap();
        // Values supplied by invocation code must not become explicit.
        val.groups.push(Group {
            entries: vec![Entry { future: true }],
            preview: true,
        });
        let mut diag = super::ConfigDiagnostics::new();
        diag.set_presence(super::ConfigPresence::from_toml(src).unwrap());
        val.validate_field_status(&mut diag);

        assert_eq!(diag.errors().len(), 1);
        assert_eq!(diag.errors()[0].field, Entry::FIELDS.future);
        assert_eq!(diag.warnings().len(), 2);
        assert_eq!(diag.experimental_fields(), &[Group::FIELDS.preview]);
        assert!(diag.is_present("groups.entries.future"));
        assert!(diag.into_result().is_err());
    }

    struct Unserializable(bool);

    impl super::ValidateFragment for Unserializable {
        fn validate_fragment(val: &toml::Value, ctx: &mut super::FragmentContext<'_>) {
            super::validate_value::<String>(val, ctx);
        }
    }

    impl Default for Unserializable {
        fn default() -> Self {
            Self(true)
        }
    }

    impl serde::Serialize for Unserializable {
        fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
            if self.0 {
                Err(serde::ser::Error::custom("unsupported configuration value"))
            } else {
                ser.serialize_str("available")
            }
        }
    }

    #[test]
    fn template_error_cause() {
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

        let val = Child::default();
        for err in [
            Child::try_template().unwrap_err(),
            Child::try_template_from(&val).unwrap_err(),
            Child::try_template_with_header().unwrap_err(),
            Child::try_template_with_header_from(&val).unwrap_err(),
            Parent::try_template().unwrap_err(),
        ] {
            assert_eq!(err.field(), "site.child.value");
            let ser = std::error::Error::source(&err).unwrap();
            assert!(ser.is::<super::ConfigSerializationError>());
            assert!(
                std::error::Error::source(ser)
                    .unwrap()
                    .is::<toml::ser::Error>()
            );
        }
    }

    #[test]
    fn template_error_array_idx() {
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

        let val = Parent {
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
        for err in [
            Parent::try_template_from(&val).unwrap_err(),
            Parent::try_template_with_header_from(&val).unwrap_err(),
        ] {
            assert_eq!(err.field(), "groups.1.entries.1.value");
            assert!(
                std::error::Error::source(err.serialization_error())
                    .unwrap()
                    .is::<toml::ser::Error>()
            );
        }
    }
}
