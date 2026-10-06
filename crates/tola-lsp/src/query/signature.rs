//! The signature one call stands in.

use anyhow::{Context, Result};
use lsp_types::SignatureHelp;
use tola_typst::typst::syntax::Source;

use super::context::{SelectedSyntax, Selection};
use super::semantic::Semantic;

impl Selection<'_> {
    pub(super) fn signature_help(
        &self,
        prepared: &Source,
        semantics: &mut Semantic<'_>,
    ) -> Result<Option<SignatureHelp>> {
        let SelectedSyntax::Call(call) = &self.syntax else {
            return Ok(None);
        };
        let name = &self.source.text()[call.callee.clone()];
        let mut signatures = match &call.target {
            tola_typst_syntax::syntax::CallTarget::Function => {
                let Some(callee) = tola_typst_syntax::syntax::node_at_range(prepared, &call.callee)
                else {
                    return Ok(None);
                };
                semantics.signatures(&callee, name)?
            }
            tola_typst_syntax::syntax::CallTarget::Field { receiver, field } => {
                let Some(receiver) = tola_typst_syntax::syntax::node_at_range(prepared, receiver)
                else {
                    return Ok(None);
                };
                semantics.field_signatures(&receiver, &self.source.text()[field.clone()], name)?
            }
        };
        if signatures.is_empty() {
            return Ok(None);
        }
        // The parameter the call stands in belongs to the signature the editor highlights, which
        // is where the client reads it.
        let parameter_count = signatures[0].parameters.as_ref().map_or(0, Vec::len);
        signatures[0].active_parameter = (parameter_count > 0)
            .then(|| u32::try_from(call.active_argument.min(parameter_count - 1)))
            .transpose()
            .context("active parameter exceeds LSP index range")?;
        Ok(Some(SignatureHelp {
            signatures,
            active_signature: Some(0),
            active_parameter: None,
        }))
    }
}

#[cfg(test)]
mod tests {
    use crate::query::tests::*;
    use lsp_types::ParameterLabel;

    /// A call reports the signature of the function it names, however the callee resolves: a
    /// standard-library call, a module member, a show rule, a method, a local function, an aliased
    /// import, and a callee that is itself a call.
    #[test]
    fn calls_report_their_signature() {
        let cases: [(&str, &str, &[&str], Option<&str>); 7] = [
            ("#align(|center)[Body]", "align(", &["alignment"], None),
            ("#(math.underline(|))\n", "math.underline(", &[], None),
            (
                "#show list.item.where(|): it => it\n",
                "list.item.where(",
                &[],
                None,
            ),
            ("#\"text\".slice(|1)", "\"text\".slice(", &[], None),
            (
                "#let write(body, depth: 0) = body\n#write(|)\n",
                "write(",
                &["depth"],
                None,
            ),
            (
                "#import \"@tola/address:0.0.0\": route-to-output as path\n    #path(|\"/hello/\")\n",
                "path(",
                &["str"],
                Some("route"),
            ),
            (
                "#let twice(f, x) = f(f(x))\n#twice(|x => x)",
                "twice(",
                &[],
                None,
            ),
        ];
        for (source, prefix, fragments, declared) in cases {
            let mut site = QuerySession::new();
            let signature = site.signature(source).expect("a signature");
            let first = &signature.signatures[0];
            // The label begins with the spelling the author wrote, so an alias cannot answer as
            // the name it imports.
            assert!(first.label.starts_with(prefix), "{source:?}: {first:?}");
            for fragment in fragments {
                assert!(first.label.contains(fragment), "{source:?}: {first:?}");
            }
            if let Some(declared) = declared {
                let parameters = first.parameters.as_ref().expect("declared parameters");
                assert!(
                    parameters.iter().any(|parameter| match &parameter.label {
                        ParameterLabel::Simple(label) => label.contains(declared),
                        ParameterLabel::LabelOffsets(offsets) => first.label
                            [offsets[0] as usize..offsets[1] as usize]
                            .contains(declared),
                    }),
                    "{source:?}: {first:?}"
                );
            }
        }
    }
    /// The parameter the call stands in is marked on the signature the editor highlights.
    #[test]
    fn signature_marks_the_active_argument() {
        let mut site = QuerySession::new();
        let signature = site
            .signature("#strong(delta: 300, |\"\")\n")
            .expect("a signature");
        assert_eq!(signature.signatures[0].active_parameter, Some(1));
    }

    /// A call's signature help reads as the shared model renders it: one line naming each
    /// parameter's type, each parameter labelled by its own name, and an element's own type as
    /// the value it returns.
    #[test]
    fn signature_help_names_its_declaration_label() {
        let mut site = QuerySession::new();
        let signature = site
            .signature("#let write(body, depth: 0) = body\n#write(|\"\")\n")
            .expect("a signature");
        let first = &signature.signatures[0];
        assert_eq!(first.label, "write(body: any, depth: any)");
        assert_eq!(
            first.parameters.as_ref().map(|parameters| parameters
                .iter()
                .map(|parameter| match &parameter.label {
                    ParameterLabel::Simple(label) => label.as_str(),
                    ParameterLabel::LabelOffsets(_) => "",
                })
                .collect::<Vec<_>>()),
            Some(vec!["body:", "depth:"])
        );

        let signature = site.signature("#text(|\"\")\n").expect("a signature");
        let label = &signature.signatures[0].label;
        assert!(
            label.starts_with("text(text: str,") && label.ends_with(") -> text"),
            "{label}"
        );
    }
}
