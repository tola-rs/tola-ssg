//! Editor projection of the public `@tola/schema.inspect` output description.

use std::collections::BTreeSet;

use super::hover::SECTION_JOIN;

use tola_typst::typst::foundations::{Array, Dict, Str, Value};

#[derive(Clone)]
pub(super) struct OutputDescription(Dict);

impl OutputDescription {
    pub(super) fn read(value: Value) -> Option<Self> {
        let Value::Dict(dict) = value else {
            return None;
        };
        string(&dict, "kind")?;
        match string(&dict, "presence")? {
            "required" | "optional" | "unknown" => Some(Self(dict)),
            _ => None,
        }
    }

    fn kind(&self) -> &str {
        string(&self.0, "kind").unwrap()
    }

    fn children(&self, key: &str) -> Vec<Self> {
        let Ok(Value::Array(values)) = self.0.get(key) else {
            return Vec::new();
        };
        values.iter().cloned().filter_map(Self::read).collect()
    }

    fn branches(&self) -> Vec<Self> {
        self.children(if self.kind() == "union" {
            "members"
        } else {
            "branches"
        })
    }

    pub(super) fn documentation(&self) -> Option<String> {
        if let Some(description) = string(&self.0, "description") {
            return (!description.is_empty()).then(|| description.to_owned());
        }
        if !matches!(self.kind(), "union" | "variant") {
            return None;
        }
        let branches = self.branches();
        let descriptions = branches
            .iter()
            .filter_map(Self::documentation)
            .collect::<BTreeSet<_>>();
        (!descriptions.is_empty())
            .then(|| descriptions.into_iter().collect::<Vec<_>>().join("\n\n"))
    }

    /// Whether callers may write keys this description does not declare: `unknown: "keep"`.
    pub(super) fn accepts_unknown(&self) -> bool {
        string(&self.0, "unknown") == Some("keep")
    }

    /// Whether the value must be written: presence `required`.
    pub(super) fn is_required(&self) -> bool {
        string(&self.0, "presence") == Some("required")
    }

    pub(super) fn ty(&self) -> Option<String> {
        match self.kind() {
            "type" => match self.0.get("type").ok()? {
                Value::Type(ty) => Some(ty.short_name().to_owned()),
                _ => None,
            },
            "any" => Some("any".into()),
            "literal" => self.0.get("value").ok().map(super::semantic::sampled_value),
            "enum" => {
                let Value::Array(values) = self.0.get("values").ok()? else {
                    return None;
                };
                Some(
                    values
                        .iter()
                        .map(super::semantic::sampled_value)
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect::<Vec<_>>()
                        .join(" | "),
                )
            }
            "object" | "variant" | "dictionary" => Some("dictionary".into()),
            // A tuple reads as array: Typst never spells an array or tuple by its member types.
            "array" | "tuple" => Some("array".into()),
            "union" => {
                let mut types = Vec::new();
                for member in self.children("members") {
                    let ty = member.ty()?;
                    if !types.contains(&ty) {
                        types.push(ty);
                    }
                }
                (!types.is_empty()).then(|| types.join(" | "))
            }
            _ => None,
        }
    }

    pub(super) fn at_path(&self, path: &[String]) -> Option<Self> {
        let Some((key, rest)) = path.split_first() else {
            return Some(self.clone());
        };
        match self.kind() {
            "object" => {
                let Value::Dict(fields) = self.0.get("fields").ok()? else {
                    return None;
                };
                match fields.get(key) {
                    Ok(value) => Self::read(value.clone())?.at_path(rest),
                    Err(_) if self.accepts_unknown() => {
                        Some(Self::unconstrained("any", "optional"))
                    }
                    Err(_) => None,
                }
            }
            "dictionary" => {
                let mut field = Self::read(self.0.get("element").ok()?.clone())?.at_path(rest)?;
                field
                    .0
                    .insert("presence".into(), Value::Str("optional".into()));
                Some(field)
            }
            "any" => Some(Self::unconstrained("any", "optional")),
            "unknown" => Some(Self::unconstrained("unknown", "unknown")),
            "union" | "variant" => {
                let branches = self.branches();
                let selected = branches
                    .iter()
                    .filter_map(|branch| branch.at_path(path))
                    .collect::<Vec<_>>();
                if selected.is_empty() {
                    return None;
                }
                let presence = if selected.len() != branches.len()
                    || selected
                        .iter()
                        .any(|branch| string(&branch.0, "presence") == Some("optional"))
                {
                    "optional"
                } else if selected
                    .iter()
                    .any(|branch| string(&branch.0, "presence") == Some("unknown"))
                {
                    "unknown"
                } else {
                    "required"
                };
                let mut joined = Self::unconstrained("union", presence);
                joined.0.insert(
                    "members".into(),
                    Value::Array(
                        selected
                            .into_iter()
                            .map(|branch| Value::Dict(branch.0))
                            .collect::<Array>(),
                    ),
                );
                Some(joined)
            }
            _ => None,
        }
        .map(|mut field| {
            let parent = string(&self.0, "presence");
            let child = string(&field.0, "presence");
            let presence = if parent == Some("optional") || child == Some("optional") {
                "optional"
            } else if parent == Some("unknown") || child == Some("unknown") {
                "unknown"
            } else {
                "required"
            };
            field
                .0
                .insert("presence".into(), Value::Str(presence.into()));
            field
        })
    }

    fn unconstrained(kind: &str, presence: &str) -> Self {
        let mut description = Dict::new();
        description.insert("kind".into(), Value::Str(kind.into()));
        description.insert("presence".into(), Value::Str(presence.into()));
        description.insert("description".into(), Value::None);
        Self(description)
    }

    pub(super) fn fields(&self) -> Vec<(String, Self)> {
        match self.kind() {
            "object" => match self.0.get("fields") {
                Ok(Value::Dict(fields)) => fields
                    .iter()
                    .filter_map(|(name, value)| {
                        Some((name.to_string(), Self::read(value.clone())?))
                    })
                    .collect(),
                _ => Vec::new(),
            },
            "union" | "variant" => {
                let mut names = BTreeSet::new();
                for branch in self.branches() {
                    names.extend(branch.fields().into_iter().map(|(name, _)| name));
                }
                names
                    .into_iter()
                    .filter_map(|name| {
                        let description = self.at_path(std::slice::from_ref(&name))?;
                        Some((name, description))
                    })
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    /// The declared field one name addresses, with the schema's presence fact.
    pub(super) fn field(&self, name: &str) -> DeclaredField {
        let optional = !self.is_required();
        let mut declaration = format!("{name}{}", if optional { "?" } else { "" });
        if let Some(ty) = self.ty() {
            declaration.push_str(&format!(": {ty}"));
        }
        if let Ok(Value::Dict(default)) = self.0.get("default")
            && let Ok(value) = default.get("value")
        {
            declaration.push_str(&format!(" = {}", super::semantic::sampled_value(value)));
        }
        DeclaredField {
            declaration,
            presence_unknown: string(&self.0, "presence") == Some("unknown"),
            documentation: self.documentation(),
        }
    }
}

/// One declared field: its declaration, and the facts the schema states beyond it.
#[derive(Clone, PartialEq)]
pub(super) struct DeclaredField {
    /// The declaration itself, without code ticks: `key?: ty = default`.
    pub(super) declaration: String,
    /// Whether the schema states neither required nor optional presence.
    pub(super) presence_unknown: bool,
    /// The documentation the declaration writes.
    pub(super) documentation: Option<String>,
}

impl DeclaredField {
    /// One declared field as a list item: declaration, presence, then documentation.
    pub(super) fn list_item(&self) -> String {
        let mut item = format!("`{}`", self.declaration);
        if self.presence_unknown {
            item.push_str("\n  (presence unknown)");
        }
        if let Some(documentation) = &self.documentation {
            item.push_str(&format!("\n  {documentation}"));
        }
        item
    }

    /// One declared field as an answer: declaration fence, presence, documentation.
    pub(super) fn section(&self) -> String {
        let mut section = format!("```typc\n{}\n```", self.declaration);
        if self.presence_unknown {
            section.push_str(SECTION_JOIN);
            section.push_str("(presence unknown)");
        }
        if let Some(documentation) = &self.documentation {
            section.push_str(SECTION_JOIN);
            section.push_str(documentation);
        }
        section
    }
}

fn string<'a>(dict: &'a Dict, key: &str) -> Option<&'a str> {
    match dict.get(key).ok()? {
        Value::Str(text) => Some(Str::as_str(text)),
        _ => None,
    }
}
