//! Source-bound declarations and lexical scopes from Typst's AST.

#[cfg(test)]
mod tests;
mod walk;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Range;

use typst_syntax::{FileId, LinkedNode, Source};

/// A lexical visibility boundary; a nested body never leaks its declarations outward.
#[derive(Clone, Debug)]
pub struct Scope {
    /// Enclosing scope, absent only at the file root.
    pub parent: Option<usize>,
    /// Half-open UTF-8 byte extent of the scope's body.
    pub range: Range<usize>,
    /// Declaration indices in binding order, with later entries able to shadow earlier ones.
    pub declarations: Vec<usize>,
    /// Same indices ordered by spelling, keeping binding order within each spelling.
    declarations_by_name: Vec<usize>,
    /// Set for a closure or named-function body, whose statements run at each call rather than
    /// during the file's own evaluation.
    pub per_call: bool,
}

impl Scope {
    fn declarations_named<'a>(&'a self, decls: &[Declaration], name: &str) -> &'a [usize] {
        let start = self
            .declarations_by_name
            .partition_point(|&id| decls[id].name.as_str() < name);
        let rest = &self.declarations_by_name[start..];
        let end = rest.partition_point(|&id| decls[id].name.as_str() == name);
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

impl Declaration {
    /// Whether this declaration is visible here within its owning scope.
    pub fn visible_at(&self, offset: usize) -> bool {
        self.visible_from <= offset
            || self
                .recursive_body
                .as_ref()
                .is_some_and(|body| body.contains(&offset))
    }
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
    for src in sources {
        for (stmt, import) in src.imports().iter().enumerate() {
            let path = match &import.source {
                ImportSource::Path(path) if !path.starts_with('@') => path,
                ImportSource::Path(_) => continue,
                _ => {
                    collected.unresolved = true;
                    continue;
                }
            };
            let Some(target) = resolve(src.source().id(), path) else {
                continue;
            };
            let selected = collected.selected.entry(target).or_default();
            if import.wildcard || import.items.is_empty() {
                selected.whole = true;
            } else {
                selected
                    .names
                    .extend(
                        src.declarations()
                            .iter()
                            .filter_map(|decl| match &decl.initializer {
                                Initializer::Import {
                                    import: owner,
                                    path,
                                } if *owner == stmt => path.first().cloned(),
                                _ => None,
                            }),
                    );
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
        index.occurrences.sort_by_key(|occ| occ.range.start);
        index.order_declarations_by_name();
        index
    }

    fn order_declarations_by_name(&mut self) {
        let decls = &self.declarations;
        for scope in &mut self.scopes {
            scope.declarations_by_name = scope.declarations.clone();
            scope
                .declarations_by_name
                .sort_by(|&left, &right| decls[left].name.cmp(&decls[right].name));
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
            .partition_point(|occ| occ.range.start <= cursor);
        self.occurrences[..after]
            .last()
            .filter(|occ| cursor <= occ.range.end)
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
        for occ in &self.occurrences {
            if matches!(occ.kind, OccurrenceKind::Name)
                && let Some(decl) = self.resolve(self.text(&occ.range), occ.scope, occ.range.start)
            {
                read.insert(decl);
            }
        }
        let mut unread: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (decl, spelled) in self.declarations.iter().enumerate() {
            if !read.contains(&decl)
                && let Initializer::Import { import, .. } = &spelled.initializer
            {
                unread.entry(*import).or_default().push(decl);
            }
        }
        unread
            .into_iter()
            .map(|(import, decls)| self.unread_import(import, &decls))
            .collect()
    }

    fn unread_import(&self, import: usize, unread: &[usize]) -> UnreadImport {
        let stmt = &self.imports[import];
        let removal = if stmt.items.is_empty() || stmt.items.len() == unread.len() {
            ImportRemoval::Statement
        } else {
            ImportRemoval::Spans(
                unread
                    .iter()
                    .filter_map(|&decl| self.item_removal(stmt, decl))
                    .collect(),
            )
        };
        UnreadImport {
            statement: stmt.range.clone(),
            file_scope: stmt.scope == 0,
            names: unread
                .iter()
                .map(|&decl| self.declarations[decl].name.clone())
                .collect(),
            removal,
        }
    }

    fn item_removal(&self, stmt: &Import, decl: usize) -> Option<Range<usize>> {
        let spelled = &self.declarations[decl].range;
        let idx = stmt
            .items
            .iter()
            .position(|item| item.start <= spelled.start && spelled.end <= item.end)?;
        let item = &stmt.items[idx];
        match stmt.items.get(idx + 1) {
            Some(next) => Some(item.start..next.start),
            None => Some(stmt.items[idx.checked_sub(1)?].end..item.end),
        }
    }

    pub(crate) fn declaration_beginning_at(&self, at: usize) -> Option<usize> {
        self.declaration_starts.get(&at).copied()
    }

    /// Returns a local declaration index; fields and external origins need a name graph.
    pub fn declared_at(&self, cursor: usize) -> Option<usize> {
        let occ = self.occurrence(cursor)?;
        match &occ.kind {
            OccurrenceKind::Declaration(decl) => Some(*decl),
            OccurrenceKind::Name => self.resolve(self.text(&occ.range), occ.scope, occ.range.start),
            OccurrenceKind::Import { .. } => self.declaration_beginning_at(occ.range.start),
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
                .find(|&id| self.declarations[id].visible_at(at));
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
        let mut cur = Some(scope);
        while let Some(id) = cur {
            if self.scopes[id].per_call {
                return false;
            }
            cur = self.scopes[id].parent;
        }
        true
    }
}
