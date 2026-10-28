//! The declared output one record chain answers.

use anyhow::Result;

use super::super::semantic::Semantic;
use super::super::site_schema::{declared_schema, descriptor_line, descriptor_type};
use super::origin::{
    ProjectedField, ProjectedRecords, ProjectedValue, RecordOrigin, SourceDescriptors,
    SourceExpression,
};

/// The descriptor lines one chain addresses; meta reads the schema declaration instead.
pub(in crate::query) fn descriptor_fields(
    origin: &RecordOrigin,
    fields: &[String],
) -> Option<Vec<String>> {
    let descriptors = origin.descriptors()?;
    match fields {
        [] => Some(
            descriptors
                .fields()
                .into_iter()
                .map(descriptor_line)
                .collect(),
        ),
        [field] => Some(vec![descriptor_line(descriptor_field(descriptors, field)?)]),
        _ => None,
    }
}

/// The descriptor field one name addresses, unless it is meta.
fn descriptor_field(
    descriptors: SourceDescriptors,
    field: &str,
) -> Option<tola_build::SourceDescriptorField> {
    if field == tola_build::SourceDescriptorField::Metadata.name() {
        return None;
    }
    descriptors
        .fields()
        .into_iter()
        .find(|candidate| candidate.name() == field)
}

/// The declared output one record chain answers.
#[derive(Clone, PartialEq)]
pub(in crate::query) struct DeclaredOutput {
    /// The lines the answer has, one per declared field.
    pub(in crate::query) lines: Vec<String>,
    /// The declaration's own description, when the chain reads a declared metadata container.
    pub(in crate::query) documentation: Option<String>,
}

/// The declared output one chain reads; path names the read a metadata field declares.
pub(in crate::query) fn declared_output(
    origin: &RecordOrigin,
    fields: &[String],
    path: &str,
    semantics: &mut Semantic<'_>,
) -> Result<Option<DeclaredOutput>> {
    let reads_metadata = fields
        .first()
        .is_some_and(|field| field == tola_build::SourceDescriptorField::Metadata.name());
    match origin {
        RecordOrigin::Projected(records) => records.declared_output(fields, path, semantics),
        RecordOrigin::Parsed { schema, .. } if reads_metadata => {
            declared_metadata_output(schema, fields, path, semantics)
        }
        _ => Ok(
            descriptor_fields(origin, fields).map(|lines| DeclaredOutput {
                lines,
                documentation: None,
            }),
        ),
    }
}

/// The schema declaration one metadata read answers, declared by the file parsing these records.
fn declared_metadata_output(
    schema: &SourceExpression,
    fields: &[String],
    path: &str,
    semantics: &mut Semantic<'_>,
) -> Result<Option<DeclaredOutput>> {
    let Some(declared) = declared_schema(schema, semantics)? else {
        return Ok(None);
    };
    let Some(declared) = declared.at_path(&fields[1..]) else {
        return Ok(None);
    };
    let declared_fields = declared.fields();
    if declared_fields.is_empty() {
        return Ok(Some(DeclaredOutput {
            lines: vec![declared.field(path).list_item()],
            documentation: None,
        }));
    }
    Ok(Some(DeclaredOutput {
        lines: declared_fields
            .iter()
            .map(|(field, description)| description.field(field).list_item())
            .collect(),
        documentation: declared.documentation(),
    }))
}

impl ProjectedRecords {
    /// The declared output one chain reading `fields` beyond these records answers.
    fn declared_output(
        &self,
        fields: &[String],
        path: &str,
        semantics: &mut Semantic<'_>,
    ) -> Result<Option<DeclaredOutput>> {
        let Some((field, rest)) = fields.split_first() else {
            let mut lines = Vec::new();
            for projected in &self.fields {
                lines.push(format!(
                    "`{}: {}`",
                    projected.name,
                    projected.ty(semantics)?
                ));
            }
            return Ok(Some(DeclaredOutput {
                lines,
                documentation: None,
            }));
        };
        let Some(projected) = self.field(field) else {
            return Ok(None);
        };
        match &projected.known {
            Some(ProjectedValue::Record(origin)) => declared_output(origin, rest, path, semantics),
            Some(ProjectedValue::Declared(ty)) if rest.is_empty() => Ok(Some(DeclaredOutput {
                lines: vec![format!("`{field}: {ty}`")],
                documentation: None,
            })),
            _ => Ok(None),
        }
    }
}

impl ProjectedField {
    /// The type a field reads as: what the body proves, else what the world observed.
    fn ty(&self, semantics: &mut Semantic<'_>) -> Result<String> {
        match &self.known {
            Some(known) => Ok(known.type_name()),
            None => Ok(self
                .observed_type(semantics)?
                .unwrap_or_else(|| "any".to_owned())),
        }
    }

    /// The type the world observed at the value the field binds.
    fn observed_type(&self, semantics: &mut Semantic<'_>) -> Result<Option<String>> {
        let Some(node) = self.value.node() else {
            return Ok(None);
        };
        Ok(semantics
            .types(&node)?
            .first()
            .map(|(value, _)| value.type_name()))
    }
}

/// The type a chain declares from a project body: a field record, a call return, or one descriptor.
pub(in crate::query) fn declared_chain_type(
    origin: &RecordOrigin,
    fields: &[String],
) -> Option<String> {
    match (origin, fields) {
        (RecordOrigin::Projected(_), []) => Some("dictionary".to_owned()),
        (RecordOrigin::Projected(records), [field]) => records
            .field(field)?
            .known
            .as_ref()
            .map(ProjectedValue::type_name),
        (_, [field]) => descriptor_field_type(origin.descriptors()?, field),
        _ => None,
    }
}

/// The type one descriptor field has, when the shape has that field.
pub(in crate::query) fn descriptor_field_type(
    descriptors: SourceDescriptors,
    field: &str,
) -> Option<String> {
    Some(descriptor_type(descriptor_field(descriptors, field)?.kind()).to_owned())
}

#[cfg(test)]
mod tests {
    use crate::query::tests::*;

    /// The metadata container answers every field of the schema the file parses against.
    #[test]
    fn metadata_shape_follows_the_parsed_schema() {
        let program = parsed_program(PAGE_SCHEMA);
        let mut site = QuerySession::with_program(&program);
        site.site.write("content/page.typ", SCHEMA_PAGE);
        let hover = site_hover(&mut site, &marked_before(&program, "ta.draft)")).expect("a hover");
        assert!(
            hover.contains("- `draft: bool = false`\n  keeps the page out of the published site"),
            "{hover}"
        );
        assert!(
            hover.contains("- `title: str = \"Untitled\"`\n  the page title"),
            "{hover}"
        );
    }

    #[test]
    fn schema_shape_matches_output_values() {
        let cases = [
            ("nullable(str)", "none", Some("value: none | str")),
            ("one-or-many(str)", "\"tag\"", Some("value: array")),
            ("trim(literal(\"  x  \"))", "\"  x  \"", Some("value: str")),
            (
                "tuple(str, rest: int)",
                "(\"point\", 1, 2)",
                // A tuple value is an array in Typst's type system, so it spells `array`.
                Some("value: array"),
            ),
            ("lazy(() => str)", "\"text\"", None),
            ("union(lazy(() => str), int)", "\"text\"", None),
            (
                "optional(optional(str, default: \"inner\"))",
                "\"text\"",
                Some("value?: str"),
            ),
        ];
        for (declaration, value, expected) in cases {
            let program = parsed_program(&format!("{PAGE_SCHEMA}\n  value: {declaration},"))
                .replace(
                    "describe, optional, schema",
                    "describe, optional, schema, nullable, one-or-many, tuple, lazy, union, trim, literal",
                );
            let mut site = QuerySession::with_program(&program);
            site.site.write(
                "content/page.typ",
                &format!("#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"Page\", value: {value}))\n"),
            );
            let hover = site_hover(&mut site, &marked_before(&program, "ta.draft)"))
                .expect("metadata hover");
            if let Some(expected) = expected {
                assert!(
                    hover.contains(&format!("- `{expected}`")),
                    "{declaration}: {hover}"
                );
            } else {
                assert!(hover.contains("- `value?`"), "{declaration}: {hover}");
            }
        }
    }

    #[test]
    fn schema_shape_follows_record_origin() {
        let second_schema = "#let second-schema = schema((title: describe(optional(str, default: \"Other\"), \"unrelated title\"), draft: optional(bool, default: false)))\n";
        let original = parsed_program(PAGE_SCHEMA);
        let cases = [
            (original.clone(), true),
            (
                original
                    .replace(
                        "#import \"@tola/source:0.0.0\": all-sources, parse-sources",
                        "#import \"@tola/source:0.0.0\" as sources",
                    )
                    .replace("all-sources()", "sources.all-sources()")
                    .replace("parse-sources(", "sources.parse-sources("),
                true,
            ),
            (
                original.clone()
                    + second_schema
                    + "#let second = parse-sources(all-sources(), second-schema)\n",
                true,
            ),
            (
                original.replace(
                    "#let kept = declared.filter",
                    "#let raw = all-sources()\n#let kept = raw.filter",
                ),
                false,
            ),
            (
                original.replace(
                    "#let kept = declared.filter",
                    "#let alias = (declared)\n#let kept = alias.filter",
                ),
                true,
            ),
            (
                original.replace(
                    "#let kept = declared.filter",
                    "#{ declared = all-sources() }\n#let kept = declared.filter",
                ),
                false,
            ),
            (
                original.replace(
                    "#let kept = declared.filter",
                    "#{ declared.at(0).meta.title = \"Edited\" }\n#let kept = declared.filter",
                ),
                false,
            ),
            (
                original.replace(
                    "not source.meta.draft",
                    "{ source.meta.title = \"Edited\"; not source.meta.draft }",
                ),
                false,
            ),
        ];
        for (program, parsed) in cases {
            let mut site = QuerySession::with_program(&program);
            site.site.write("content/page.typ", DECLARED_PAGE);
            let hover = site_hover(&mut site, &marked_before(&program, "ta.draft"))
                .expect("metadata hover");
            assert_eq!(
                hover.contains("the page title"),
                parsed,
                "{program}: {hover}"
            );
            assert_eq!(
                hover.contains("= \"Untitled\""),
                parsed,
                "{program}: {hover}"
            );
            assert!(!hover.contains("unrelated title"), "{program}: {hover}");
        }
    }

    /// A `current-source()` descriptor answers the fields the lexical call has, without
    /// `meta`.
    #[test]
    fn current_source_descriptor_omits_metadata() {
        let mut site = QuerySession::with_program(&parsed_program(PAGE_SCHEMA));
        let hover = site
            .hover_text(
                "#import \"@tola/source:0.0.0\": current-source\n#let inp|ut = current-source()\n",
            )
            .expect("a hover");
        assert!(hover.contains("- `route-segments: array`"), "{hover}");
        assert!(!hover.contains("- `meta: dictionary`"), "{hover}");
    }
}
