//! AST walk that indexes scopes, declarations, imports, and identifier occurrences.

use std::ops::Range;

use typst_syntax::ast::{self, AstNode};
use typst_syntax::{LinkedNode, Span, SyntaxKind};

use super::{
    Declaration, DeclarationKind, Import, ImportSource, Initializer, NamePath, Occurrence,
    OccurrenceKind, Scope, SourceNames,
};

impl SourceNames {
    fn range<'a>(&self, anchor: &LinkedNode<'_>, node: impl AstNode<'a>) -> Range<usize> {
        anchor.find(node.span()).map_or(0..0, |found| found.range())
    }

    fn scope(&mut self, parent: usize, range: Range<usize>, per_call: bool) -> usize {
        let id = self.scopes.len();
        self.scopes.push(Scope {
            parent: Some(parent),
            range,
            declarations: Vec::new(),
            declarations_by_name: Vec::new(),
            per_call,
        });
        id
    }

    fn declare(
        &mut self,
        anchor: &LinkedNode<'_>,
        ident: ast::Ident<'_>,
        scope: usize,
        visible_from: usize,
        kind: DeclarationKind,
        initializer: Initializer,
    ) -> Option<usize> {
        let range = anchor.find(ident.span())?.range();
        Some(self.bind(
            Declaration {
                name: ident.get().to_string(),
                range,
                scope,
                visible_from,
                kind,
                initializer,
                recursive_body: None,
            },
            true,
        ))
    }

    fn bind(&mut self, decl: Declaration, spelled: bool) -> usize {
        let id = self.declarations.len();
        self.scopes[decl.scope].declarations.push(id);
        self.declaration_starts.insert(decl.range.start, id);
        if spelled {
            self.occurrences.push(Occurrence {
                range: decl.range.clone(),
                scope: decl.scope,
                math: false,
                kind: OccurrenceKind::Declaration(id),
            });
        }
        self.declarations.push(decl);
        id
    }

    fn path(&self, anchor: &LinkedNode<'_>, expr: ast::Expr<'_>, scope: usize) -> Option<NamePath> {
        fn segments(
            names: &SourceNames,
            anchor: &LinkedNode<'_>,
            expr: ast::Expr<'_>,
            found: &mut Vec<Range<usize>>,
        ) -> Option<bool> {
            match expr {
                ast::Expr::Ident(ident) => {
                    found.push(names.range(anchor, ident));
                    Some(false)
                }
                ast::Expr::MathIdent(ident) => {
                    found.push(names.range(anchor, ident));
                    Some(true)
                }
                ast::Expr::FieldAccess(field) => {
                    let math = segments(names, anchor, field.target(), found)?;
                    found.push(names.range(anchor, field.field()));
                    Some(math)
                }
                ast::Expr::MathFieldAccess(field) => {
                    let math = segments(
                        names,
                        anchor,
                        field.target().to_untyped().cast::<ast::Expr>()?,
                        found,
                    )?;
                    found.push(names.range(anchor, field.field()));
                    Some(math)
                }
                ast::Expr::Parenthesized(group) => segments(names, anchor, group.expr(), found),
                _ => None,
            }
        }
        let mut found = Vec::new();
        let math = segments(self, anchor, expr, &mut found)?;
        Some(NamePath {
            at: found.first()?.start,
            segments: found,
            scope,
            math,
        })
    }

    fn initializer(
        &self,
        anchor: &LinkedNode<'_>,
        expr: Option<ast::Expr<'_>>,
        scope: usize,
    ) -> Initializer {
        match expr {
            Some(ast::Expr::Closure(closure)) => Initializer::Function {
                body: self.range(anchor, closure.body()),
            },
            Some(expr) => self
                .path(anchor, expr, scope)
                .map_or(Initializer::Unknown, Initializer::Alias),
            None => Initializer::Unknown,
        }
    }

    fn record_field<'a>(
        &mut self,
        node: &LinkedNode<'_>,
        scope: usize,
        field: impl AstNode<'a>,
        target: ast::Expr<'_>,
    ) {
        if let Some(path) = self.path(node, target, scope) {
            self.occurrences.push(Occurrence {
                range: self.range(node, field),
                scope,
                math: path.math,
                kind: OccurrenceKind::Field(path),
            });
        }
    }

    fn walk_child(&mut self, node: &LinkedNode<'_>, span: Span, scope: usize) {
        if let Some(child) = node.find(span) {
            self.walk(&child, scope);
        }
    }

    pub(super) fn walk(&mut self, node: &LinkedNode<'_>, scope: usize) {
        if let Some(binding) = node.cast::<ast::LetBinding>() {
            self.walk_let(node, binding, scope);
            return;
        }
        if let Some(closure) = node.cast::<ast::Closure>() {
            self.walk_closure(node, closure, scope);
            return;
        }
        if let Some(for_loop) = node.cast::<ast::ForLoop>() {
            self.walk_for_loop(node, for_loop, scope);
            return;
        }
        if let Some(import) = node.cast::<ast::ModuleImport>() {
            self.walk_import(node, import, scope);
            return;
        }
        if let Some(field) = node.cast::<ast::FieldAccess>() {
            self.walk_child(node, field.target().span(), scope);
            self.record_field(node, scope, field.field(), field.target());
            return;
        }
        if let Some(field) = node.cast::<ast::MathFieldAccess>() {
            self.walk_child(node, field.target().span(), scope);
            if let Some(target) = field.target().to_untyped().cast::<ast::Expr>() {
                self.record_field(node, scope, field.field(), target);
            }
            return;
        }
        if let Some(named) = node.cast::<ast::Named>() {
            self.walk_named_arg(node, named, scope);
            return;
        }
        if matches!(node.kind(), SyntaxKind::Ident | SyntaxKind::MathIdent) {
            self.occurrences.push(Occurrence {
                range: node.range(),
                scope,
                math: node.kind() == SyntaxKind::MathIdent,
                kind: OccurrenceKind::Name,
            });
            return;
        }
        let scope = if matches!(
            node.kind(),
            SyntaxKind::CodeBlock | SyntaxKind::ContentBlock
        ) {
            self.scope(scope, node.range(), false)
        } else {
            scope
        };
        for child in node.children() {
            self.walk(&child, scope);
        }
    }

    fn walk_let(&mut self, node: &LinkedNode<'_>, binding: ast::LetBinding<'_>, scope: usize) {
        let init = binding.init();
        let recursive_body = match binding.kind() {
            ast::LetBindingKind::Closure(_) => match init {
                Some(ast::Expr::Closure(closure)) => Some(self.range(node, closure.body())),
                _ => None,
            },
            _ => None,
        };
        let names = binding.kind().bindings();
        let single = names.len() == 1;
        for name in names {
            let init_kind = if single {
                self.initializer(node, init, scope)
            } else {
                Initializer::Unknown
            };
            if let Some(id) = self.declare(
                node,
                name,
                scope,
                node.range().end,
                DeclarationKind::Let,
                init_kind,
            ) {
                self.declarations[id].recursive_body = recursive_body.clone();
            }
        }
        if let Some(init) = init.and_then(|init| node.find(init.span())) {
            self.walk(&init, scope);
        }
    }

    fn walk_closure(&mut self, node: &LinkedNode<'_>, closure: ast::Closure<'_>, scope: usize) {
        // Defaults evaluate in the enclosing scope, not in the parameter scope.
        for param in closure.params().children() {
            if let ast::Param::Named(named) = param {
                self.walk_child(node, named.expr().span(), scope);
            }
        }
        let body_range = self.range(node, closure.body());
        let child_scope = self.scope(scope, body_range.clone(), true);
        for param in closure.params().children() {
            let is_named = matches!(param, ast::Param::Named(_));
            let names = match param {
                ast::Param::Pos(pattern) => pattern.bindings(),
                ast::Param::Named(named) => vec![named.name()],
                ast::Param::Spread(spread) => spread.sink_ident().into_iter().collect(),
            };
            for name in names {
                if let Some(id) = self.declare(
                    node,
                    name,
                    child_scope,
                    body_range.start,
                    DeclarationKind::Parameter,
                    Initializer::Unknown,
                ) && is_named
                {
                    self.named_parameters
                        .entry(body_range.start)
                        .or_default()
                        .insert(self.declarations[id].name.clone(), id);
                }
            }
        }
        self.walk_child(node, closure.body().span(), child_scope);
    }

    fn walk_for_loop(&mut self, node: &LinkedNode<'_>, for_loop: ast::ForLoop<'_>, scope: usize) {
        self.walk_child(node, for_loop.iterable().span(), scope);
        let body_range = self.range(node, for_loop.body());
        let child_scope = self.scope(scope, body_range.clone(), false);
        for name in for_loop.pattern().bindings() {
            let _ = self.declare(
                node,
                name,
                child_scope,
                body_range.start,
                DeclarationKind::Loop,
                Initializer::Unknown,
            );
        }
        self.walk_child(node, for_loop.body().span(), child_scope);
    }

    fn walk_import(&mut self, node: &LinkedNode<'_>, import: ast::ModuleImport<'_>, scope: usize) {
        let source_range = self.range(node, import.source());
        let source = match import.source() {
            ast::Expr::Str(path) => ImportSource::Path(path.get().to_string()),
            expr => self
                .path(node, expr, scope)
                .map_or(ImportSource::Unknown, ImportSource::Name),
        };
        self.walk_child(node, import.source().span(), scope);
        let id = self.imports.len();
        self.imports.push(Import {
            source,
            range: node.range(),
            source_range: source_range.clone(),
            scope,
            wildcard: matches!(import.imports(), Some(ast::Imports::Wildcard)),
            items: Vec::new(),
        });
        if let Some(name) = import.new_name() {
            let _ = self.declare(
                node,
                name,
                scope,
                node.range().end,
                DeclarationKind::Import {
                    explicit_alias: true,
                    implicit: false,
                },
                Initializer::Import {
                    import: id,
                    path: Vec::new(),
                },
            );
        } else if import.imports().is_none()
            && let Ok(name) = import.bare_name()
        {
            self.bind(
                Declaration {
                    name: name.to_string(),
                    range: source_range,
                    scope,
                    visible_from: node.range().end,
                    kind: DeclarationKind::Import {
                        explicit_alias: false,
                        implicit: true,
                    },
                    initializer: Initializer::Import {
                        import: id,
                        path: Vec::new(),
                    },
                    recursive_body: None,
                },
                false,
            );
        }
        if let Some(ast::Imports::Items(items)) = import.imports() {
            for item in items.iter() {
                let spelling = match item {
                    ast::ImportItem::Simple(path) => self.range(node, path),
                    ast::ImportItem::Renamed(renamed) => self.range(node, renamed),
                };
                self.imports[id].items.push(spelling);
                let mut path = Vec::new();
                for part in item.path().iter() {
                    path.push(part.get().to_string());
                    self.occurrences.push(Occurrence {
                        range: self.range(node, part),
                        scope,
                        math: false,
                        kind: OccurrenceKind::Import {
                            import: id,
                            path: path.clone(),
                        },
                    });
                }
                let explicit_alias = matches!(item, ast::ImportItem::Renamed(_));
                let decl = self.declare(
                    node,
                    item.bound_name(),
                    scope,
                    node.range().end,
                    DeclarationKind::Import {
                        explicit_alias,
                        implicit: false,
                    },
                    Initializer::Import { import: id, path },
                );
                if !explicit_alias && decl.is_some() {
                    self.occurrences.pop();
                }
            }
        }
    }

    fn walk_named_arg(&mut self, node: &LinkedNode<'_>, named: ast::Named<'_>, scope: usize) {
        if let Some(call) = node.parent().and_then(|args| args.parent()) {
            let callee = match call.cast::<ast::FuncCall>() {
                Some(call) => Some(call.callee()),
                None => call
                    .cast::<ast::MathCall>()
                    .and_then(|call| call.callee().to_untyped().cast::<ast::Expr>()),
            };
            // Labels use identifier syntax even inside math calls, so rename keeps `Ident` rules.
            if let Some(callee) = callee.and_then(|callee| self.path(call, callee, scope)) {
                self.occurrences.push(Occurrence {
                    range: self.range(node, named.name()),
                    scope,
                    math: false,
                    kind: OccurrenceKind::Argument(callee),
                });
            }
        }
        self.walk_child(node, named.expr().span(), scope);
    }
}
