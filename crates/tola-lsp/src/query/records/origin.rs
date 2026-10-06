//! Where one record chain's declared shape comes from.

use std::ops::Range;
use std::sync::Arc;

use anyhow::Result;
use tola_packages::Signature;
use tola_typst::typst::foundations::Value;
use tola_typst::typst::syntax::ast::{self, AstNode};
use tola_typst::typst::syntax::{FileId, LinkedNode, VirtualRoot};

use super::super::context::Selection;
use super::super::imports::calls_package_member;
use super::super::schema;
use super::super::semantic::Semantic;
use super::super::site_schema::declared_schema;

/// The field chain one expression reads: its root and the fields read after it, in read order.
///
/// `None` when the expression reads no field at all.
pub(crate) fn field_chain<'a>(node: &LinkedNode<'a>) -> Option<(LinkedNode<'a>, Vec<String>)> {
    let mut fields = Vec::new();
    let mut current = node.clone();
    while let Some(access) = current.cast::<ast::FieldAccess>() {
        fields.push(access.field().get().to_string());
        current = current.find(access.target().span())?;
    }
    fields.reverse();
    (!fields.is_empty()).then_some((current, fields))
}
/// Assignment and method mutation invalidate an initializer's provenance. The array methods the
/// trace follows a callback through are read as pure; every other method call on a binding
/// conservatively leaves its origin unproved.
fn schema_binding_is_written(
    names: &tola_typst_syntax::names::SourceNames,
    declaration: usize,
) -> bool {
    let mut stack = vec![LinkedNode::new(names.source().root())];
    while let Some(node) = stack.pop() {
        let target = if let Some(binary) = node.cast::<ast::Binary>()
            && matches!(
                binary.op(),
                ast::BinOp::Assign
                    | ast::BinOp::AddAssign
                    | ast::BinOp::SubAssign
                    | ast::BinOp::MulAssign
                    | ast::BinOp::DivAssign
            ) {
            node.find(binary.lhs().span())
        } else if let Some(call) = node.cast::<ast::FuncCall>()
            && let Some(callee) = node.find(call.callee().span())
            && let Some(access) = callee.cast::<ast::FieldAccess>()
            && !matches!(access.field().as_str(), "filter" | "map")
            && !matches!(
                names.declarations()[declaration].kind,
                tola_typst_syntax::names::DeclarationKind::Import { .. }
            )
        {
            callee.find(access.target().span())
        } else {
            None
        };
        if let Some(target) = target {
            let root = tola_typst_syntax::syntax::root_expression(target);
            if names.declared_at(root.range().start) == Some(declaration) {
                return true;
            }
        }
        if let Some(assignment) = node.cast::<ast::DestructAssignment>()
            && assignment.pattern().bindings().into_iter().any(|name| {
                names
                    .source()
                    .find(name.span())
                    .is_some_and(|name| names.declared_at(name.range().start) == Some(declaration))
            })
        {
            return true;
        }
        stack.extend(node.children());
    }
    false
}

/// Whether one expression's observed values are all arrays: the evidence that a traced parameter
/// or loop variable reads its records from one receiver. An expression observed nowhere or too
/// many times pins nothing.
fn is_observed_array(node: &LinkedNode<'_>, semantics: &mut Semantic<'_>) -> Result<bool> {
    let values = semantics.values(node)?;
    Ok(!values.is_empty()
        && values.len() < tola_typst::typst::engine::Sink::MAX_VALUES
        && values
            .iter()
            .all(|(value, _)| matches!(value, Value::Array(_))))
}

/// Where one record chain's declared shape comes from.
#[derive(Clone)]
pub(in crate::query) enum RecordOrigin {
    /// The site's own source descriptors, from the native that produced them.
    Descriptors(SourceDescriptors),
    /// One `parse-sources` call's records, with the declaration that resolved them.
    Parsed {
        /// The schema argument the call resolves its records against.
        schema: SourceExpression,
        /// The descriptors the call's input records hold, when the site's own natives made them.
        descriptors: Option<SourceDescriptors>,
    },
    /// One project function's records, with the fields its body returns.
    Projected(ProjectedRecords),
}

impl RecordOrigin {
    /// The source descriptors these records hold, when the site's own natives made them.
    ///
    /// `parse-sources` admits only complete `all-sources()` records, so a call whose input records
    /// were not traced still has every descriptor.
    pub(in crate::query) fn descriptors(&self) -> Option<SourceDescriptors> {
        match self {
            Self::Descriptors(descriptors) => Some(*descriptors),
            Self::Parsed { descriptors, .. } => Some(descriptors.unwrap_or(SourceDescriptors::All)),
            Self::Projected(_) => None,
        }
    }
}

/// One expression, addressed in the source that writes it.
///
/// A record's declared shape may be declared by a file other than the one an editor asks about,
/// so the source stays addressable after the node that read the expression is gone.
#[derive(Clone)]
pub(in crate::query) struct SourceExpression {
    /// The declaring file's names, which keep its source alive.
    names: Arc<tola_typst_syntax::names::SourceNames>,
    /// The expression's bytes.
    range: Range<usize>,
}

impl SourceExpression {
    /// The node the expression spans, when its file still holds it.
    pub(in crate::query) fn node(&self) -> Option<LinkedNode<'_>> {
        tola_typst_syntax::syntax::node_at_range(self.names.source(), &self.range)
    }
}

/// The element records one project function's call returns, as its body declares them.
#[derive(Clone)]
pub(in crate::query) struct ProjectedRecords {
    /// The fields the returned record literal writes, in source order.
    pub(super) fields: Vec<ProjectedField>,
}

/// One field of a project function's returned record.
#[derive(Clone)]
pub(in crate::query) struct ProjectedField {
    /// The field's key.
    pub(super) name: String,
    /// The value the field binds, in the function's own file.
    pub(super) value: SourceExpression,
    /// What the function's body proves the value is.
    pub(super) known: Option<ProjectedValue>,
}

/// What a project function's body proves about one returned field's value.
#[derive(Clone)]
pub(in crate::query) enum ProjectedValue {
    /// A record: a chain reading through the field continues with its declared shape.
    Record(Box<RecordOrigin>),
    /// The value's declared type, from the callee's own return declaration.
    Declared(String),
}

/// The native that produced one record chain's source descriptors.
#[derive(Clone, Copy)]
pub(in crate::query) enum SourceDescriptors {
    /// `all-sources()`: every discovered source, `meta` included.
    All,
    /// `current-source()`: the file that writes the call, without `meta`.
    Current,
}

impl SourceDescriptors {
    /// The descriptor fields records from this native hold, in the descriptor's own order.
    pub(in crate::query) fn fields(self) -> Vec<tola_build::SourceDescriptorField> {
        tola_build::SourceDescriptorField::ALL
            .iter()
            .copied()
            .filter(|field| match self {
                Self::All => true,
                Self::Current => field.in_current_source(),
            })
            .collect()
    }
}

/// Follow only unchanged local bindings, a for loop's single-name variable, the single record
/// parameter of an inline native array closure, and a call to a function this site's own program
/// files declare, to the declared origin of the records one chain reads.
///
/// A `parse-sources` call declares the schema its records resolved against; the natives that make
/// the site's own descriptors name themselves. Value equality and source-file identity do not
/// establish a record's origin.
pub(in crate::query) fn record_origin<'a>(
    names: &'a Arc<tola_typst_syntax::names::SourceNames>,
    current: LinkedNode<'a>,
    semantics: &mut Semantic<'_>,
) -> Result<Option<RecordOrigin>> {
    record_origin_within(names, current, semantics, &mut Vec::new())
}

/// `followed` holds `(file, declaration)` pairs: one resolution may leave its own file to follow
/// a project function, and a declaration index is only unique inside one file.
fn record_origin_within<'a>(
    names: &'a Arc<tola_typst_syntax::names::SourceNames>,
    mut current: LinkedNode<'a>,
    semantics: &mut Semantic<'_>,
    followed: &mut Vec<(FileId, usize)>,
) -> Result<Option<RecordOrigin>> {
    loop {
        if let Some(grouped) = current.cast::<ast::Parenthesized>() {
            let Some(inner) = current.find(grouped.expr().span()) else {
                return Ok(None);
            };
            current = inner;
            continue;
        }
        if let Some(call) = current.cast::<ast::FuncCall>() {
            let Some(callee) = current.find(call.callee().span()) else {
                return Ok(None);
            };
            for (item, descriptors) in [
                (tola_packages::ALL_SOURCES, SourceDescriptors::All),
                (tola_packages::CURRENT_SOURCE, SourceDescriptors::Current),
            ] {
                if calls_package_member(names, &callee, tola_packages::SOURCE_PACKAGE, item) {
                    return Ok(Some(RecordOrigin::Descriptors(descriptors)));
                }
            }
            if calls_package_member(
                names,
                &callee,
                tola_packages::SOURCE_PACKAGE,
                tola_packages::PARSE_SOURCES,
            ) {
                let root = tola_typst_syntax::syntax::root_expression(callee);
                let Some(declaration) = names.declared_at(root.range().start) else {
                    return Ok(None);
                };
                if schema_binding_is_written(names, declaration) {
                    return Ok(None);
                }
                // `parse-sources` binds its schema positionally, so a call that names it cannot
                // build and declares no origin.
                let arguments = call.args().items().collect::<Vec<_>>();
                let [ast::Arg::Pos(records), ast::Arg::Pos(schema)] = arguments.as_slice() else {
                    return Ok(None);
                };
                let (records, schema) = (records.span(), schema.span());
                let Some(schema) = current.find(schema) else {
                    return Ok(None);
                };
                let Some(records) = current.find(records) else {
                    return Ok(None);
                };
                let descriptors = match record_origin_within(names, records, semantics, followed)? {
                    Some(RecordOrigin::Descriptors(descriptors)) => Some(descriptors),
                    Some(RecordOrigin::Parsed { descriptors, .. }) => descriptors,
                    Some(RecordOrigin::Projected(_)) | None => None,
                };
                return Ok(Some(RecordOrigin::Parsed {
                    schema: SourceExpression {
                        names: Arc::clone(names),
                        range: schema.range(),
                    },
                    descriptors,
                }));
            }
            // A call to a function the site's own program files declare answers the records its
            // body returns: the body is the declaration an author reads.
            if let Some(origin) = project_function_origin(&callee, semantics, followed)? {
                return Ok(Some(origin));
            }
            // A filter keeps the records of its receiver, so a chain over its result keeps the
            // receiver's declared origin; every other call leaves the origin unproved.
            let Some(access) = callee.cast::<ast::FieldAccess>() else {
                return Ok(None);
            };
            if access.field().as_str() != "filter" {
                return Ok(None);
            }
            let Some(receiver) = callee.find(access.target().span()) else {
                return Ok(None);
            };
            if !is_observed_array(&receiver, semantics)? {
                return Ok(None);
            }
            current = receiver;
            continue;
        }
        if current.cast::<ast::Ident>().is_none() {
            return Ok(None);
        }
        let Some(declaration) = names.declared_at(current.range().start) else {
            return Ok(None);
        };
        let file = names.source().id();
        if followed.contains(&(file, declaration)) || schema_binding_is_written(names, declaration)
        {
            return Ok(None);
        }
        followed.push((file, declaration));
        let declaration = &names.declarations()[declaration];
        let Some(mut owner) =
            tola_typst_syntax::syntax::node_at_range(names.source(), &declaration.range)
        else {
            return Ok(None);
        };
        match declaration.kind {
            tola_typst_syntax::names::DeclarationKind::Let => {
                while owner.cast::<ast::LetBinding>().is_none() {
                    let Some(parent) = owner.parent() else {
                        return Ok(None);
                    };
                    owner = parent.clone();
                }
                let binding = owner.cast::<ast::LetBinding>().unwrap();
                if !matches!(
                    binding.kind(),
                    ast::LetBindingKind::Normal(ast::Pattern::Normal(ast::Expr::Ident(_)))
                ) {
                    return Ok(None);
                }
                let Some(initializer) = binding
                    .init()
                    .and_then(|initializer| owner.find(initializer.span()))
                else {
                    return Ok(None);
                };
                current = initializer;
            }
            tola_typst_syntax::names::DeclarationKind::Parameter => {
                while owner.cast::<ast::Closure>().is_none() {
                    let Some(parent) = owner.parent() else {
                        return Ok(None);
                    };
                    owner = parent.clone();
                }
                let closure = owner.cast::<ast::Closure>().unwrap();
                let parameters = closure.params().children().collect::<Vec<_>>();
                if !matches!(
                    parameters.as_slice(),
                    [ast::Param::Pos(ast::Pattern::Normal(ast::Expr::Ident(_)))]
                ) {
                    return Ok(None);
                }
                let Some(call_node) = owner.parent().and_then(|arguments| arguments.parent())
                else {
                    return Ok(None);
                };
                let Some(call) = call_node.cast::<ast::FuncCall>() else {
                    return Ok(None);
                };
                let arguments = call.args().items().collect::<Vec<_>>();
                if !matches!(arguments.as_slice(), [ast::Arg::Pos(argument)] if argument.span() == owner.span())
                {
                    return Ok(None);
                }
                let Some(callee) = call_node.find(call.callee().span()) else {
                    return Ok(None);
                };
                let Some(access) = callee.cast::<ast::FieldAccess>() else {
                    return Ok(None);
                };
                let Some(receiver) = callee.find(access.target().span()) else {
                    return Ok(None);
                };
                if !is_observed_array(&receiver, semantics)? {
                    return Ok(None);
                }
                current = receiver;
            }
            tola_typst_syntax::names::DeclarationKind::Loop => {
                while owner.cast::<ast::ForLoop>().is_none() {
                    let Some(parent) = owner.parent() else {
                        return Ok(None);
                    };
                    owner = parent.clone();
                }
                let loop_ = owner.cast::<ast::ForLoop>().unwrap();
                if !matches!(loop_.pattern(), ast::Pattern::Normal(ast::Expr::Ident(_))) {
                    return Ok(None);
                }
                let Some(iterable) = owner.find(loop_.iterable().span()) else {
                    return Ok(None);
                };
                current = iterable;
            }
            _ => return Ok(None),
        }
    }
}

/// The origin one call to a function this site's own program files declare resolves.
///
/// The declaration is what the compiler proved the callee names: a package export names a package
/// file, and a value no declaration establishes names nothing, so both answer no origin. The body
/// answers the records it returns, or any other origin the record machinery reads from it, exactly
/// as a `parse-sources` body does.
fn project_function_origin(
    callee: &LinkedNode<'_>,
    semantics: &mut Semantic<'_>,
    followed: &mut Vec<(FileId, usize)>,
) -> Result<Option<RecordOrigin>> {
    let Some(span) = semantics.definition(callee)? else {
        return Ok(None);
    };
    let Some(file) = span.id() else {
        return Ok(None);
    };
    if !matches!(file.root(), VirtualRoot::Project) {
        return Ok(None);
    }
    let Some(range) = semantics.span_range(span) else {
        return Ok(None);
    };
    let Some(names) = semantics.source_names(file) else {
        return Ok(None);
    };
    let Some(declaration) = names.declared_at(range.start) else {
        return Ok(None);
    };
    if followed.contains(&(file, declaration)) {
        return Ok(None);
    }
    let tola_typst_syntax::names::Initializer::Function { body } =
        &names.declarations()[declaration].initializer
    else {
        return Ok(None);
    };
    let Some(body) = tola_typst_syntax::syntax::node_at_range(names.source(), body) else {
        return Ok(None);
    };
    let Some(tail) = tail_expression(body) else {
        return Ok(None);
    };
    // The declaration is pushed before the body is read, so a body that reaches its own call
    // cannot resolve it again.
    followed.push((file, declaration));
    if let Some(records) = mapped_records(&tail, &names, semantics, followed)? {
        return Ok(Some(RecordOrigin::Projected(records)));
    }
    record_origin_within(&names, tail, semantics, followed)
}

/// The expression one closure body evaluates to: the body expression itself, or a code block's
/// last expression.
fn tail_expression(body: LinkedNode<'_>) -> Option<LinkedNode<'_>> {
    let Some(block) = body.cast::<ast::CodeBlock>() else {
        return Some(body);
    };
    let code = body.find(block.body().span())?;
    let last = code.cast::<ast::Code>()?.exprs().next_back()?;
    code.find(last.span())
}

/// The element record one `.map(<parameter> => (field: …))` expression returns, when the callback
/// binds one identifier and its body is one record literal.
fn mapped_records<'a>(
    call: &LinkedNode<'a>,
    names: &'a Arc<tola_typst_syntax::names::SourceNames>,
    semantics: &mut Semantic<'_>,
    followed: &mut Vec<(FileId, usize)>,
) -> Result<Option<ProjectedRecords>> {
    let Some(func) = call.cast::<ast::FuncCall>() else {
        return Ok(None);
    };
    let Some(callee) = call.find(func.callee().span()) else {
        return Ok(None);
    };
    let Some(access) = callee.cast::<ast::FieldAccess>() else {
        return Ok(None);
    };
    if access.field().as_str() != "map" {
        return Ok(None);
    }
    let arguments = func.args().items().collect::<Vec<_>>();
    let [ast::Arg::Pos(callback)] = arguments.as_slice() else {
        return Ok(None);
    };
    let Some(callback) = call.find(callback.span()) else {
        return Ok(None);
    };
    let Some(closure) = callback.cast::<ast::Closure>() else {
        return Ok(None);
    };
    let parameters = closure.params().children().collect::<Vec<_>>();
    let [ast::Param::Pos(ast::Pattern::Normal(ast::Expr::Ident(parameter)))] =
        parameters.as_slice()
    else {
        return Ok(None);
    };
    let Some(parameter) = call.find(parameter.span()) else {
        return Ok(None);
    };
    let Some(body) = call.find(closure.body().span()) else {
        return Ok(None);
    };
    let Some(dict) = record_literal(&body) else {
        return Ok(None);
    };
    let Some(receiver) = call.find(access.target().span()) else {
        return Ok(None);
    };
    let mut fields = Vec::new();
    for item in dict.cast::<ast::Dict>().unwrap().items() {
        // A keyed or spread field leaves the record's fields unproved.
        let ast::DictItem::Named(field) = item else {
            return Ok(None);
        };
        let Some(value) = dict.find(field.expr().span()) else {
            return Ok(None);
        };
        let known = projected_field_value(
            &value,
            &parameter.range(),
            &receiver,
            names,
            semantics,
            followed,
        )?;
        fields.push(ProjectedField {
            name: field.name().get().to_string(),
            value: SourceExpression {
                names: Arc::clone(names),
                range: value.range(),
            },
            known,
        });
    }
    Ok((!fields.is_empty()).then_some(ProjectedRecords { fields }))
}

/// The record one expression writes: a dictionary, or a parenthesized dictionary.
fn record_literal<'a>(node: &LinkedNode<'a>) -> Option<LinkedNode<'a>> {
    if node.cast::<ast::Dict>().is_some() {
        return Some(node.clone());
    }
    let grouped = node.cast::<ast::Parenthesized>()?;
    let inner = node.find(grouped.expr().span())?;
    inner.cast::<ast::Dict>().is_some().then_some(inner)
}

/// What one returned field's value proves to be.
///
/// The callback's parameter has one element of the mapped records; another expression the
/// record machinery resolves keeps its own origin; a call whose callee declares a return names
/// that type.
fn projected_field_value<'a>(
    value: &LinkedNode<'a>,
    parameter: &Range<usize>,
    receiver: &LinkedNode<'a>,
    names: &'a Arc<tola_typst_syntax::names::SourceNames>,
    semantics: &mut Semantic<'_>,
    followed: &mut Vec<(FileId, usize)>,
) -> Result<Option<ProjectedValue>> {
    if value.range() == *parameter {
        return Ok(
            record_origin_within(names, receiver.clone(), semantics, followed)?
                .map(|origin| ProjectedValue::Record(Box::new(origin))),
        );
    }
    if let Some(origin) = record_origin_within(names, value.clone(), semantics, followed)? {
        return Ok(Some(ProjectedValue::Record(Box::new(origin))));
    }
    Ok(call_return_type(value, semantics)?.map(ProjectedValue::Declared))
}

/// The type one call's own declaration names for its return, when the world observed a function
/// at the callee: a package native's cast, or an element's own name.
fn call_return_type(call: &LinkedNode<'_>, semantics: &mut Semantic<'_>) -> Result<Option<String>> {
    let Some(func) = call.cast::<ast::FuncCall>() else {
        return Ok(None);
    };
    let Some(callee) = call.find(func.callee().span()) else {
        return Ok(None);
    };
    let Some((Value::Func(func), _)) = semantics.values(&callee)?.into_iter().next() else {
        return Ok(None);
    };
    Ok(Signature::of(func.name().unwrap_or_default(), &func).returns)
}
/// The record one chain's value is, when the chain reads through records.
///
/// A chain that ends at the record itself has its root's origin; one reading a field of a
/// project function's returned record continues through that field's own record.
pub(in crate::query) fn chained_origin(
    origin: &RecordOrigin,
    fields: &[String],
) -> Option<RecordOrigin> {
    let Some((field, rest)) = fields.split_first() else {
        return Some(origin.clone());
    };
    let RecordOrigin::Projected(records) = origin else {
        return None;
    };
    let projected = records
        .fields
        .iter()
        .find(|candidate| &candidate.name == field)?;
    match &projected.known {
        Some(ProjectedValue::Record(inner)) => chained_origin(inner, rest),
        _ => None,
    }
}
impl Selection<'_> {
    /// The schema of the particular record chain, observed completely at its proven parse call.
    pub(in crate::query) fn parsed_schema(
        &self,
        names: &Arc<tola_typst_syntax::names::SourceNames>,
        root: LinkedNode<'_>,
        semantics: &mut Semantic<'_>,
    ) -> Result<Option<schema::OutputDescription>> {
        let Some(RecordOrigin::Parsed { schema, .. }) = record_origin(names, root, semantics)?
        else {
            return Ok(None);
        };
        declared_schema(&schema, semantics)
    }
}

#[cfg(test)]
mod tests {
    use crate::query::tests::*;
    use lsp_types::GotoDefinitionResponse;

    #[test]
    fn observed_sources_answer_in_path_order() {
        let program = parsed_program(PAGE_SCHEMA);
        let mut site = QuerySession::with_program(&program);
        site.site.write("content/b.typ", SCHEMA_PAGE);
        site.site.write("content/a.typ", SCHEMA_PAGE);
        let Some(GotoDefinitionResponse::Array(locations)) =
            site_definition(&mut site, &marked_before(&program, "aft)"))
        else {
            panic!("expected the candidate locations");
        };
        let files = locations
            .iter()
            .map(|location| location.uri.as_str().rsplit('/').next().unwrap_or_default())
            .collect::<Vec<_>>();
        assert_eq!(files, ["a.typ", "b.typ"]);
    }

    /// A declared field answers its schema line even when the site realized no records at all.
    #[test]
    fn declared_field_answers_without_observed_records() {
        let program = parsed_program(PAGE_SCHEMA);
        let mut site = QuerySession::with_program(&program);
        let hover = site_hover(&mut site, &marked_before(&program, "aft)")).expect("a hover");
        assert!(
            hover.contains(
                "### Declared fields\n- `source.meta.draft: bool = false`\n  keeps the page out of the published site"
            ),
            "{hover}"
        );
    }

    /// A for loop's variable answers the site's descriptor shape, never a bare `any` and never
    /// the metadata schema it resolves its `meta` against.
    #[test]
    fn declared_shape_follows_loop_variable() {
        let program = parsed_program(PAGE_SCHEMA);
        let mut site = QuerySession::with_program(&program);
        let marked = program.replace("source.file", "sour|ce.file");
        let hover = site_hover(&mut site, &marked).expect("a hover");
        assert!(hover.contains("- `route-segments: array`"), "{hover}");
        assert!(!hover.contains("let source = any"), "{hover}");
        assert!(!hover.contains("the page title"), "{hover}");
    }

    /// A map closure's parameter answers the declared shape of the records it reads.
    #[test]
    fn declared_shape_follows_map_parameter() {
        let program = parsed_program(PAGE_SCHEMA).replace(
            "#for source in kept {",
            "#let titles = declared.map(entry => entry.meta.title)\n#for source in kept {",
        );
        let mut site = QuerySession::with_program(&program);
        let marked = program.replace("entry.meta.title", "entry.meta.tit|le");
        let hover = site_hover(&mut site, &marked).expect("a hover");
        assert!(
            hover.contains("- `entry.meta.title: str = \"Untitled\"`\n  the page title"),
            "{hover}"
        );
    }

    /// A filter closure's parameter answers the site's descriptor shape, never the metadata schema
    /// the records it reads resolve against.
    #[test]
    fn filter_parameter_answers_the_descriptor_shape() {
        let program = parsed_program(PAGE_SCHEMA).replace(
            "parse-sources(all-sources(), page-schema)",
            "parse-sources(all-sources().slice(0), page-schema)",
        );
        let mut site = QuerySession::with_program(&program);
        let marked = program.replace("filter(source =>", "filter(sour|ce =>");
        let hover = site_hover(&mut site, &marked).expect("a hover");
        assert!(hover.contains("- `route-segments: array`"), "{hover}");
        assert!(hover.contains("- `meta: dictionary`"), "{hover}");
        assert!(!hover.contains("the page title"), "{hover}");
    }

    /// A project function's mapped record answers its fields without any record observed.
    #[test]
    fn project_map_record_answers_without_observations() {
        let mut site = QuerySession::with_program(MAPPED_PROGRAM);
        site.site.write("site/selection.typ", &mapped_pages());
        site.site.write("site/page.typ", MAPPED_PAGE);
        let hover =
            site_hover(&mut site, &marked_before(MAPPED_PROGRAM, "entry in")).expect("a hover");
        assert!(hover.contains("- `source: dictionary`"), "{hover}");
        assert!(hover.contains("- `output: str`"), "{hover}");
    }

    /// A mapped field answers the type the package call returning it declares.
    #[test]
    fn mapped_field_answers_its_package_return() {
        let mut site = QuerySession::with_program(MAPPED_PROGRAM);
        site.site.write("site/selection.typ", &mapped_pages());
        site.site.write("site/page.typ", MAPPED_PAGE);
        let marked = marked_before(MAPPED_PAGE, "output");
        let hover = template_hover(&mut site, "site/page.typ", &marked).expect("a hover");
        assert!(hover.contains("- `output: str`"), "{hover}");
        assert!(!hover.contains("- `source: dictionary`"), "{hover}");
    }

    /// A project function's mapped record answers its fields where the world observed records.
    #[test]
    fn project_map_record_answers_with_records() {
        let mut site = QuerySession::with_program(MAPPED_PROGRAM);
        site.site.write("site/selection.typ", &mapped_pages());
        site.site.write("site/page.typ", MAPPED_PAGE);
        site.site.write("content/page.typ", SCHEMA_PAGE);
        let hover =
            site_hover(&mut site, &marked_before(MAPPED_PROGRAM, "entry in")).expect("a hover");
        assert!(hover.contains("- `source: dictionary`"), "{hover}");
    }

    /// A parameter no call ran for answers the argument shape its call site passes.
    #[test]
    fn parameter_answers_its_call_site_argument() {
        let mut site = QuerySession::with_program(MAPPED_PROGRAM);
        site.site.write("site/selection.typ", &mapped_pages());
        site.site.write("site/page.typ", MAPPED_PAGE);
        let page = template_hover(
            &mut site,
            "site/page.typ",
            &marked_before(MAPPED_PAGE, "page)"),
        )
        .expect("a hover");
        assert!(page.contains("let page = dictionary;"), "{page}");
        assert!(page.contains("- `output: str`"), "{page}");
    }

    /// A chain over a parameter no call ran for answers the records its argument parses.
    #[test]
    fn parameter_chain_answers_its_argument_schema() {
        let pages = mapped_pages();
        let mut site = QuerySession::with_program(MAPPED_PROGRAM);
        site.site.write("site/selection.typ", &pages);
        site.site.write("site/page.typ", MAPPED_PAGE);
        let shape = template_hover(
            &mut site,
            "site/selection.typ",
            &marked_before(&pages, "meta.title"),
        )
        .expect("a hover");
        assert!(shape.contains("- `title: str = \"Untitled\"`"), "{shape}");
        assert!(shape.contains("- `draft: bool = false`"), "{shape}");
        let field = template_hover(
            &mut site,
            "site/selection.typ",
            &marked_before(&pages, "title }"),
        )
        .expect("a hover");
        assert!(
            field.contains("- `record.meta.title: str = \"Untitled\"`"),
            "{field}"
        );
    }
}
