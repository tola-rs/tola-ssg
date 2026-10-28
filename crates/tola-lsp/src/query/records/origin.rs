//! Where one record chain's declared shape comes from.

use std::ops::Range;
use std::sync::Arc;

use anyhow::Result;
use tola_packages::Signature;
use tola_typst::typst::foundations::Value;
use tola_typst::typst::syntax::ast::{self, AstNode};
use tola_typst::typst::syntax::{FileId, LinkedNode, SyntaxKind, VirtualRoot};
use tola_typst_syntax::names::{Declaration, DeclarationKind, SourceNames};

use super::super::context::Selection;
use super::super::imports::calls_package_member;
use super::super::inference::binding_is_written;
use super::super::schema;
use super::super::semantic::Semantic;
use super::super::site_schema::declared_schema;

/// One expression's field chain: its root and the fields read after it; None without fields.
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
fn schema_binding_is_written(
    names: &SourceNames,
    declaration: usize,
    semantics: &Semantic<'_>,
) -> Result<bool> {
    binding_is_written(
        names,
        declaration,
        names.source().text().len(),
        |method| {
            // The projection resolver proves record lineage through these callbacks.
            matches!(method, "filter" | "map")
        },
        || {
            semantics.cancellation().ensure_active()?;
            Ok(())
        },
    )
}

/// The receiver a method call reads, when every observation at it is an array.
fn array_receiver<'a>(
    callee: &LinkedNode<'a>,
    semantics: &mut Semantic<'_>,
) -> Result<Option<LinkedNode<'a>>> {
    let Some(access) = callee.cast::<ast::FieldAccess>() else {
        return Ok(None);
    };
    let Some(receiver) = callee.find(access.target().span()) else {
        return Ok(None);
    };
    let values = semantics.known_values(&receiver)?;
    let observed_array = tola_typst::observation_is_complete(&values)
        && values
            .iter()
            .all(|(value, _)| matches!(value, Value::Array(_)));
    Ok(observed_array.then_some(receiver))
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
    /// The source descriptors these records hold; parse-sources admits only complete all-sources
    pub(in crate::query) fn descriptors(&self) -> Option<SourceDescriptors> {
        match self {
            Self::Descriptors(descriptors) => Some(*descriptors),
            Self::Parsed { descriptors, .. } => Some(descriptors.unwrap_or(SourceDescriptors::All)),
            Self::Projected(_) => None,
        }
    }
}

/// One expression addressed in the source that writes it, which may outlive the node.
#[derive(Clone)]
pub(in crate::query) struct SourceExpression {
    /// The declaring file's names, which keep its source alive.
    names: Arc<SourceNames>,
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

impl ProjectedRecords {
    /// The field the returned record literal writes under `name`.
    pub(super) fn field(&self, name: &str) -> Option<&ProjectedField> {
        self.fields.iter().find(|field| field.name == name)
    }
}

impl ProjectedValue {
    /// The type spelling the proven value reads as.
    pub(super) fn type_name(&self) -> String {
        match self {
            Self::Record(_) => "dictionary".to_owned(),
            Self::Declared(ty) => ty.clone(),
        }
    }
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

/// One declaration an origin walk already followed; an index is unique only inside its file.
#[derive(PartialEq)]
struct FollowedDeclaration {
    file: FileId,
    index: usize,
}

/// Follow only unchanged local bindings, loop variables, mapped records, and project function
pub(in crate::query) fn record_origin<'a>(
    names: &'a Arc<SourceNames>,
    current: LinkedNode<'a>,
    semantics: &mut Semantic<'_>,
) -> Result<Option<RecordOrigin>> {
    record_origin_within(names, current, semantics, &mut Vec::new())
}

/// A chain's records come from the bindings its root reads, followed through let, calls, and loops.
fn record_origin_within<'a>(
    names: &'a Arc<SourceNames>,
    mut current: LinkedNode<'a>,
    semantics: &mut Semantic<'_>,
    followed: &mut Vec<FollowedDeclaration>,
) -> Result<Option<RecordOrigin>> {
    loop {
        if let Some(grouped) = current.get().cast::<ast::Parenthesized>() {
            let Some(inner) = current.find(grouped.expr().span()) else {
                return Ok(None);
            };
            current = inner;
            continue;
        }
        if let Some(call) = current.get().cast::<ast::FuncCall>() {
            match call_origin(&current, call, names, semantics, followed)? {
                CallOrigin::Proven(origin) => return Ok(Some(origin)),
                CallOrigin::Receiver(receiver) => {
                    current = receiver;
                    continue;
                }
                CallOrigin::Unproven => return Ok(None),
            }
        }
        if current.get().cast::<ast::Ident>().is_none() {
            return Ok(None);
        }
        let Some(index) = names.declared_at(current.range().start) else {
            return Ok(None);
        };
        let file = names.source().id();
        let reached = FollowedDeclaration { file, index };
        if followed.contains(&reached) || schema_binding_is_written(names, index, semantics)? {
            return Ok(None);
        }
        followed.push(reached);
        let Some(next) = binding_source(names, &names.declarations()[index], semantics)? else {
            return Ok(None);
        };
        current = next;
    }
}

/// What one call establishes about the origin of the records a chain reads through it.
enum CallOrigin<'a> {
    /// The call's own declaration proves the origin.
    Proven(RecordOrigin),
    /// A `filter` keeps the records of the receiver the chain continues from.
    Receiver(LinkedNode<'a>),
    /// The call proves nothing.
    Unproven,
}

/// The origin one call establishes, from the natives and site functions its callee names.
fn call_origin<'a>(
    call_node: &LinkedNode<'a>,
    call: ast::FuncCall<'a>,
    names: &'a Arc<SourceNames>,
    semantics: &mut Semantic<'_>,
    followed: &mut Vec<FollowedDeclaration>,
) -> Result<CallOrigin<'a>> {
    let Some(callee) = call_node.find(call.callee().span()) else {
        return Ok(CallOrigin::Unproven);
    };
    if let Some(descriptors) = source_descriptors(names, &callee) {
        return Ok(CallOrigin::Proven(RecordOrigin::Descriptors(descriptors)));
    }
    if calls_package_member(
        names,
        &callee,
        tola_packages::SOURCE_PACKAGE,
        tola_packages::PARSE_SOURCES,
    ) {
        return Ok(parsed_origin(call_node, call, names, semantics, followed)?
            .map_or(CallOrigin::Unproven, CallOrigin::Proven));
    }
    // A call to a site-declared function answers the records its body returns.
    if let Some(origin) = project_function_origin(&callee, semantics, followed)? {
        return Ok(CallOrigin::Proven(origin));
    }
    // A filter keeps its receiver's records, so a chain over it keeps that origin.
    let Some(access) = callee.cast::<ast::FieldAccess>() else {
        return Ok(CallOrigin::Unproven);
    };
    if access.field().as_str() != "filter" {
        return Ok(CallOrigin::Unproven);
    }
    let Some(receiver) = array_receiver(&callee, semantics)? else {
        return Ok(CallOrigin::Unproven);
    };
    Ok(CallOrigin::Receiver(receiver))
}

/// The source-descriptor native one call names, when it names one.
fn source_descriptors(names: &SourceNames, callee: &LinkedNode<'_>) -> Option<SourceDescriptors> {
    [
        (tola_packages::ALL_SOURCES, SourceDescriptors::All),
        (tola_packages::CURRENT_SOURCE, SourceDescriptors::Current),
    ]
    .into_iter()
    .find(|(item, _)| calls_package_member(names, callee, tola_packages::SOURCE_PACKAGE, item))
    .map(|(_, descriptors)| descriptors)
}

/// The origin one `parse-sources` call declares, from its schema argument and its input records.
fn parsed_origin<'a>(
    call_node: &LinkedNode<'a>,
    call: ast::FuncCall<'a>,
    names: &'a Arc<SourceNames>,
    semantics: &mut Semantic<'_>,
    followed: &mut Vec<FollowedDeclaration>,
) -> Result<Option<RecordOrigin>> {
    let root = tola_typst_syntax::syntax::root_expression(call_node.clone());
    let Some(declaration) = names.declared_at(root.range().start) else {
        return Ok(None);
    };
    if schema_binding_is_written(names, declaration, semantics)? {
        return Ok(None);
    }
    // parse-sources binds its schema positionally; a nested or named schema call proves nothing.
    let arguments = call.args().items().collect::<Vec<_>>();
    let [ast::Arg::Pos(records), ast::Arg::Pos(schema)] = arguments.as_slice() else {
        return Ok(None);
    };
    let (records, schema) = (records.span(), schema.span());
    let Some(schema) = call_node.find(schema) else {
        return Ok(None);
    };
    let Some(records) = call_node.find(records) else {
        return Ok(None);
    };
    let descriptors = match record_origin_within(names, records, semantics, followed)? {
        Some(RecordOrigin::Descriptors(descriptors)) => Some(descriptors),
        Some(RecordOrigin::Parsed { descriptors, .. }) => descriptors,
        Some(RecordOrigin::Projected(_)) | None => None,
    };
    Ok(Some(RecordOrigin::Parsed {
        schema: SourceExpression {
            names: Arc::clone(names),
            range: schema.range(),
        },
        descriptors,
    }))
}

/// The expression a binding reads its records from: a `let` initializer, a closure parameter's
fn binding_source<'a>(
    names: &'a Arc<SourceNames>,
    declaration: &Declaration,
    semantics: &mut Semantic<'_>,
) -> Result<Option<LinkedNode<'a>>> {
    let Some(owner) = tola_typst_syntax::syntax::node_at_range(names.source(), &declaration.range)
    else {
        return Ok(None);
    };
    match declaration.kind {
        DeclarationKind::Let => Ok(let_initializer(&owner)),
        DeclarationKind::Parameter => parameter_receiver(&owner, semantics),
        DeclarationKind::Loop => Ok(loop_iterable(&owner)),
        _ => Ok(None),
    }
}

/// The nearest node at or above one node of one kind.
fn enclosing<'a>(node: &LinkedNode<'a>, kind: SyntaxKind) -> Option<LinkedNode<'a>> {
    let mut current = node.clone();
    loop {
        if current.kind() == kind {
            return Some(current);
        }
        current = current.parent()?.clone();
    }
}

/// The initializer one `let` declaration binds, when its pattern is one plain name.
fn let_initializer<'a>(declaration: &LinkedNode<'a>) -> Option<LinkedNode<'a>> {
    let owner = enclosing(declaration, SyntaxKind::LetBinding)?;
    let binding = owner.cast::<ast::LetBinding>()?;
    if !matches!(
        binding.kind(),
        ast::LetBindingKind::Normal(ast::Pattern::Normal(ast::Expr::Ident(_)))
    ) {
        return None;
    }
    binding
        .init()
        .and_then(|initializer| owner.find(initializer.span()))
}

/// Whether `closure` binds exactly one positional parameter, as one identifier.
fn binds_one_identifier(closure: ast::Closure<'_>) -> bool {
    matches!(
        closure.params().children().collect::<Vec<_>>().as_slice(),
        [ast::Param::Pos(ast::Pattern::Normal(ast::Expr::Ident(_)))]
    )
}

/// The array receiver one inline closure's single parameter reads its element records from.
fn parameter_receiver<'a>(
    declaration: &LinkedNode<'a>,
    semantics: &mut Semantic<'_>,
) -> Result<Option<LinkedNode<'a>>> {
    let Some(owner) = enclosing(declaration, SyntaxKind::Closure) else {
        return Ok(None);
    };
    let Some(closure) = owner.cast::<ast::Closure>() else {
        return Ok(None);
    };
    if !binds_one_identifier(closure) {
        return Ok(None);
    }
    let Some(call_node) = owner.parent().and_then(|arguments| arguments.parent()) else {
        return Ok(None);
    };
    let Some(call) = call_node.cast::<ast::FuncCall>() else {
        return Ok(None);
    };
    let arguments = call.args().items().collect::<Vec<_>>();
    let [ast::Arg::Pos(argument)] = arguments.as_slice() else {
        return Ok(None);
    };
    if argument.span() != owner.span() {
        return Ok(None);
    }
    let Some(callee) = call_node.find(call.callee().span()) else {
        return Ok(None);
    };
    array_receiver(&callee, semantics)
}

/// The iterable one for loop's variable reads its records from, when its pattern is one plain name.
fn loop_iterable<'a>(declaration: &LinkedNode<'a>) -> Option<LinkedNode<'a>> {
    let owner = enclosing(declaration, SyntaxKind::ForLoop)?;
    let loop_ = owner.cast::<ast::ForLoop>()?;
    if !matches!(loop_.pattern(), ast::Pattern::Normal(ast::Expr::Ident(_))) {
        return None;
    }
    owner.find(loop_.iterable().span())
}

/// The origin one call to a site-declared function resolves, from its declaration and body.
fn project_function_origin(
    callee: &LinkedNode<'_>,
    semantics: &mut Semantic<'_>,
    followed: &mut Vec<FollowedDeclaration>,
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
    if followed.contains(&FollowedDeclaration {
        file,
        index: declaration,
    }) {
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
    // The declaration is pushed before its body is read, so a recursive call cannot re-resolve.
    followed.push(FollowedDeclaration {
        file,
        index: declaration,
    });
    if let Some(records) = mapped_records(&tail, &names, semantics, followed)? {
        return Ok(Some(RecordOrigin::Projected(records)));
    }
    record_origin_within(&names, tail, semantics, followed)
}

/// The expression one closure body evaluates: itself, or a code block's last expression.
fn tail_expression(body: LinkedNode<'_>) -> Option<LinkedNode<'_>> {
    let Some(block) = body.cast::<ast::CodeBlock>() else {
        return Some(body);
    };
    let code = body.find(block.body().span())?;
    let last = code.cast::<ast::Code>()?.exprs().next_back()?;
    code.find(last.span())
}

/// The record a mapped callback returns, when it binds one identifier and a record literal.
fn mapped_records<'a>(
    call: &LinkedNode<'a>,
    names: &'a Arc<SourceNames>,
    semantics: &mut Semantic<'_>,
    followed: &mut Vec<FollowedDeclaration>,
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
    if !binds_one_identifier(closure) {
        return Ok(None);
    }
    let Some(body) = call.find(closure.body().span()) else {
        return Ok(None);
    };
    let Some((literal, dict)) = record_literal(&body) else {
        return Ok(None);
    };
    let mut fields = Vec::new();
    for item in dict.items() {
        // A keyed or spread field leaves the record's fields unproved.
        let ast::DictItem::Named(field) = item else {
            return Ok(None);
        };
        let Some(value) = literal.find(field.expr().span()) else {
            return Ok(None);
        };
        let known = projected_field_value(&value, names, semantics, followed)?;
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

/// The record one expression writes: its node and dictionary, written directly or in parentheses.
fn record_literal<'a>(node: &LinkedNode<'a>) -> Option<(LinkedNode<'a>, ast::Dict<'a>)> {
    if let Some(dict) = node.get().cast::<ast::Dict>() {
        return Some((node.clone(), dict));
    }
    let grouped = node.get().cast::<ast::Parenthesized>()?;
    let inner = node.find(grouped.expr().span())?;
    let dict = inner.get().cast::<ast::Dict>()?;
    Some((inner, dict))
}

/// What one returned field's value proves: its origin, or its callee's declared return.
fn projected_field_value<'a>(
    value: &LinkedNode<'a>,
    names: &'a Arc<SourceNames>,
    semantics: &mut Semantic<'_>,
    followed: &mut Vec<FollowedDeclaration>,
) -> Result<Option<ProjectedValue>> {
    if let Some(origin) = record_origin_within(names, value.clone(), semantics, followed)? {
        return Ok(Some(ProjectedValue::Record(Box::new(origin))));
    }
    Ok(call_return_type(value, semantics)?.map(ProjectedValue::Declared))
}

/// The return type a call's declaration names, via a native cast or an element name.
fn call_return_type(call: &LinkedNode<'_>, semantics: &mut Semantic<'_>) -> Result<Option<String>> {
    let Some(func) = call.cast::<ast::FuncCall>() else {
        return Ok(None);
    };
    let Some(callee) = call.find(func.callee().span()) else {
        return Ok(None);
    };
    let Some((Value::Func(func), _)) = semantics.known_values(&callee)?.into_iter().next() else {
        return Ok(None);
    };
    Ok(Signature::of(func.name().unwrap_or_default(), &func).returns)
}
/// The record a chain's value is, when the chain reads through records.
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
    let projected = records.field(field)?;
    match &projected.known {
        Some(ProjectedValue::Record(inner)) => chained_origin(inner, rest),
        _ => None,
    }
}
impl Selection<'_> {
    /// The schema of the particular record chain, observed completely at its proven parse call.
    pub(in crate::query) fn parsed_schema(
        &self,
        names: &Arc<SourceNames>,
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
}
