//! Source-bound declarations and lexical scopes from Typst's AST.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Range;

use typst_syntax::ast::{self, AstNode};
use typst_syntax::{FileId, LinkedNode, Source, SyntaxKind};

/// A lexical visibility boundary; a nested body never leaks its declarations outward.
#[derive(Clone, Debug)]
pub struct Scope {
    /// Enclosing scope, absent only at the file root.
    pub parent: Option<usize>,
    /// Half-open UTF-8 byte extent of the scope's body.
    pub range: Range<usize>,
    /// Declaration indices in binding order, with later entries able to shadow earlier ones.
    pub declarations: Vec<usize>,
    /// The same indices ordered by spelling, binding order kept inside one spelling, so one name
    /// resolves without scanning every declaration the scope holds.
    declarations_by_name: Vec<usize>,
    /// Set for a closure or named-function body, whose statements run at each call rather than
    /// during the file's own evaluation.
    pub per_call: bool,
}

impl Scope {
    /// The declaration indices one spelling binds, in binding order.
    ///
    /// [`Self::declarations_by_name`] is ordered by spelling, so the two partitions frame the
    /// spelling's own declarations without reading the scope's others.
    fn declarations_named<'a>(&'a self, declarations: &[Declaration], name: &str) -> &'a [usize] {
        let start = self
            .declarations_by_name
            .partition_point(|&id| declarations[id].name.as_str() < name);
        let rest = &self.declarations_by_name[start..];
        let end = rest.partition_point(|&id| declarations[id].name.as_str() == name);
        &rest[..end]
    }
}

/// A name root followed by zero or more field selections.
#[derive(Clone, Debug)]
pub struct NamePath {
    /// Identifier byte ranges, starting with the lexical root.
    pub segments: Vec<Range<usize>>,
    /// Scope used for the first segment; later segments are fields, not lexical names.
    pub scope: usize,
    /// Lexical lookup offset, retaining declaration-before-use precedence.
    pub at: usize,
    /// Selects the math namespace for a bare math identifier, not for escaped code.
    pub math: bool,
}

/// Source-established identity clues; these do not evaluate or infer runtime values.
#[derive(Clone, Debug)]
pub enum Initializer {
    /// The expression has no statically established function, module, or alias identity.
    Unknown,
    /// The AST declares a closure, including named-function syntax.
    Function {
        /// Body extent used to associate named call arguments with their parameters.
        body: Range<usize>,
    },
    /// The initializer directly reads an existing name or module field.
    Alias(NamePath),
    /// An imported value, with its external spelling kept separate from the local declaration.
    Import {
        /// Index of the statement that supplies the module interface.
        import: usize,
        /// Original field path; an empty path denotes the imported module itself.
        path: Vec<String>,
    },
}

/// Binding forms whose visibility and rename spelling differ.
#[derive(Clone, Debug)]
pub enum DeclarationKind {
    /// A `let` pattern or the name in named-function syntax.
    Let,
    /// Visible only in a closure body, not in parameter defaults.
    Parameter,
    /// Visible only in the loop body, not in the iterable expression.
    Loop,
    /// A local name introduced by an import statement.
    Import {
        /// An explicit `as` spelling can change without changing the external export.
        explicit_alias: bool,
        /// No identifier is written locally; rename must insert an alias after the source.
        implicit: bool,
    },
}

/// One locally bound identity, distinct from any external export it imports.
#[derive(Clone, Debug)]
pub struct Declaration {
    /// Local spelling, after applying an explicit or implicit import alias.
    pub name: String,
    /// Identifier bytes, or the source-expression bytes of an implicit module name.
    pub range: Range<usize>,
    /// Owning scope, used for both visibility and same-scope collision checks.
    pub scope: usize,
    /// First byte at which ordinary lexical uses can resolve this declaration.
    pub visible_from: usize,
    /// Extra visibility for a named function's self-reference, excluding its defaults.
    pub recursive_body: Option<Range<usize>>,
    /// Determines whether rename replaces a name or introduces an import alias.
    pub kind: DeclarationKind,
    /// Static identity evidence only; runtime values remain the compiler's authority.
    pub initializer: Initializer,
}

/// The source of a module interface, without host path resolution or evaluation.
#[derive(Clone, Debug)]
pub enum ImportSource {
    /// Decoded import string for the host's site/package resolver.
    Path(String),
    /// A lexical name or field whose known module interface can be reused.
    Name(NamePath),
    /// A dynamic expression with no source-established module identity.
    Unknown,
}

/// An import statement before the host supplies its target source.
#[derive(Clone, Debug)]
pub struct Import {
    /// Path or name expression from which exports are selected.
    pub source: ImportSource,
    /// Statement bytes excluding the markup `#`; the end is an alias insertion point.
    pub range: Range<usize>,
    /// Complete source-expression bytes, preserving string quotes when inserting another import.
    pub source_range: Range<usize>,
    /// Scope in which imported local names become visible.
    pub scope: usize,
    /// Whether the statement contributes the target interface's exported names.
    pub wildcard: bool,
    /// Byte extents of the statement's item spellings, in source order.
    ///
    /// Empty when the statement selects no item: a wildcard, or an import of the module itself.
    pub items: Vec<Range<usize>>,
}
/// Grammar roles distinguish local identities from external import spellings.
#[derive(Clone, Debug)]
pub enum OccurrenceKind {
    /// Index of the local declaration written at this occurrence.
    Declaration(usize),
    /// A lexical read rather than a field, parameter label, or import path segment.
    Name,
    /// Target path of a field selection, excluding the field written at this occurrence.
    Field(NamePath),
    /// Callee path whose named parameter may own this argument label.
    Argument(NamePath),
    /// An external import path prefix, potentially also declaring its final local name.
    Import {
        /// Index of the statement supplying the external module interface.
        import: usize,
        /// Original path through the current segment, before any local `as` alias.
        path: Vec<String>,
    },
}

/// One identifier spelling in the original parsed source.
#[derive(Clone, Debug)]
pub struct Occurrence {
    /// UTF-8 bytes to select or replace; protocol adapters must convert positions.
    pub range: Range<usize>,
    /// Lexical scope at the spelling, independent of where its declaration lives.
    pub scope: usize,
    /// Requires math identifier syntax when renaming this occurrence.
    pub math: bool,
    /// Determines which lexical, import, field, or parameter lookup applies.
    pub kind: OccurrenceKind,
}

/// One import statement whose bindings no spelling in its source reads.
///
/// Source-local: the statement's target is never loaded, so a wildcard statement — whose interface
/// this source cannot see — never appears here, and the record states whether the file's own scope
/// holds the unread bindings. A binding in that scope is also the file's module interface, which
/// another source can import, so only a caller that can rule that out may call it unused.
#[derive(Clone, Debug)]
pub struct UnreadImport {
    /// The statement's bytes, excluding the markup `#`.
    pub statement: Range<usize>,
    /// Whether the file's own scope holds the unread bindings.
    pub file_scope: bool,
    /// The unread local spellings, in source order.
    pub names: Vec<String>,
    /// What a correction deletes to remove exactly those bindings.
    pub removal: ImportRemoval,
}

/// What removes the bindings an [`UnreadImport`] reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportRemoval {
    /// The whole statement, because nothing it introduces would remain imported.
    Statement,
    /// Exactly these byte extents, in source order; the statement keeps its other bindings.
    Spans(Vec<Range<usize>>),
}

/// The names other sources select from one source's interface.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectedNames {
    /// Whether some statement imports the file itself or everything it exports.
    pub whole: bool,
    /// The names statements select by spelling, as their first path segment spells them.
    pub names: BTreeSet<String>,
}

impl SelectedNames {
    /// Whether a binding with this spelling is reachable from another source.
    pub fn reaches(&self, name: &str) -> bool {
        self.whole || self.names.contains(name)
    }
}

/// What the statements of one source set select from that set's interfaces.
#[derive(Debug, Default)]
pub struct SelectedInterfaces {
    /// Whether a statement's source is not a path, so nothing here says what it selects.
    pub unresolved: bool,
    /// The names other sources select, keyed by the file they select from.
    pub selected: HashMap<FileId, SelectedNames>,
}

impl SelectedInterfaces {
    /// Whether the index saw nothing that reads this spelling of `file`.
    ///
    /// A statement whose source is not a path may select any name of any file, so an index holding
    /// one reads nothing as unread.
    pub fn reads_nothing_from(&self, file: FileId, name: &str) -> bool {
        !(self.unresolved)
            && !self
                .selected
                .get(&file)
                .is_some_and(|selected| selected.reaches(name))
    }
}

/// Collects what other sources select from each source's interface.
///
/// `resolve` establishes the file one path spells, exactly as the compiler resolves it; a path this
/// never resolves selects from nothing, and a package path is skipped because a package's sources
/// are never sources of the site that spells it. A selective import records the spelling's first
/// path segment, which is the name its target must export; a statement that imports the module
/// itself or a wildcard records `whole`, because either reaches every name.
pub fn selected_interfaces<'a>(
    sources: impl IntoIterator<Item = &'a SourceNames>,
    resolve: impl Fn(FileId, &str) -> Option<FileId>,
) -> SelectedInterfaces {
    let mut collected = SelectedInterfaces::default();
    for source in sources {
        for (statement, import) in source.imports().iter().enumerate() {
            let path = match &import.source {
                ImportSource::Path(path) if !path.starts_with('@') => path,
                ImportSource::Path(_) => continue,
                _ => {
                    collected.unresolved = true;
                    continue;
                }
            };
            let Some(target) = resolve(source.source().id(), path) else {
                continue;
            };
            let selected = collected.selected.entry(target).or_default();
            if import.wildcard || import.items.is_empty() {
                selected.whole = true;
            } else {
                selected
                    .names
                    .extend(source.declarations().iter().filter_map(|declaration| {
                        match &declaration.initializer {
                            Initializer::Import {
                                import: owner,
                                path,
                            } if *owner == statement => path.first().cloned(),
                            _ => None,
                        }
                    }));
            }
        }
    }
    collected
}

/// An immutable name index for one exact parsed source.
///
/// Ranges and declaration indices address [`Self::source`]. After changing source text, construct
/// a new index rather than retaining answers from the previous source.
#[derive(Debug)]
pub struct SourceNames {
    source: Source,
    scopes: Vec<Scope>,
    declarations: Vec<Declaration>,
    imports: Vec<Import>,
    named_parameters: BTreeMap<usize, BTreeMap<String, usize>>,
    occurrences: Vec<Occurrence>,
    declaration_starts: BTreeMap<usize, usize>,
}

impl SourceNames {
    /// Indexes the official AST without evaluating expressions or loading imports.
    pub fn new(source: Source) -> Self {
        let mut index = Self {
            scopes: vec![Scope {
                parent: None,
                range: 0..source.text().len(),
                declarations: Vec::new(),
                declarations_by_name: Vec::new(),
                per_call: false,
            }],
            source: source.clone(),
            declarations: Vec::new(),
            imports: Vec::new(),
            named_parameters: BTreeMap::new(),
            occurrences: Vec::new(),
            declaration_starts: BTreeMap::new(),
        };
        index.walk(&LinkedNode::new(source.root()), 0);
        index
            .occurrences
            .sort_by_key(|occurrence| occurrence.range.start);
        index.order_declarations_by_name();
        index
    }

    /// Orders each scope's declarations by spelling; the sort is stable, so equal spellings keep
    /// binding order.
    fn order_declarations_by_name(&mut self) {
        let declarations = &self.declarations;
        for scope in &mut self.scopes {
            scope.declarations_by_name = scope.declarations.clone();
            scope
                .declarations_by_name
                .sort_by(|&left, &right| declarations[left].name.cmp(&declarations[right].name));
        }
    }

    /// The parsed source every range in this index addresses.
    pub fn source(&self) -> &Source {
        &self.source
    }

    /// Lexical scopes; scope zero is the file and descendants retain their parent indices.
    pub fn scopes(&self) -> &[Scope] {
        &self.scopes
    }

    /// Local declarations in binding order, including imported aliases.
    pub fn declarations(&self) -> &[Declaration] {
        &self.declarations
    }

    /// Import statements in source order, before a host supplies their target interfaces.
    pub fn imports(&self) -> &[Import] {
        &self.imports
    }

    /// Identifier spellings in source order, including unresolved reads.
    pub(crate) fn occurrences(&self) -> &[Occurrence] {
        &self.occurrences
    }

    pub(crate) fn named_parameters(&self) -> &BTreeMap<usize, BTreeMap<String, usize>> {
        &self.named_parameters
    }

    /// Selects a spelling at a byte cursor, including its trailing boundary.
    pub fn occurrence(&self, cursor: usize) -> Option<&Occurrence> {
        let after = self
            .occurrences
            .partition_point(|occurrence| occurrence.range.start <= cursor);
        self.occurrences[..after]
            .last()
            .filter(|occurrence| cursor <= occurrence.range.end)
    }

    /// The unread imports of this source a report may name.
    ///
    /// A binding the file's own scope holds is part of its module interface, and another source may
    /// import it; only an index built from every source of the site can rule that out, and
    /// `selected` states one. A binding a nested scope holds reaches no other source, so it is
    /// named whatever the index holds.
    pub fn reportable_unread_imports(&self, selected: &SelectedInterfaces) -> Vec<UnreadImport> {
        self.unread_imports()
            .into_iter()
            .filter(|unread| {
                !unread.file_scope
                    || unread
                        .names
                        .iter()
                        .all(|name| selected.reads_nothing_from(self.source.id(), name))
            })
            .collect()
    }

    /// Every import statement whose bindings no spelling in this source reads.
    ///
    /// Source-local: a statement's target is never loaded, so a wildcard statement — whose
    /// interface this source cannot see — never appears, and a statement whose bindings the file's
    /// own scope holds keeps [`UnreadImport::file_scope`] set, because those bindings are also
    /// this file's module interface and another source can import them.
    pub fn unread_imports(&self) -> Vec<UnreadImport> {
        let mut read = BTreeSet::new();
        for occurrence in &self.occurrences {
            if matches!(occurrence.kind, OccurrenceKind::Name)
                && let Some(declaration) = self.resolve(
                    self.text(&occurrence.range),
                    occurrence.scope,
                    occurrence.range.start,
                )
            {
                read.insert(declaration);
            }
        }
        let mut unread: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (declaration, spelled) in self.declarations.iter().enumerate() {
            if !read.contains(&declaration)
                && let Initializer::Import { import, .. } = &spelled.initializer
            {
                unread.entry(*import).or_default().push(declaration);
            }
        }
        unread
            .into_iter()
            .map(|(import, declarations)| self.unread_import(import, &declarations))
            .collect()
    }

    /// The record one statement's unread declarations make.
    fn unread_import(&self, import: usize, unread: &[usize]) -> UnreadImport {
        let statement = &self.imports[import];
        let removal = if statement.items.is_empty() || statement.items.len() == unread.len() {
            // Nothing the statement introduces would remain imported, and a statement that names
            // the module itself has no item to remove, so only the whole statement can go.
            ImportRemoval::Statement
        } else {
            ImportRemoval::Spans(
                unread
                    .iter()
                    .filter_map(|&declaration| self.item_removal(statement, declaration))
                    .collect(),
            )
        };
        UnreadImport {
            statement: statement.range.clone(),
            file_scope: statement.scope == 0,
            names: unread
                .iter()
                .map(|&declaration| self.declarations[declaration].name.clone())
                .collect(),
            removal,
        }
    }

    /// The bytes whose deletion removes exactly the item binding one declaration.
    ///
    /// A first item takes the separator that follows it and a later one the separator before it,
    /// so a statement keeps every item it still imports.
    fn item_removal(&self, statement: &Import, declaration: usize) -> Option<Range<usize>> {
        let spelled = &self.declarations[declaration].range;
        let index = statement
            .items
            .iter()
            .position(|item| item.start <= spelled.start && spelled.end <= item.end)?;
        let item = &statement.items[index];
        match statement.items.get(index + 1) {
            Some(next) => Some(item.start..next.start),
            None => Some(statement.items[index.checked_sub(1)?].end..item.end),
        }
    }

    /// Returns a local declaration index; fields and external origins need a name graph.
    pub fn declared_at(&self, cursor: usize) -> Option<usize> {
        let occurrence = self.occurrence(cursor)?;
        match &occurrence.kind {
            OccurrenceKind::Declaration(declaration) => Some(*declaration),
            OccurrenceKind::Name => self.resolve(
                self.text(&occurrence.range),
                occurrence.scope,
                occurrence.range.start,
            ),
            OccurrenceKind::Import { .. } => self
                .declaration_starts
                .get(&occurrence.range.start)
                .copied(),
            _ => None,
        }
    }

    /// Whether the spelling at the cursor reads a value.
    ///
    /// A declaration, import path segment, argument label, dict key, or field name reads nothing
    /// by itself.
    pub fn reads_value_at(&self, cursor: usize) -> bool {
        matches!(
            self.occurrence(cursor),
            Some(Occurrence {
                kind: OccurrenceKind::Name,
                ..
            })
        )
    }

    /// Resolves lexical bindings at a byte position; wildcard interfaces are not loaded here.
    pub fn resolve(&self, name: &str, mut scope: usize, at: usize) -> Option<usize> {
        loop {
            let found = self.scopes[scope]
                .declarations_named(&self.declarations, name)
                .iter()
                .rev()
                .copied()
                .find(|&id| {
                    let declaration = &self.declarations[id];
                    declaration.visible_from <= at
                        || declaration
                            .recursive_body
                            .as_ref()
                            .is_some_and(|body| body.contains(&at))
                });
            if found.is_some() {
                return found;
            }
            scope = self.scopes[scope].parent?;
        }
    }

    /// Slices a range belonging to this source; foreign or non-UTF-8 ranges are invalid.
    pub fn text(&self, range: &Range<usize>) -> &str {
        &self.source.text()[range.clone()]
    }

    /// Chooses the innermost body containing the byte offset, or the file scope at a gap.
    pub fn scope_at(&self, at: usize) -> usize {
        self.scopes
            .iter()
            .enumerate()
            .filter(|(_, scope)| scope.range.contains(&at))
            .min_by_key(|(id, scope)| (scope.range.len(), std::cmp::Reverse(*id)))
            .map_or(0, |(id, _)| id)
    }

    /// Whether this file's own evaluation reaches every statement the scope holds.
    ///
    /// A closure body runs whenever its function is called, and a call site may live in another
    /// file with another argument, so evaluating this file observes one call rather than all of
    /// them.
    pub fn evaluated_here(&self, scope: usize) -> bool {
        let mut current = Some(scope);
        while let Some(id) = current {
            if self.scopes[id].per_call {
                return false;
            }
            current = self.scopes[id].parent;
        }
        true
    }

    /// The byte range one AST node spans, searched from the node whose subtree holds it.
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

    fn bind(&mut self, declaration: Declaration, spelled: bool) -> usize {
        let id = self.declarations.len();
        self.scopes[declaration.scope].declarations.push(id);
        self.declaration_starts.insert(declaration.range.start, id);
        if spelled {
            self.occurrences.push(Occurrence {
                range: declaration.range.clone(),
                scope: declaration.scope,
                math: false,
                kind: OccurrenceKind::Declaration(id),
            });
        }
        self.declarations.push(declaration);
        id
    }

    fn path(
        &self,
        anchor: &LinkedNode<'_>,
        expression: ast::Expr<'_>,
        scope: usize,
    ) -> Option<NamePath> {
        fn segments(
            names: &SourceNames,
            anchor: &LinkedNode<'_>,
            expression: ast::Expr<'_>,
            found: &mut Vec<Range<usize>>,
        ) -> Option<bool> {
            match expression {
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
        let math = segments(self, anchor, expression, &mut found)?;
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
        expression: Option<ast::Expr<'_>>,
        scope: usize,
    ) -> Initializer {
        match expression {
            Some(ast::Expr::Closure(closure)) => Initializer::Function {
                body: self.range(anchor, closure.body()),
            },
            Some(expression) => self
                .path(anchor, expression, scope)
                .map_or(Initializer::Unknown, Initializer::Alias),
            None => Initializer::Unknown,
        }
    }

    fn walk(&mut self, node: &LinkedNode<'_>, scope: usize) {
        if let Some(binding) = node.cast::<ast::LetBinding>() {
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
                let initializer = if single {
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
                    initializer,
                ) {
                    self.declarations[id].recursive_body = recursive_body.clone();
                }
            }
            if let Some(init) = init.and_then(|init| node.find(init.span())) {
                self.walk(&init, scope);
            }
            return;
        }
        if let Some(closure) = node.cast::<ast::Closure>() {
            // Defaults evaluate in the enclosing scope, not in the parameter scope.
            for parameter in closure.params().children() {
                if let ast::Param::Named(named) = parameter
                    && let Some(default) = node.find(named.expr().span())
                {
                    self.walk(&default, scope);
                }
            }
            let body_range = self.range(node, closure.body());
            let child_scope = self.scope(scope, body_range.clone(), true);
            for parameter in closure.params().children() {
                let named_parameter = matches!(parameter, ast::Param::Named(_));
                let names = match parameter {
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
                    ) && named_parameter
                    {
                        self.named_parameters
                            .entry(body_range.start)
                            .or_default()
                            .insert(self.declarations[id].name.clone(), id);
                    }
                }
            }
            if let Some(body) = node.find(closure.body().span()) {
                self.walk(&body, child_scope);
            }
            return;
        }
        if let Some(loop_) = node.cast::<ast::ForLoop>() {
            if let Some(iterable) = node.find(loop_.iterable().span()) {
                self.walk(&iterable, scope);
            }
            let body_range = self.range(node, loop_.body());
            let child_scope = self.scope(scope, body_range.clone(), false);
            for name in loop_.pattern().bindings() {
                let _ = self.declare(
                    node,
                    name,
                    child_scope,
                    body_range.start,
                    DeclarationKind::Loop,
                    Initializer::Unknown,
                );
            }
            if let Some(body) = node.find(loop_.body().span()) {
                self.walk(&body, child_scope);
            }
            return;
        }
        if let Some(import) = node.cast::<ast::ModuleImport>() {
            let source_range = self.range(node, import.source());
            let source = match import.source() {
                ast::Expr::Str(path) => ImportSource::Path(path.get().to_string()),
                expression => self
                    .path(node, expression, scope)
                    .map_or(ImportSource::Unknown, ImportSource::Name),
            };
            if let Some(expression) = node.find(import.source().span()) {
                self.walk(&expression, scope);
            }
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
                    let declaration = self.declare(
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
                    if !explicit_alias && declaration.is_some() {
                        self.occurrences.pop();
                    }
                }
            }
            return;
        }
        if let Some(field) = node.cast::<ast::FieldAccess>() {
            if let Some(target) = node.find(field.target().span()) {
                self.walk(&target, scope);
            }
            if let Some(path) = self.path(node, field.target(), scope) {
                self.occurrences.push(Occurrence {
                    range: self.range(node, field.field()),
                    scope,
                    math: path.math,
                    kind: OccurrenceKind::Field(path),
                });
            }
            return;
        }
        if let Some(field) = node.cast::<ast::MathFieldAccess>() {
            if let Some(target) = node.find(field.target().span()) {
                self.walk(&target, scope);
            }
            if let Some(target) = field.target().to_untyped().cast::<ast::Expr>()
                && let Some(path) = self.path(node, target, scope)
            {
                self.occurrences.push(Occurrence {
                    range: self.range(node, field.field()),
                    scope,
                    math: path.math,
                    kind: OccurrenceKind::Field(path),
                });
            }
            return;
        }
        if let Some(named) = node.cast::<ast::Named>() {
            if let Some(call) = node.parent().and_then(|args| args.parent()) {
                let callee = match call.cast::<ast::FuncCall>() {
                    Some(call) => Some(call.callee()),
                    None => call
                        .cast::<ast::MathCall>()
                        .and_then(|call| call.callee().to_untyped().cast::<ast::Expr>()),
                };
                // A label is identifier syntax in a math argument list as well, so renaming keeps
                // the identifier rule rather than the math-variable rule.
                if let Some(callee) = callee.and_then(|callee| self.path(call, callee, scope)) {
                    self.occurrences.push(Occurrence {
                        range: self.range(node, named.name()),
                        scope,
                        math: false,
                        kind: OccurrenceKind::Argument(callee),
                    });
                }
            }
            if let Some(value) = node.find(named.expr().span()) {
                self.walk(&value, scope);
            }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst_syntax::{RootedPath, VirtualPath, VirtualRoot};

    fn declaration(marked: &str) -> (SourceNames, usize) {
        let cursor = marked.find('|').unwrap();
        let index = detached_index(&marked.replace('|', ""));
        let declaration = index.declared_at(cursor).expect("bound name");
        (index, declaration)
    }

    fn detached_index(text: &str) -> SourceNames {
        SourceNames::new(Source::detached(text))
    }

    fn file(path: &str) -> FileId {
        RootedPath::new(VirtualRoot::Project, VirtualPath::new(path).unwrap()).intern()
    }

    fn project(path: &str, text: &str) -> SourceNames {
        SourceNames::new(Source::new(file(path), text.to_owned()))
    }

    #[test]
    fn unread_imports_omit_bindings_this_source_reads() {
        let index = detached_index("#import \"lib.typ\": used, unused\n#used\n");
        let unread = index.unread_imports();
        assert_eq!(unread.len(), 1, "{unread:?}");
        assert_eq!(unread[0].names, ["unused"]);
        assert!(unread[0].file_scope);
        let ImportRemoval::Spans(spans) = &unread[0].removal else {
            panic!("the statement keeps an item: {:?}", unread[0].removal);
        };
        assert_eq!(spans.len(), 1);
        assert_eq!(index.text(&spans[0]), ", unused");
    }

    #[test]
    fn wildcard_imports_are_never_reported() {
        let index = detached_index("#import \"lib.typ\": *\n");
        assert!(index.unread_imports().is_empty());
    }

    #[test]
    fn module_imports_remove_their_whole_statement() {
        let index = detached_index("#import \"lib.typ\"\n");
        let unread = index.unread_imports();
        assert_eq!(unread.len(), 1, "{unread:?}");
        assert_eq!(unread[0].names, ["lib"]);
        assert_eq!(unread[0].removal, ImportRemoval::Statement);
        assert!(unread[0].file_scope);
    }

    #[test]
    fn shadowed_imports_are_unread() {
        let index = detached_index("#import \"lib.typ\": value\n#let value = 1\n#value\n");
        let unread = index.unread_imports();
        assert_eq!(unread.len(), 1, "{unread:?}");
        assert_eq!(unread[0].names, ["value"]);
    }

    #[test]
    fn nested_imports_are_not_file_interface() {
        let index = detached_index("#let greet() = { import \"lib.typ\": name; 1 }\n#greet()\n");
        let unread = index.unread_imports();
        assert_eq!(unread.len(), 1, "{unread:?}");
        assert!(!unread[0].file_scope);
    }

    #[test]
    fn selection_records_names_items_select() {
        let target = project("lib.typ", "");
        let page = project("page.typ", "#import \"lib.typ\": a.b as c, d\n");
        let interfaces = selected_interfaces([&page], |_, path| {
            (path == "lib.typ").then_some(target.source().id())
        });
        assert!(!interfaces.unresolved);
        let selected = &interfaces.selected[&target.source().id()];
        assert!(!selected.whole);
        assert_eq!(selected.names.iter().collect::<Vec<_>>(), ["a", "d"]);
    }

    #[test]
    fn module_and_wildcard_selection_reaches_every_name() {
        let target = project("lib.typ", "");
        let page = project("page.typ", "#import \"lib.typ\"\n#import \"lib.typ\": *\n");
        let interfaces = selected_interfaces([&page], |_, path| {
            (path == "lib.typ").then_some(target.source().id())
        });
        assert!(interfaces.selected[&target.source.id()].reaches("anything"));
    }

    #[test]
    fn nested_unread_imports_need_no_selection_evidence() {
        let index = detached_index("#let greet() = { import \"lib.typ\": name; 1 }\n");
        let reported = index.reportable_unread_imports(&SelectedInterfaces::default());
        assert_eq!(reported.len(), 1, "{reported:?}");
        assert_eq!(reported[0].names, ["name"]);
    }

    #[test]
    fn selection_evidence_withholds_file_scope_imports() {
        let source = project("lib.typ", "#import \"other.typ\": dropped\n");

        let reported = source.reportable_unread_imports(&SelectedInterfaces::default());
        assert_eq!(reported.len(), 1, "{reported:?}");
        assert_eq!(reported[0].names, ["dropped"]);

        let mut selected = SelectedInterfaces::default();
        selected
            .selected
            .insert(source.source().id(), SelectedNames::default());
        selected
            .selected
            .get_mut(&source.source().id())
            .unwrap()
            .names
            .insert("dropped".to_owned());
        assert!(
            source.reportable_unread_imports(&selected).is_empty(),
            "a selected name reaches the import"
        );

        let unresolved = SelectedInterfaces {
            unresolved: true,
            ..SelectedInterfaces::default()
        };
        assert!(
            source.reportable_unread_imports(&unresolved).is_empty(),
            "an unresolved source may reach any name"
        );
    }

    #[test]
    fn name_sources_leave_selection_unresolved() {
        let page = detached_index("#let module = \"lib.typ\"\n#import module: x\n");
        let interfaces = selected_interfaces([&page], |_, _| None);
        assert!(interfaces.unresolved);
        assert!(interfaces.selected.is_empty());
    }

    #[test]
    fn parameters_bind_every_pattern() {
        for marked in [
            "#let f(pos) = pos|",
            "#let f(named: 1) = named|",
            "#let f((left, (right, ..rest))) = right|",
            "#let f(..args) = args|",
            "#let f = ((x, y), flag: false, ..rest) => rest|",
        ] {
            let (index, id) = declaration(marked);
            assert!(
                matches!(index.declarations()[id].kind, DeclarationKind::Parameter),
                "{marked}"
            );
        }
    }

    #[test]
    fn named_functions_resolve_recursively() {
        let marked = "#let recurse(n) = if n > 0 { recurse|(n - 1) }";
        let (index, id) = declaration(marked);
        let at = marked.find("recurse").unwrap();
        assert_eq!(index.declarations()[id].range, at..at + "recurse".len());
    }

    #[test]
    fn defaults_use_enclosing_scope() {
        let (index, id) = declaration("#let count = 1\n#let f(count: count|) = count");
        assert!(matches!(
            index.declarations()[id].kind,
            DeclarationKind::Let
        ));
    }

    #[test]
    fn shadowing_ends_with_scope() {
        let marked = "#let value = 1\n#{ let value = 2; value }\n#value|";
        let (index, id) = declaration(marked);
        let outer = marked.find("value").unwrap();
        assert_eq!(index.declarations()[id].range, outer..outer + "value".len());
    }

    #[test]
    fn loop_patterns_exclude_iterable() {
        let (index, id) =
            declaration("#let values = ((1, 2),)\n#for (left, right) in values { right| }");
        assert!(matches!(
            index.declarations()[id].kind,
            DeclarationKind::Loop
        ));
    }

    #[test]
    fn statements_outside_function_bodies_evaluate_here() {
        for source in [
            "#import \"lib.typ\": greet\n#greet",
            "#{ import \"lib.typ\": greet }\n#greet",
            "#for _ in (1,) { import \"lib.typ\": greet }\n#greet",
        ] {
            let index = detached_index(source);
            let import = index.imports().first().expect("an import statement");
            assert!(index.evaluated_here(import.scope), "{source}");
        }
    }

    #[test]
    fn function_body_statements_defer_to_their_callers() {
        for source in [
            "#let partial(name) = { import name + \".typ\": render; render }\n#partial(\"a\")",
            "#let partial = (name) => { import name + \".typ\": render }\n#partial(\"a\")",
            "#let partial(name) = { let inner = { import name + \".typ\": render }; inner }",
        ] {
            let index = detached_index(source);
            let import = index.imports().first().expect("an import statement");
            assert!(!index.evaluated_here(import.scope), "{source}");
        }
    }

    #[test]
    fn only_lexical_reads_are_value_reads() {
        for (marked, reads) in [
            ("#let f(par|ameter: 1) = parameter\n", false),
            ("#let key = (ke|y: 1)\n", false),
            ("#let f(x) = x\n#f(lab|el: 1)\n", false),
            ("#let key = (a: 1)\n#key.fie|ld\n", false),
            ("#import \"lib.typ\": it|em\n", false),
            ("#let value = 1\n#val|ue\n", true),
        ] {
            let cursor = marked.find('|').expect("a marked cursor");
            let index = detached_index(&marked.replace('|', ""));
            assert_eq!(index.reads_value_at(cursor), reads, "{marked}");
        }
    }
}
