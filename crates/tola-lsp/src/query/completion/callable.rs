//! What one callable's completion spells: the arguments the author still passes, the calls the
//! position admits, and the rule spellings a show position adds.

use std::ops::Range;

use lsp_types::{CompletionItem, CompletionTextEdit, InsertTextFormat};
use tola_packages::{ParameterKind, Signature};
use tola_typst::typst::foundations::{CastInfo, Content, Func, Scope, Type, Value};
use tola_typst::typst::syntax::ast;
use tola_typst::typst::syntax::{LinkedNode, Side, Source, SyntaxKind, SyntaxMode};

/// Whether the call's receiver is already written where the call stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Receiver {
    /// A value's method: the expression the author wrote is the receiver.
    Bound,
    /// A module's or a type's member: the call passes the receiver as its first argument.
    Unbound,
}

/// What one callable offers a completion: the arguments the author still passes, and the calls the
/// position can spell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Callable {
    /// The positional parameters the author still passes.
    positional: usize,
    /// The named parameters the author still passes.
    named: usize,
    /// The callable collects the remaining arguments, so its argument list never runs out.
    rest: bool,
    /// The next positional parameter accepts content, so the call also writes as a block.
    content: bool,
    /// The callable builds an element.
    element: bool,
    /// The callable is a type, and the call runs its constructor.
    ty: bool,
    /// The callable's scope holds functions that take no receiver, so its name is a value of its
    /// own and the author may want it uncalled.
    static_members: bool,
}

impl Callable {
    /// What one known value offers, called with the receiver `receiver` states.
    pub(super) fn of_value(value: &Value, receiver: Receiver) -> Option<Self> {
        match value {
            Value::Func(func) => Some(Self::of_func(func, receiver)),
            Value::Type(ty) => {
                let constructor = ty.constructor().ok()?;
                Some(Self {
                    ty: true,
                    static_members: scope_has_static_member(ty.scope()),
                    ..Self::of_func(&constructor, Receiver::Unbound)
                })
            }
            _ => None,
        }
    }

    /// What one declared signature offers. Only its documentation names a parameter's type, so a
    /// block form is offered when the block argument is documented as `content`.
    pub(super) fn of_signature(signature: &Signature) -> Self {
        let mut callable = Self::default();
        for parameter in &signature.parameters {
            match parameter.kind {
                ParameterKind::Rest => callable.rest = true,
                ParameterKind::Positional => {
                    if callable.positional == 0 {
                        callable.content = parameter.ty_spelling() == "content";
                    }
                    callable.positional += 1;
                }
                ParameterKind::Named => callable.named += 1,
            }
        }
        callable
    }

    fn of_func(func: &Func, receiver: Receiver) -> Self {
        let mut params = func.params();
        let mut remaining = params.next();
        if receiver == Receiver::Bound
            && remaining
                .as_ref()
                .is_some_and(|param| param.name() == Some("self"))
        {
            remaining = params.next();
        }
        let mut callable = Self {
            element: func.to_element().is_some(),
            static_members: func.scope().is_some_and(scope_has_static_member),
            ..Self::default()
        };
        for param in remaining.into_iter().chain(params) {
            if param.variadic() {
                callable.rest = true;
            } else if param.positional() {
                if callable.positional == 0 {
                    callable.content = param
                        .to_native()
                        .is_some_and(|native| is_content(&native.input));
                }
                callable.positional += 1;
            }
        }
        callable
    }

    /// Whether the name stands on its own: the author writes the call only when they want it.
    fn prefers_scope(&self) -> bool {
        self.static_members && (self.element || self.ty)
    }
}

/// Whether a call's parameter accepts content, which the same call also writes as a block.
fn is_content(cast: &CastInfo) -> bool {
    match cast {
        CastInfo::Type(ty) => *ty == Type::of::<Content>(),
        CastInfo::Value(value, _) => matches!(value, Value::Content(_)),
        CastInfo::Union(casts) => casts.iter().any(is_content),
        CastInfo::Any => false,
    }
}

/// Whether a callable's scope holds a function that takes no receiver: the name then denotes a
/// value the author reads as well as calls.
fn scope_has_static_member(scope: &Scope) -> bool {
    scope.iter().any(|(_, binding)| match binding.read() {
        Value::Func(func) => func
            .params()
            .find(|param| param.positional())
            .is_none_or(|param| param.name() != Some("self")),
        _ => false,
    })
}

/// Whether the receivers of one member call agree that they supply `self`. A disagreement or an
/// empty set answers `None`, so the completion keeps the name alone rather than guess a call.
pub(super) fn receiver_binds(binds: impl Iterator<Item = bool>) -> Option<Receiver> {
    let mut agreed = None;
    for bound in binds {
        match agreed {
            Some(seen) if seen != bound => return None,
            _ => agreed = Some(bound),
        }
    }
    agreed.map(|bound| {
        if bound {
            Receiver::Bound
        } else {
            Receiver::Unbound
        }
    })
}

/// The syntax a completion stands in, as far as the calls it offers depend on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Surrounding {
    /// Ordinary code.
    Code,
    /// A position that names rather than calls: an import list, an include, a show rule's selector,
    /// a call's own arguments, or a `let` bound to a bare name.
    Name,
    /// A show rule's transform: the value the rule applies to what it selects.
    Transform,
    /// A `selector(...)` argument: the element or application a selector names.
    Selector,
}

/// The completions one callable's name offers here: the name as a scope, the call, the block form
/// of a call whose next argument is content, and the rule spellings a show position adds.
pub(super) fn callable_items(
    item: CompletionItem,
    source: &Source,
    cursor: usize,
    replaced: &Range<usize>,
    callable: Callable,
) -> Vec<CompletionItem> {
    if source.text()[replaced.end..].starts_with(['(', '[']) {
        return vec![item];
    }
    let rule = match surrounding(source, cursor) {
        Surrounding::Name => return vec![item],
        Surrounding::Transform if callable.positional + callable.named > 0 => Some(".with"),
        Surrounding::Selector if callable.element => Some(".where"),
        Surrounding::Transform | Surrounding::Selector => return vec![item],
        Surrounding::Code => None,
    };
    if let Some(suffix) = rule {
        let spelled = format!("{}{suffix}(${{}})", item.label);
        return vec![item.clone(), variant(&item, suffix, &spelled)];
    }
    let call = call_text(&item.label, &callable);
    let mut items = Vec::new();
    if callable.prefers_scope() {
        items.push(item.clone());
        items.push(variant(&item, ".paren", &call));
    } else {
        items.push(with_text(&item, &call));
    }
    if callable.content && !callable.rest {
        items.push(variant(
            &item,
            ".bracket",
            &format!("{}[${{}}]", item.label),
        ));
    }
    items
}

/// The syntax one completion stands in. Anything outside code mode names rather than calls.
fn surrounding(source: &Source, cursor: usize) -> Surrounding {
    let Some(mut node) = LinkedNode::new(source.root()).leaf_at(cursor, Side::Before) else {
        return Surrounding::Code;
    };
    if node.mode_after() != Some(SyntaxMode::Code) {
        return Surrounding::Name;
    }
    loop {
        match node.kind() {
            SyntaxKind::ModuleImport | SyntaxKind::ModuleInclude => return Surrounding::Name,
            SyntaxKind::ShowRule => {
                // The transform is the rule's last child; what comes before the colon selects.
                return match node.children().next_back() {
                    Some(transform) if covers(&transform, cursor) => Surrounding::Transform,
                    _ => Surrounding::Name,
                };
            }
            SyntaxKind::Args => {
                return match node.parent() {
                    Some(parent) if selector_names(parent) => Surrounding::Selector,
                    _ => Surrounding::Name,
                };
            }
            SyntaxKind::LetBinding => {
                let mut initializer = node
                    .cast::<ast::LetBinding>()
                    .and_then(|binding| binding.init());
                while let Some(ast::Expr::Parenthesized(group)) = initializer {
                    initializer = Some(group.expr());
                }
                return if matches!(
                    initializer,
                    Some(ast::Expr::Ident(_) | ast::Expr::FieldAccess(_))
                ) {
                    Surrounding::Name
                } else {
                    Surrounding::Code
                };
            }
            SyntaxKind::Closure
            | SyntaxKind::CodeBlock
            | SyntaxKind::ContentBlock
            | SyntaxKind::Markup => return Surrounding::Code,
            _ => {}
        }
        let Some(parent) = node.parent() else {
            return Surrounding::Code;
        };
        node = parent.clone();
    }
}

/// Whether one node covers a cursor offset, at either boundary.
fn covers(node: &LinkedNode<'_>, cursor: usize) -> bool {
    let range = node.range();
    range.start <= cursor && cursor <= range.end
}

/// Whether one call is `selector(...)`, whose arguments name what the rule selects.
fn selector_names(node: &LinkedNode<'_>) -> bool {
    let Some(call) = node.cast::<ast::FuncCall>() else {
        return false;
    };
    matches!(call.callee(), ast::Expr::Ident(ident) if ident.as_str() == "selector")
}

/// The call one item inserts, ending where the author's next argument goes.
fn call_text(name: &str, callable: &Callable) -> String {
    if callable.positional == 0 && !callable.rest {
        format!("{name}()${{}}")
    } else {
        format!("{name}(${{}})")
    }
}

/// One completion of the same name under a suffix: the label keeps the suffix apart from the name,
/// while the client filters on the name the author typed.
fn variant(item: &CompletionItem, suffix: &str, text: &str) -> CompletionItem {
    let mut variant = with_text(item, text);
    variant.label = format!("{}{suffix}", item.label);
    variant.filter_text = Some(item.label.clone());
    variant
}

/// One completion of the same name whose insertion is `text` instead of the name.
fn with_text(item: &CompletionItem, text: &str) -> CompletionItem {
    let mut completed = item.clone();
    let text = crate::completion::typst_snippet(text);
    if let Some(edit) = completed.text_edit.as_mut() {
        match edit {
            CompletionTextEdit::Edit(edit) => edit.new_text = text,
            CompletionTextEdit::InsertAndReplace(edit) => edit.new_text = text,
        }
    }
    completed.insert_text_format = Some(InsertTextFormat::SNIPPET);
    completed
}
