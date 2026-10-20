//! The Issue a site author reads: decoded, normalized, and rendered in one place.
//!
//! The schema combinators evaluate in Typst and describe failures in a tree whose generated
//! values are wrapper nodes. That shape is convenient while evaluating but leaks a coordinate
//! system no site author should meet. This module folds the wrappers into an explicit `origin`,
//! and renders the same sentences for `format-issues` and the compiler diagnostic.

use typst::ecow::EcoString;
use typst::foundations::{Array, Dict, Repr, Value};

/// How a generated value came to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OriginKind {
    /// The object key was absent and `optional` supplied its declared default.
    Default,
    /// `map` or `convert` produced the value, which the output schema then rejected.
    Output,
    /// Generated values of more than one kind produced the value.
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
        if kinds.iter().all(|kind| *kind == Self::Default) {
            Self::Default
        } else if kinds.iter().all(|kind| *kind == Self::Output) {
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

/// The generated value an issue's message applies to.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Origin {
    /// How the value was generated.
    pub kind: OriginKind,
    /// Path inside the generated value to the failing value.
    pub path: Vec<Value>,
}

/// One failure, as a site author reads it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Issue {
    pub code: EcoString,
    /// Path relative to the parent value.
    pub path: Vec<Value>,
    pub message: EcoString,
    pub notes: Vec<EcoString>,
    pub children: Vec<Issue>,
    /// `none` when the input supplied the value; otherwise how the schema generated it.
    pub origin: Option<Origin>,
}

impl Issue {
    /// Decode one Issue without interpreting its wrappers.
    pub(crate) fn decode(value: &Value) -> Result<Self, EcoString> {
        let Value::Dict(issue) = value else {
            return Err("expected an Issue dictionary".into());
        };
        if issue.len() != 5 && issue.len() != 6 {
            return Err("expected an Issue with five fields and an optional `origin`".into());
        }
        let path = array(issue, "path")?;
        validate_path(path.as_slice())?;
        let origin = match issue.get("origin") {
            Ok(Value::None) | Err(_) => None,
            Ok(Value::Dict(origin)) => Some(decode_origin(origin)?),
            _ => return Err("Issue `origin` must be `none` or a dictionary".into()),
        };
        Ok(Self {
            code: string(issue, "code")?,
            path: path.iter().cloned().collect(),
            message: string(issue, "message")?,
            notes: array(issue, "notes")?
                .iter()
                .map(|note| match note {
                    Value::Str(note) => Ok(note.as_str().into()),
                    _ => Err("Issue notes must be strings".into()),
                })
                .collect::<Result<Vec<EcoString>, EcoString>>()?,
            children: array(issue, "children")?
                .iter()
                .map(Self::decode)
                .collect::<Result<_, _>>()?,
            origin,
        })
    }

    /// Encode one Issue as the `@tola/schema` package spells it.
    pub(crate) fn encode(&self) -> Value {
        let mut issue = Dict::new();
        issue.insert("code".into(), Value::Str(self.code.as_str().into()));
        issue.insert(
            "path".into(),
            Value::Array(self.path.iter().cloned().collect()),
        );
        issue.insert("message".into(), Value::Str(self.message.as_str().into()));
        issue.insert(
            "notes".into(),
            Value::Array(
                self.notes
                    .iter()
                    .map(|note| Value::Str(note.as_str().into()))
                    .collect(),
            ),
        );
        issue.insert(
            "children".into(),
            Value::Array(self.children.iter().map(Self::encode).collect()),
        );
        issue.insert(
            "origin".into(),
            match &self.origin {
                Some(origin) => origin.encode(),
                None => Value::None,
            },
        );
        Value::Dict(issue)
    }

    fn generated(&self) -> bool {
        OriginKind::from_code(&self.code).is_some()
    }

    /// The sentence a site author reads. `field` names the generated value, and `origin_path`
    /// locates the failure inside it.
    pub(crate) fn sentence(
        &self,
        field: &[Value],
        kind: OriginKind,
        origin_path: &[Value],
    ) -> EcoString {
        let field = format!("`{}`", issue_path(field));
        let built_in = self.code.starts_with("schema.");
        let wrapper = self.code == "schema.default" || self.code == "schema.output";
        let mut sentence: EcoString = match kind {
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
                sentence.push_str(clause);
            }
            sentence.push_str(&format!(" at `{}`", issue_path(origin_path)));
        }
        // A wrapper with no cause keeps its message in the data; the sentence already says that
        // the generated value is invalid.
        if !wrapper {
            sentence.push_str(": ");
            sentence.push_str(self.message.as_str());
        }
        sentence
    }
}

/// Fold generated-value wrappers into `origin`, preserving every cause and note.
pub(crate) fn normalize(issues: &[Issue]) -> Vec<Issue> {
    let mut normalized = Vec::new();
    for issue in issues {
        if issue.generated() && issue.origin.is_none() {
            collect_generated(issue, &issue.path, &[], &[], &mut normalized);
        } else {
            normalized.push(copy_issue(issue, &issue.path, issue.origin.clone()));
        }
    }
    normalized
}

/// Render Issues to the deterministic text `format-issues` returns.
pub(crate) fn format(issues: &[Issue]) -> String {
    let normalized = normalize(issues);
    let mut lines = Vec::new();
    for issue in &normalized {
        render(issue, &[], &Region::root(), "", &mut lines);
    }
    lines.join("\n")
}

/// Where an issue's path is measured.
#[derive(Debug, Clone)]
pub(crate) struct Region {
    /// The generated value's field path, or `none` at the root.
    field: Option<Vec<Value>>,
    /// The generated value's kind, meaningful when `field` is `some`.
    kind: OriginKind,
    /// Path from the generated value's root to the current parent value.
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

    /// Whether paths are measured inside a generated value.
    pub(crate) fn is_generated(&self) -> bool {
        self.field.is_some()
    }

    /// How a hinted issue at `path` reads.
    pub(crate) fn position(&self, path: &[Value]) -> String {
        let Some(field) = &self.field else {
            return format!("`{}`", issue_path(path));
        };
        let full = [self.prefix.as_slice(), path].concat();
        let value = self.kind.value_phrase();
        if full.is_empty() {
            format!("the {value} for `{}`", issue_path(field))
        } else {
            format!(
                "`{}` in the {value} for `{}`",
                issue_path(&full),
                issue_path(field)
            )
        }
    }
}

/// The sentence one generated-value issue reads, and the region its children are measured in.
pub(crate) struct Description {
    pub(crate) field: Vec<Value>,
    pub(crate) kind: OriginKind,
    pub(crate) origin_path: Vec<Value>,
    pub(crate) child: Region,
}

/// Describe `issue` at `path` inside `region`, when the schema generated its value.
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
    let full = [path, &issue.path].concat();
    if let Some(description) = describe(issue, path, region) {
        lines.push(format!(
            "{indent}{}",
            issue.sentence(
                &description.field,
                description.kind,
                &description.origin_path
            )
        ));
        for note in &issue.notes {
            lines.push(format!("{indent}  note: {note}"));
        }
        for child in &issue.children {
            render(
                child,
                &[],
                &description.child,
                &format!("{indent}  "),
                lines,
            );
        }
    } else {
        lines.push(format!(
            "{indent}{}: {}",
            region.position(&full),
            issue.message
        ));
        for note in &issue.notes {
            lines.push(format!("{indent}  note: {note}"));
        }
        for child in &issue.children {
            render(child, &full, region, &format!("{indent}  "), lines);
        }
    }
}

fn collect_generated(
    issue: &Issue,
    field: &[Value],
    prefix: &[Value],
    kinds: &[OriginKind],
    normalized: &mut Vec<Issue>,
) {
    let kinds = [
        kinds,
        &[OriginKind::from_code(&issue.code).expect("generated")],
    ]
    .concat();
    let origin = OriginKind::summary(&kinds);
    let Some(first) = issue.children.first() else {
        normalized.push(copy_issue(
            issue,
            field,
            Some(Origin {
                kind: origin,
                path: prefix.to_vec(),
            }),
        ));
        return;
    };
    let first_path = [prefix, &first.path].concat();
    if first.generated() {
        collect_generated(first, field, &first_path, &kinds, normalized);
    } else {
        normalized.push(copy_issue(
            first,
            field,
            Some(Origin {
                kind: origin,
                path: first_path,
            }),
        ));
    }
    for child in &issue.children[1..] {
        let child_path = [prefix, &child.path].concat();
        if child.generated() {
            collect_generated(child, field, &child_path, &kinds, normalized);
        } else {
            normalized.push(copy_issue(
                child,
                field,
                Some(Origin {
                    kind: origin,
                    path: child_path,
                }),
            ));
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

/// One path as `format-issues` and the diagnostics spell it.
pub(crate) fn issue_path(path: &[Value]) -> String {
    let mut rendered = "$".to_owned();
    for segment in path {
        match segment {
            Value::Int(index) => rendered.push_str(&format!("[{index}]")),
            Value::Str(key)
                if key.chars().enumerate().all(|(index, ch)| {
                    ch.is_ascii_alphabetic()
                        || ch == '_'
                        || (index > 0 && (ch.is_ascii_digit() || ch == '-'))
                }) && !key.is_empty() =>
            {
                rendered.push('.');
                rendered.push_str(key.as_str());
            }
            Value::Str(key) => {
                rendered.push_str(&format!("[{}]", key.repr().replace("`", "\\u{60}")))
            }
            _ => unreachable!("Issue path segments were checked on ingress"),
        }
    }
    rendered
}

fn decode_origin(origin: &Dict) -> Result<Origin, EcoString> {
    let kind = match string(origin, "kind")?.as_str() {
        "default" => OriginKind::Default,
        "output" => OriginKind::Output,
        "generated" => OriginKind::Generated,
        other => return Err(format!("unknown Issue origin kind `{other}`").into()),
    };
    let path = array(origin, "path")?;
    validate_path(path.as_slice())?;
    Ok(Origin {
        kind,
        path: path.iter().cloned().collect(),
    })
}

fn validate_path(path: &[Value]) -> Result<(), EcoString> {
    if path.iter().all(|segment| {
        matches!(segment, Value::Str(_)) || matches!(segment, Value::Int(index) if *index >= 0)
    }) {
        Ok(())
    } else {
        Err("Issue paths contain string keys or nonnegative indices".into())
    }
}

fn string(dict: &Dict, key: &str) -> Result<EcoString, EcoString> {
    match dict.get(key) {
        Ok(Value::Str(value)) => Ok(value.as_str().into()),
        _ => Err(format!("Issue `{key}` must be a string").into()),
    }
}

fn array<'a>(dict: &'a Dict, key: &str) -> Result<&'a Array, EcoString> {
    match dict.get(key) {
        Ok(Value::Array(value)) => Ok(value),
        _ => Err(format!("Issue `{key}` must be an array").into()),
    }
}

impl Origin {
    fn encode(&self) -> Value {
        let mut origin = Dict::new();
        origin.insert("kind".into(), Value::Str(self.kind.wire_kind().into()));
        origin.insert(
            "path".into(),
            Value::Array(self.path.iter().cloned().collect()),
        );
        Value::Dict(origin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(value: &str) -> Value {
        Value::Str(value.into())
    }

    fn issue(code: &str, path: Vec<Value>, message: &str) -> Issue {
        Issue {
            code: code.into(),
            path,
            message: message.into(),
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
    fn normalization_folds_default_into_origin() {
        let cause = issue(
            "schema.type",
            vec![key("count")],
            "expected `int`, found `str`",
        );
        let raw = wrapper("schema.default", vec![key("rows")], vec![cause]);

        let normalized = normalize(&[raw]);

        assert_eq!(normalized.len(), 1);
        let issue = &normalized[0];
        assert_eq!(issue.code, "schema.type");
        assert_eq!(issue.path, vec![key("rows")]);
        assert_eq!(
            issue.origin,
            Some(Origin {
                kind: OriginKind::Default,
                path: vec![key("count")],
            })
        );
        assert!(issue.children.is_empty());
    }

    #[test]
    fn normalization_keeps_nested_generations_in_outer_field() {
        let cause = issue("schema.type", vec![], "expected `int`, found `str`");
        let inner = wrapper("schema.default", vec![key("count")], vec![cause]);
        let outer = wrapper("schema.default", vec![key("value")], vec![inner]);

        let normalized = normalize(&[outer]);

        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized[0].path, vec![key("value")]);
        let origin = normalized[0].origin.as_ref().expect("generated");
        assert_eq!(origin.kind, OriginKind::Default);
        assert_eq!(origin.path, vec![key("count")]);
    }

    #[test]
    fn authored_causes_read_as_sentences() {
        let mut cause = issue(
            "custom",
            vec![],
            "Published blog pages need a publication date",
        );
        cause.notes.push("review metadata".into());
        let raw = wrapper("schema.default", vec![key("published")], vec![cause]);

        assert_eq!(
            format(&[raw]),
            "`$.published` is not set: Published blog pages need a publication date\n  note: review metadata"
        );
    }

    #[test]
    fn converted_values_use_the_output_voice() {
        let cause = issue(
            "schema.type",
            vec![key("count")],
            "expected `int`, found `str`",
        );
        let raw = wrapper("schema.output", vec![key("value")], vec![cause]);

        assert_eq!(
            format(&[raw]),
            "`$.value` was converted by the schema, but the result is invalid at `$.count`: expected `int`, found `str`"
        );
    }

    #[test]
    fn ordinary_issues_keep_tree() {
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

    #[test]
    fn encoding_round_trips_the_normalized_shape() {
        let cause = issue(
            "schema.type",
            vec![key("count")],
            "expected `int`, found `str`",
        );
        let raw = wrapper("schema.default", vec![key("rows")], vec![cause]);
        let normalized = normalize(&[raw]);

        let encoded = normalized[0].encode();
        let decoded = Issue::decode(&encoded).expect("encoded Issue decodes");

        assert_eq!(decoded, normalized[0]);
    }
}
