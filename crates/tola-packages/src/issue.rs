//! Schema Issue decoding, wrapper normalization, and diagnostic/text rendering.

use typst::ecow::EcoString;
use typst::foundations::{Array, Dict, Repr, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OriginKind {
    Default,
    Output,
    Generated,
}

impl OriginKind {
    fn from_code(code: &str) -> Option<Self> {
        match code {
            "schema.default" => Some(Self::Default),
            "schema.output" => Some(Self::Output),
            _ => None,
        }
    }

    fn summary(kinds: &[Self]) -> Self {
        if kinds.iter().all(|k| *k == Self::Default) {
            Self::Default
        } else if kinds.iter().all(|k| *k == Self::Output) {
            Self::Output
        } else {
            Self::Generated
        }
    }

    pub(crate) fn value_phrase(self) -> &'static str {
        match self {
            Self::Default => "schema's default value",
            Self::Output => "converted value",
            Self::Generated => "schema-generated value",
        }
    }

    fn wire_kind(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Output => "output",
            Self::Generated => "generated",
        }
    }

    pub(crate) fn join(outer: Self, inner: Self) -> Self {
        if outer == inner {
            outer
        } else {
            Self::Generated
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Origin {
    pub kind: OriginKind,
    pub path: Vec<Value>,
}

impl Origin {
    fn encode(&self) -> Value {
        Value::Dict(crate::dict([
            ("kind", Value::Str(self.kind.wire_kind().into())),
            ("path", Value::Array(self.path.iter().cloned().collect())),
        ]))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Issue {
    pub code: EcoString,
    pub path: Vec<Value>,
    pub message: EcoString,
    pub notes: Vec<EcoString>,
    pub children: Vec<Issue>,
    pub origin: Option<Origin>,
}

impl Issue {
    pub(crate) fn decode(val: &Value) -> Result<Self, EcoString> {
        let Value::Dict(issue) = val else {
            return Err("expected an Issue dictionary".into());
        };
        if issue.len() != 5 && issue.len() != 6 {
            return Err("expected an Issue with five fields and an optional `origin`".into());
        }
        let path = array_field(issue, "path")?;
        validate_path(path.as_slice())?;
        let origin = match issue.get("origin") {
            Ok(Value::None) | Err(_) => None,
            Ok(Value::Dict(origin)) => Some(decode_origin(origin)?),
            _ => return Err("Issue `origin` must be `none` or a dictionary".into()),
        };
        Ok(Self {
            code: str_field(issue, "code")?,
            path: path.iter().cloned().collect(),
            message: str_field(issue, "message")?,
            notes: array_field(issue, "notes")?
                .iter()
                .map(|n| match n {
                    Value::Str(s) => Ok(s.as_str().into()),
                    _ => Err("Issue notes must be strings".into()),
                })
                .collect::<Result<Vec<EcoString>, EcoString>>()?,
            children: array_field(issue, "children")?
                .iter()
                .map(Self::decode)
                .collect::<Result<_, _>>()?,
            origin,
        })
    }

    pub(crate) fn encode(&self) -> Value {
        Value::Dict(crate::dict([
            ("code", Value::Str(self.code.as_str().into())),
            ("path", Value::Array(self.path.iter().cloned().collect())),
            ("message", Value::Str(self.message.as_str().into())),
            (
                "notes",
                Value::Array(
                    self.notes
                        .iter()
                        .map(|n| Value::Str(n.as_str().into()))
                        .collect(),
                ),
            ),
            (
                "children",
                Value::Array(self.children.iter().map(Self::encode).collect()),
            ),
            (
                "origin",
                self.origin.as_ref().map_or(Value::None, Origin::encode),
            ),
        ]))
    }

    fn generated(&self) -> bool {
        OriginKind::from_code(&self.code).is_some()
    }

    pub(crate) fn sentence(
        &self,
        field: &[Value],
        kind: OriginKind,
        origin_path: &[Value],
    ) -> EcoString {
        let field = format!("`{}`", issue_path(field));
        let built_in = self.code.starts_with("schema.");
        let wrapper = self.code == "schema.default" || self.code == "schema.output";
        let mut out: EcoString = match kind {
            OriginKind::Output if built_in => {
                format!("{field} was converted by the schema, but the result is invalid")
            }
            OriginKind::Output => format!("{field} was converted by the schema"),
            OriginKind::Generated if built_in => {
                format!("{field} was generated by the schema, but the value is invalid")
            }
            OriginKind::Generated => format!("{field} was generated by the schema"),
            OriginKind::Default if built_in => {
                format!("{field} is not set, and the schema's default value is invalid")
            }
            OriginKind::Default => format!("{field} is not set"),
        }
        .into();
        if !origin_path.is_empty() {
            if !built_in {
                let clause = match kind {
                    OriginKind::Output => ", and the result fails",
                    OriginKind::Generated => ", and the schema-generated value fails",
                    OriginKind::Default => ", and the schema's default fails",
                };
                out.push_str(clause);
            }
            out.push_str(&format!(" at `{}`", issue_path(origin_path)));
        }
        if !wrapper {
            out.push_str(": ");
            out.push_str(self.message.as_str());
        }
        out
    }
}

/// Folds generated-value wrappers into `origin` while preserving causes and notes.
pub(crate) fn normalize(issues: &[Issue]) -> Vec<Issue> {
    let mut out = Vec::new();
    for issue in issues {
        if issue.generated() && issue.origin.is_none() {
            collect_generated(issue, &issue.path, &[], &[], &mut out);
        } else {
            out.push(copy_issue(issue, &issue.path, issue.origin.clone()));
        }
    }
    out
}

pub(crate) fn format(issues: &[Issue]) -> String {
    let mut lines = Vec::new();
    for issue in &normalize(issues) {
        render(issue, &[], &Region::root(), "", &mut lines);
    }
    lines.join("\n")
}

#[derive(Debug, Clone)]
pub(crate) struct Region {
    field: Option<Vec<Value>>,
    kind: OriginKind,
    prefix: Vec<Value>,
}

impl Region {
    pub(crate) fn root() -> Self {
        Self {
            field: None,
            kind: OriginKind::Default,
            prefix: Vec::new(),
        }
    }

    pub(crate) fn generated(field: Vec<Value>, kind: OriginKind) -> Self {
        Self {
            field: Some(field),
            kind,
            prefix: Vec::new(),
        }
    }

    pub(crate) fn is_generated(&self) -> bool {
        self.field.is_some()
    }

    pub(crate) fn position(&self, path: &[Value]) -> String {
        let Some(field) = &self.field else {
            return format!("`{}`", issue_path(path));
        };
        let full = [self.prefix.as_slice(), path].concat();
        let val = self.kind.value_phrase();
        if full.is_empty() {
            format!("the {val} for `{}`", issue_path(field))
        } else {
            format!(
                "`{}` in the {val} for `{}`",
                issue_path(&full),
                issue_path(field)
            )
        }
    }
}

pub(crate) struct Description {
    pub(crate) field: Vec<Value>,
    pub(crate) kind: OriginKind,
    pub(crate) origin_path: Vec<Value>,
    pub(crate) child: Region,
}

pub(crate) fn describe(issue: &Issue, path: &[Value], region: &Region) -> Option<Description> {
    let origin = issue.origin.as_ref()?;
    let full = [path, &issue.path].concat();
    Some(match &region.field {
        None => Description {
            field: full.clone(),
            kind: origin.kind,
            origin_path: origin.path.clone(),
            child: Region {
                field: Some(full),
                kind: origin.kind,
                prefix: Vec::new(),
            },
        },
        Some(field) => {
            let kind = OriginKind::join(region.kind, origin.kind);
            Description {
                field: field.clone(),
                kind,
                origin_path: [region.prefix.as_slice(), &full, &origin.path].concat(),
                child: Region {
                    field: Some(field.clone()),
                    kind,
                    prefix: [region.prefix.as_slice(), &full].concat(),
                },
            }
        }
    })
}

fn render(issue: &Issue, path: &[Value], region: &Region, indent: &str, lines: &mut Vec<String>) {
    let (head, child_path, child_region) = match describe(issue, path, region) {
        Some(desc) => (
            issue
                .sentence(&desc.field, desc.kind, &desc.origin_path)
                .to_string(),
            Vec::new(),
            desc.child,
        ),
        None => {
            let full = [path, &issue.path].concat();
            (
                format!("{}: {}", region.position(&full), issue.message),
                full,
                region.clone(),
            )
        }
    };
    lines.push(format!("{indent}{head}"));
    for note in &issue.notes {
        lines.push(format!("{indent}  note: {note}"));
    }
    let next_indent = format!("{indent}  ");
    for child in &issue.children {
        render(child, &child_path, &child_region, &next_indent, lines);
    }
}

fn collect_generated(
    issue: &Issue,
    field: &[Value],
    prefix: &[Value],
    kinds: &[OriginKind],
    out: &mut Vec<Issue>,
) {
    let kinds = [
        kinds,
        &[OriginKind::from_code(&issue.code).expect("generated")],
    ]
    .concat();
    let kind = OriginKind::summary(&kinds);
    if issue.children.is_empty() {
        out.push(copy_issue(
            issue,
            field,
            Some(Origin {
                kind,
                path: prefix.to_vec(),
            }),
        ));
        return;
    }
    for child in &issue.children {
        let path = [prefix, &child.path].concat();
        if child.generated() {
            collect_generated(child, field, &path, &kinds, out);
        } else {
            out.push(copy_issue(child, field, Some(Origin { kind, path })));
        }
    }
}

fn copy_issue(issue: &Issue, path: &[Value], origin: Option<Origin>) -> Issue {
    Issue {
        code: issue.code.clone(),
        path: path.to_vec(),
        message: issue.message.clone(),
        notes: issue.notes.clone(),
        children: normalize(&issue.children),
        origin,
    }
}

pub(crate) fn issue_path(path: &[Value]) -> String {
    let mut out = "$".to_owned();
    for seg in path {
        match seg {
            Value::Int(idx) => out.push_str(&format!("[{idx}]")),
            Value::Str(key)
                if !key.is_empty()
                    && key.chars().enumerate().all(|(i, c)| {
                        c.is_ascii_alphabetic()
                            || c == '_'
                            || (i > 0 && (c.is_ascii_digit() || c == '-'))
                    }) =>
            {
                out.push('.');
                out.push_str(key.as_str());
            }
            Value::Str(key) => {
                out.push_str(&format!(
                    "[{}]",
                    key.repr().as_str().replace('`', "\\u{60}")
                ));
            }
            _ => unreachable!("Issue path segments were checked on decode"),
        }
    }
    out
}

fn decode_origin(origin: &Dict) -> Result<Origin, EcoString> {
    let kind = match str_field(origin, "kind")?.as_str() {
        "default" => OriginKind::Default,
        "output" => OriginKind::Output,
        "generated" => OriginKind::Generated,
        other => return Err(format!("unknown Issue origin kind `{other}`").into()),
    };
    let path = array_field(origin, "path")?;
    validate_path(path.as_slice())?;
    Ok(Origin {
        kind,
        path: path.iter().cloned().collect(),
    })
}

fn validate_path(path: &[Value]) -> Result<(), EcoString> {
    if path
        .iter()
        .all(|s| matches!(s, Value::Str(_)) || matches!(s, Value::Int(i) if *i >= 0))
    {
        Ok(())
    } else {
        Err("Issue paths contain string keys or nonnegative indices".into())
    }
}

fn str_field(dict: &Dict, key: &str) -> Result<EcoString, EcoString> {
    crate::dict_str(dict, key)
        .map(Into::into)
        .ok_or_else(|| format!("Issue `{key}` must be a string").into())
}

fn array_field<'a>(dict: &'a Dict, key: &str) -> Result<&'a Array, EcoString> {
    crate::dict_array(dict, key).ok_or_else(|| format!("Issue `{key}` must be an array").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(val: &str) -> Value {
        Value::Str(val.into())
    }

    fn issue(code: &str, path: Vec<Value>, msg: &str) -> Issue {
        Issue {
            code: code.into(),
            path,
            message: msg.into(),
            notes: Vec::new(),
            children: Vec::new(),
            origin: None,
        }
    }

    fn wrapper(code: &str, path: Vec<Value>, children: Vec<Issue>) -> Issue {
        Issue {
            code: code.into(),
            path,
            message: "generated value".into(),
            notes: Vec::new(),
            children,
            origin: None,
        }
    }

    #[test]
    fn normalize_default_origin() {
        let cause = issue(
            "schema.type",
            vec![key("count")],
            "expected `int`, found `str`",
        );
        let raw = wrapper("schema.default", vec![key("rows")], vec![cause]);

        let out = normalize(&[raw]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code, "schema.type");
        assert_eq!(out[0].path, vec![key("rows")]);
        assert_eq!(
            out[0].origin,
            Some(Origin {
                kind: OriginKind::Default,
                path: vec![key("count")],
            })
        );
        assert!(out[0].children.is_empty());
    }

    #[test]
    fn normalize_nested_origin() {
        let cause = issue("schema.type", vec![], "expected `int`, found `str`");
        let inner = wrapper("schema.default", vec![key("count")], vec![cause]);
        let outer = wrapper("schema.default", vec![key("value")], vec![inner]);

        let out = normalize(&[outer]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path, vec![key("value")]);
        let origin = out[0].origin.as_ref().expect("generated");
        assert_eq!(origin.kind, OriginKind::Default);
        assert_eq!(origin.path, vec![key("count")]);
    }

    #[test]
    fn format_nested_issues() {
        let mut parent = issue("custom", vec![], "outer");
        parent.children.push(issue(
            "schema.type",
            vec![key("inner")],
            "expected `int`, found `str`",
        ));

        assert_eq!(
            format(&[parent]),
            "`$`: outer\n  `$.inner`: expected `int`, found `str`"
        );
    }
}
