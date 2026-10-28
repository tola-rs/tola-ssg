//! The signature one call stands in.

use anyhow::{Context, Result};
use lsp_types::SignatureHelp;
use tola_packages::ParameterKind;
use tola_typst::typst::syntax::{Source, ast};

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
        let (mut signatures, parameters) = match &call.target {
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
        let rest = parameters
            .iter()
            .position(|parameter| parameter.kind == ParameterKind::Rest);
        let written = if let Some(written) = &call.argument_name {
            parameters
                .iter()
                .position(|parameter| {
                    parameter.kind == ParameterKind::Named
                        && parameter.name == self.source.text()[written.clone()]
                })
                .or(rest)
        } else {
            let positional = tola_typst_syntax::syntax::node_at_range(prepared, &call.callee)
                .and_then(|callee| callee.parent().cloned())
                .and_then(|node| {
                    let invocation = node.cast::<ast::FuncCall>()?;
                    let mut positional = 0;
                    for argument in invocation.args().items().take(call.active_argument) {
                        match argument {
                            ast::Arg::Pos(_) => positional += 1,
                            ast::Arg::Named(_) => {}
                            ast::Arg::Spread(_) => return None,
                        }
                    }
                    Some(positional)
                });
            positional.and_then(|positional| {
                parameters
                    .iter()
                    .enumerate()
                    .filter(|(_, parameter)| parameter.kind == ParameterKind::Positional)
                    .nth(positional)
                    .map(|(index, _)| index)
                    .or(rest)
            })
        };
        signatures[0].active_parameter = written
            .map(u32::try_from)
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
                "#import \"@tola/address:0.0.0\": output-of as path\n    #path(|\"/hello/\")\n",
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
    #[test]
    fn positional_arguments_skip_named_arguments() {
        let mut site = QuerySession::new();
        for (source, expected) in [
            ("#text(size: 10pt, |\"hello\")\n", "text:"),
            ("#let f(a: 1, b) = b\n#f(a: 2, |\"x\")\n", "b:"),
            ("#let f(a, ..xs) = a\n#f(1, size: 2, |3)\n", "xs:"),
        ] {
            let signature = site.signature(source).expect("a signature");
            let first = &signature.signatures[0];
            let selected = first.active_parameter.expect("a selected parameter");
            assert_eq!(
                first.parameters.as_ref().unwrap()[selected as usize].label,
                ParameterLabel::Simple(expected.to_owned()),
                "{source}"
            );
        }
    }

    #[test]
    fn unknown_positions_mark_no_parameter() {
        let mut site = QuerySession::new();
        for source in [
            "#let f(size: 1) = size\n#f(|2)\n",
            "#let f(a, b) = a\n#f(..(1,), |2)\n",
            "#let f(size: 1) = size\n#f(|unknown: 2)\n",
            "#let f(a, b: 1) = a\n#f(|a: 2)\n",
        ] {
            let signature = site.signature(source).expect("a signature");
            assert_eq!(signature.signatures[0].active_parameter, None, "{source}");
        }
    }

    /// A named argument marks the parameter it names, not the position it was written at.
    #[test]
    fn named_argument_marks_its_own_parameter() {
        let mut site = QuerySession::new();
        let signature = site
            .signature("#let size(body, width: 0pt, height: 0pt) = body\n#size(|width: 1pt)\n")
            .expect("a signature");
        let help = &signature.signatures[0];
        let parameters = help.parameters.as_ref().expect("parameters");
        let marked = help.active_parameter.expect("a marked parameter");
        assert_eq!(
            parameters[marked as usize].label,
            ParameterLabel::Simple("width:".to_owned())
        );
    }

    /// A signature names the parameters a local declaration takes, each labelled by its own name.
    #[test]
    fn local_call_labels_its_signature() {
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
    }

    /// A library call's signature names the element's own type as the value it returns.
    #[test]
    fn library_call_labels_its_return() {
        let mut site = QuerySession::new();
        let signature = site.signature("#text(|\"\")\n").expect("a signature");
        let label = &signature.signatures[0].label;
        assert!(
            label.starts_with("text(text: str,") && label.ends_with(") -> text"),
            "{label}"
        );
    }
}
