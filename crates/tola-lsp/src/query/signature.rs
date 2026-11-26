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
        prep: &Source,
        sem: &mut Semantic<'_>,
    ) -> Result<Option<SignatureHelp>> {
        let SelectedSyntax::Call(call) = &self.syntax else {
            return Ok(None);
        };
        let name = &self.source.text()[call.callee.clone()];
        let callee_node = tola_typst_syntax::syntax::node_at_range(prep, &call.callee);
        let (mut sigs, params) = match &call.target {
            tola_typst_syntax::syntax::CallTarget::Function => {
                let Some(callee) = &callee_node else {
                    return Ok(None);
                };
                sem.signatures(callee, name)?
            }
            tola_typst_syntax::syntax::CallTarget::Field { receiver, field } => {
                let Some(receiver) = tola_typst_syntax::syntax::node_at_range(prep, receiver)
                else {
                    return Ok(None);
                };
                sem.field_signatures(&receiver, &self.source.text()[field.clone()], name)?
            }
        };
        if sigs.is_empty() {
            return Ok(None);
        }
        let rest = params.iter().position(|p| p.kind == ParameterKind::Rest);
        let written = if let Some(written) = &call.argument_name {
            params
                .iter()
                .position(|p| {
                    p.kind == ParameterKind::Named && p.name == self.source.text()[written.clone()]
                })
                .or(rest)
        } else {
            let positional = callee_node
                .as_ref()
                .and_then(|callee| callee.parent().cloned())
                .and_then(|node| {
                    let call_ast = node.cast::<ast::FuncCall>()?;
                    let mut pos = 0;
                    for arg in call_ast.args().items().take(call.active_argument) {
                        match arg {
                            ast::Arg::Pos(_) => pos += 1,
                            ast::Arg::Named(_) => {}
                            ast::Arg::Spread(_) => return None,
                        }
                    }
                    Some(pos)
                });
            positional.and_then(|pos| {
                params
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.kind == ParameterKind::Positional)
                    .nth(pos)
                    .map(|(idx, _)| idx)
                    .or(rest)
            })
        };
        sigs[0].active_parameter = written
            .map(u32::try_from)
            .transpose()
            .context("active parameter exceeds LSP index range")?;
        Ok(Some(SignatureHelp {
            signatures: sigs,
            active_signature: Some(0),
            active_parameter: None,
        }))
    }
}

#[cfg(test)]
mod tests {
    use crate::query::tests::*;
    use lsp_types::{ParameterLabel, SignatureHelp};

    fn sig_of(src: &str) -> SignatureHelp {
        QuerySession::new().signature(src).expect("a signature")
    }

    #[test]
    fn callee_signature_labels() {
        let cases: [(&str, &str, &[&str], Option<&str>); 8] = [
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
            ("#text(|\"\")\n", "text(", &["text"], None),
        ];
        for (src, prefix, fragments, declared) in cases {
            let sig = sig_of(src);
            let first = &sig.signatures[0];
            assert!(first.label.starts_with(prefix), "{src:?}: {first:?}");
            for fragment in fragments {
                assert!(first.label.contains(fragment), "{src:?}: {first:?}");
            }
            if let Some(declared) = declared {
                let params = first.parameters.as_ref().expect("declared parameters");
                assert!(
                    params.iter().any(|p| match &p.label {
                        ParameterLabel::Simple(label) => label.contains(declared),
                        ParameterLabel::LabelOffsets(offs) => {
                            first.label[offs[0] as usize..offs[1] as usize].contains(declared)
                        }
                    }),
                    "{src:?}: {first:?}"
                );
            }
        }
    }

    #[test]
    fn datalog_query_required_named_arrays() {
        let sig = sig_of(
            "#import \"@tola/datalog:0.0.0\": database\n#let db = database(())\n#db.query(|)",
        );
        let query = &sig.signatures[0];
        assert!(
            query
                .label
                .ends_with("query(select: array, where: array) -> array")
        );
        for param in query.parameters.as_ref().unwrap() {
            let Some(lsp_types::Documentation::MarkupContent(docs)) = &param.documentation else {
                panic!("query parameter has no documentation");
            };
            assert!(docs.value.contains("Required."));
        }
    }

    #[test]
    fn active_parameter_selection() {
        for (src, expected) in [
            ("#text(size: 10pt, |\"hello\")\n", Some("text:")),
            ("#let f(a: 1, b) = b\n#f(a: 2, |\"x\")\n", Some("b:")),
            ("#let f(a, ..xs) = a\n#f(1, size: 2, |3)\n", Some("xs:")),
            (
                "#let size(body, width: 0pt, height: 0pt) = body\n#size(|width: 1pt)\n",
                Some("width:"),
            ),
            ("#let f(size: 1) = size\n#f(|2)\n", None),
            ("#let f(a, b) = a\n#f(..(1,), |2)\n", None),
            ("#let f(size: 1) = size\n#f(|unknown: 2)\n", None),
            ("#let f(a, b: 1) = a\n#f(|a: 2)\n", None),
        ] {
            let sig = sig_of(src);
            let first = &sig.signatures[0];
            match expected {
                Some(label) => {
                    let selected = first.active_parameter.expect("a selected parameter");
                    assert_eq!(
                        first.parameters.as_ref().unwrap()[selected as usize].label,
                        ParameterLabel::Simple(label.to_owned()),
                        "{src}"
                    );
                }
                None => assert_eq!(first.active_parameter, None, "{src}"),
            }
        }
    }
}
