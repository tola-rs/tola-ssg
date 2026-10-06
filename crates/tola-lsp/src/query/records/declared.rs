//! The declared output one record chain answers.

use anyhow::Result;

use super::super::semantic::{self, Semantic};
use super::super::site_schema::{declared_schema, descriptor_line, descriptor_type};
use super::origin::{
    ProjectedField, ProjectedRecords, ProjectedValue, RecordOrigin, SourceDescriptors,
};

/// The descriptor field lines one chain rooted at the site's own descriptors addresses.
///
/// The whole shape answers for the root itself; one field answers for a chain reading it. A chain
/// through `meta` reads the schema declaration instead.
pub(in crate::query) fn descriptor_fields(
    descriptors: SourceDescriptors,
    fields: &[String],
) -> Option<Vec<String>> {
    let carried = descriptors.fields();
    match fields {
        [] => Some(
            carried
                .iter()
                .map(|field| descriptor_line(*field))
                .collect(),
        ),
        [field] if field != tola_build::SourceDescriptorField::Metadata.name() => carried
            .iter()
            .find(|candidate| candidate.name() == field)
            .map(|field| vec![descriptor_line(*field)]),
        _ => None,
    }
}

/// The declared output one record chain answers.
#[derive(Clone, PartialEq)]
pub(in crate::query) struct DeclaredOutput {
    /// The lines the answer has, one per declared field.
    pub(in crate::query) lines: Vec<String>,
    /// The declaration's own description, when the chain reads a declared metadata container.
    pub(in crate::query) documentation: Option<String>,
}

/// The declared output one chain reading `fields` beyond one record origin answers.
///
/// `path` is the chain's own text, which a single declared metadata field names itself by: the
/// field's declaration has the whole read, because the read is what declares the field. The
/// whole shape answers for the root itself, one field answers for a chain reading it, and a
/// record's `meta` read answers the schema the file parsing it declares.
pub(in crate::query) fn declared_output(
    origin: &RecordOrigin,
    fields: &[String],
    path: &str,
    semantics: &mut Semantic<'_>,
) -> Result<Option<DeclaredOutput>> {
    match origin {
        RecordOrigin::Descriptors(descriptors) => {
            Ok(descriptor_fields(*descriptors, fields).map(DeclaredOutput::lines))
        }
        RecordOrigin::Parsed {
            schema,
            descriptors,
        } => {
            if fields
                .first()
                .is_some_and(|field| field == tola_build::SourceDescriptorField::Metadata.name())
            {
                let Some(declared) = declared_schema(schema, semantics)? else {
                    return Ok(None);
                };
                let Some(declared) = declared.at_path(fields.get(1..).unwrap_or_default()) else {
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
            } else {
                // `parse-sources` admits only complete `all-sources()` records, so a call whose
                // input records were not traced still has every descriptor.
                Ok(
                    descriptor_fields(descriptors.unwrap_or(SourceDescriptors::All), fields)
                        .map(DeclaredOutput::lines),
                )
            }
        }
        RecordOrigin::Projected(records) => records.declared_output(fields, path, semantics),
    }
}

impl DeclaredOutput {
    /// One shape's lines as an answer, without a declaration's own description.
    fn lines(lines: Vec<String>) -> Self {
        Self {
            lines,
            documentation: None,
        }
    }
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
        let Some(projected) = self
            .fields
            .iter()
            .find(|candidate| &candidate.name == field)
        else {
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
    /// The type this field reads as: what the body proves, or what the world observed at the
    /// field's own value expression.
    fn ty(&self, semantics: &mut Semantic<'_>) -> Result<String> {
        match &self.known {
            Some(ProjectedValue::Record(_)) => Ok("dictionary".to_owned()),
            Some(ProjectedValue::Declared(ty)) => Ok(ty.clone()),
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
            .values(&node)?
            .first()
            .map(|(value, _)| semantic::type_spelling(&value.ty())))
    }
}

/// The type one chain over a record origin declares, when a project function's body proves it.
///
/// Only the reads a body alone can declare answer here: the record a field has, the type the
/// field's callee returns, or one descriptor field. Every other read is left to the values the
/// world observed at its own expression.
pub(in crate::query) fn declared_chain_type(
    origin: &RecordOrigin,
    fields: &[String],
) -> Option<String> {
    match (origin, fields) {
        (RecordOrigin::Projected(_), []) => Some("dictionary".to_owned()),
        (RecordOrigin::Projected(records), [field]) => {
            let projected = records
                .fields
                .iter()
                .find(|candidate| &candidate.name == field)?;
            match &projected.known {
                Some(ProjectedValue::Record(_)) => Some("dictionary".to_owned()),
                Some(ProjectedValue::Declared(ty)) => Some(ty.clone()),
                None => None,
            }
        }
        (RecordOrigin::Descriptors(descriptors), [field]) => {
            descriptor_field_type(*descriptors, field)
        }
        (RecordOrigin::Parsed { descriptors, .. }, [field]) => {
            descriptor_field_type(descriptors.unwrap_or(SourceDescriptors::All), field)
        }
        _ => None,
    }
}

/// The type one descriptor field has, when the shape has that field.
pub(in crate::query) fn descriptor_field_type(
    descriptors: SourceDescriptors,
    field: &str,
) -> Option<String> {
    if field == tola_build::SourceDescriptorField::Metadata.name() {
        return None;
    }
    descriptors
        .fields()
        .iter()
        .find(|candidate| candidate.name() == field)
        .map(|field| descriptor_type(field.kind()).to_owned())
}

#[cfg(test)]
mod tests {
    use crate::query::tests::*;

    #[test]
    fn metadata_field_answers_its_schema_description() {
        let program = parsed_program(PAGE_SCHEMA);
        let mut site = QuerySession::with_program(&program);
        site.site.write("content/page.typ", SCHEMA_PAGE);
        let hover = site_hover(&mut site, &marked_before(&program, "aft)")).expect("a hover");
        assert!(
            hover.contains("keeps the page out of the published site"),
            "{hover}"
        );
    }

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
    fn metadata_hover_preserves_output_contract() {
        let schema = "title: describe(optional(str), \"Title\"), draft: optional(bool, default: false), author: schema((name: describe(str, \"Author name\"))), score: optional(map(int, str, output: str), default: 7),";
        let program = parsed_program(schema)
            .replace(
                "describe, optional, schema",
                "describe, optional, schema, map",
            )
            .replace("))\n#let declared", "), unknown: \"keep\")\n#let declared")
            .replace(
                "not source.meta.draft",
                "source.meta.author.name != \"hidden\"",
            );
        let mut site = QuerySession::with_program(&program);
        site.site.write("content/page.typ", "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((author: (name: \"Ada\"), extra: 1))\n");
        let container = site_hover(&mut site, &marked_before(&program, "ta.author")).unwrap();
        for expected in ["### Declared fields", "title?: str", "score: str = 7"] {
            assert!(container.contains(expected), "{expected}: {container}");
        }
        let field = site_hover(&mut site, &marked_before(&program, "me !=")).unwrap();
        assert!(field.contains("Author name"), "{field}");
        assert!(field.contains("str"), "{field}");
    }
    #[test]
    fn wrapped_schema_keeps_field_docs() {
        for wrapper in [
            "describe(page-schema, \"Pages\")",
            "check(page-schema, _ => ())",
            "map(any, value => value, output: page-schema)",
        ] {
            let program = parsed_program(PAGE_SCHEMA)
                .replace(
                    "describe, optional, schema",
                    "describe, optional, schema, check, map, any",
                )
                .replace(
                    "parse-sources(all-sources(), page-schema)",
                    &format!("parse-sources(all-sources(), {wrapper})"),
                );
            let mut site = QuerySession::with_program(&program);
            site.site.write("content/page.typ", DECLARED_PAGE);
            let hover = site_hover(&mut site, &marked_before(&program, "aft)")).unwrap();
            assert!(hover.contains("keeps the page out"), "{wrapper}: {hover}");
        }
    }

    #[test]
    fn field_docs_follow_schema_values() {
        let mut site = QuerySession::new();
        let hover = site.hover_text("#import \"@tola/schema:0.0.0\": optional, describe\n#let explanation = \"The title\"\n#let fields = (tit|le: optional(describe(str, explanation)))\n").unwrap();
        assert!(hover.contains("The title"), "{hover}");
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

    #[test]
    fn nested_metadata_shape_follows_its_value() {
        let program = parsed_program(&format!("{PAGE_SCHEMA}\n  author: schema((name: str)),"))
            .replace(
                "not source.meta.draft",
                "source.meta.author.name != \"hidden\"",
            );
        let mut site = QuerySession::with_program(&program);
        site.site.write("content/page.typ", "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"Page\", author: (name: \"A\")))\n");
        let hover = site_hover(&mut site, &marked_before(&program, "or.name"))
            .expect("nested metadata hover");
        assert!(hover.contains("- `name: str`"), "{hover}");
        assert!(!hover.contains("- `title:"), "{hover}");
    }

    /// A schema wider than the shape lists counts the fields it left out.
    #[test]
    fn wide_schema_counts_remaining_fields() {
        let mut schema = String::from(
            "  draft: describe(optional(bool, default: false), \"keeps the page out of the published site\"),\n",
        );
        for index in 0..24 {
            schema.push_str(&format!(
                "  field{index}: describe(optional(str), \"field {index}\"),\n"
            ));
        }
        let program = parsed_program(&schema);
        let mut site = QuerySession::with_program(&program);
        site.site.write("content/page.typ", SCHEMA_PAGE);
        let hover = site_hover(&mut site, &marked_before(&program, "ta.draft)")).expect("a hover");
        assert!(hover.contains("… 5 more fields"), "{hover}");
    }

    /// The descriptor shape answers from its authority even where the world observed no record.
    #[test]
    fn descriptor_shape_answers_without_sources() {
        let program = parsed_program(PAGE_SCHEMA);
        let mut site = QuerySession::with_program(&program);
        let hover =
            site_hover(&mut site, &marked_before(&program, "rce.meta.draft)")).expect("a hover");
        for field in [
            "- `id: str`",
            "- `file: path`",
            "- `path: str`",
            "- `filename: str`",
            "- `route-segments: array`",
            "- `meta: dictionary`",
        ] {
            assert!(hover.contains(field), "{field}: {hover}");
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
