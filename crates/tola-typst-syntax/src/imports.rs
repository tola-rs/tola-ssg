//! Import identities shared by navigation, rename, and name classification.

mod build;
mod pending;
mod query;
mod rename;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Range;
use std::sync::Arc;

use typst_library::Library;
use typst_library::foundations::{Module, Value};
use typst_syntax::FileId;

use crate::names::SourceNames;

/// A binding index valid only within the [`NameGraph`] instance that produced it.
pub type BindingId = usize;

/// Name categories established by declarations, import interfaces, or real library metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameClass {
    /// An official value whose metadata establishes neither a function nor a module.
    Value,
    /// A declared closure or a function supplied by an imported/library interface.
    Function,
    /// A module interface whose members can be selected by name.
    Namespace,
}

/// A local identity retained separately from the external export it may reference.
#[derive(Debug)]
pub struct Binding {
    /// Source introducing this local identity, not necessarily its ultimate definition.
    pub file: FileId,
    /// Index in the source's declarations; absent for a name introduced only by `*`.
    pub declaration: Option<usize>,
    /// Import-statement index for a wildcard expansion; absent for explicit declarations.
    pub wildcard: Option<usize>,
    /// Spelling visible in the importing/local scope, including any alias.
    pub name: String,
}

/// A name's declaration, established by source identity or official static library metadata.
pub enum NameDefinition {
    /// The source snapshot and its declaration index, valid only in this graph.
    Source {
        /// The immutable source snapshot containing the declaration.
        file: FileId,
        /// The declaration's index inside that snapshot's names.
        declaration: usize,
    },
    /// A library value whose metadata needs no source evaluation.
    Builtin(Value),
}

/// One visible module export with the declaration identity this graph established.
pub struct ModuleExport {
    /// The spelling callers select from the module.
    pub name: String,
    /// The category established by source or static metadata.
    pub class: NameClass,
    /// The declaration or static value, absent for an unresolved import cycle.
    pub definition: Option<NameDefinition>,
}

/// A source spelling annotated against one immutable dependency view.
#[derive(Clone, Debug)]
pub struct NameOccurrence {
    /// UTF-8 byte range in the file passed to [`NameGraph::occurrences`].
    pub range: Range<usize>,
    /// Source-backed identity, absent for unknown names and native library values.
    pub binding: Option<BindingId>,
    /// Known metadata category; absence deliberately leaves dynamic values unclassified.
    pub class: Option<NameClass>,
    /// Whether the spelling introduces a local binding, including an unaliased import item.
    pub declaration: bool,
}

/// A rename replacement against the original source, never a progressively edited buffer.
#[derive(Debug)]
pub struct NameEdit {
    /// Source whose original text the range addresses.
    pub file: FileId,
    /// Half-open UTF-8 byte range; an empty range inserts alias syntax.
    pub range: Range<usize>,
    /// Replacement may include `as` syntax rather than only the requested identifier.
    pub replacement: String,
}

/// Rejections that prevent a rename from changing name resolution or immutable packages.
#[derive(Clone, Debug, thiserror::Error)]
pub enum RenameError {
    /// The requested spelling is not one identifier in the official code grammar.
    #[error("`{0}` is not a Typst identifier")]
    InvalidIdentifier(String),
    /// A code identifier would become a different expression at a bare math use.
    #[error("`{0}` would not resolve as this binding in math")]
    InvalidMathIdentifier(String),
    /// A declaration would collide, or an existing use would resolve to another binding.
    #[error("renaming to `{0}` would capture or collide with another name")]
    Conflict(String),
    /// An import whose target is not established could spell the name in another file.
    #[error("Tola could not rename across the import in `{0}`; resolve the import first")]
    UnknownImport(String),
    /// The selected declaration belongs to a package rather than the site's own sources.
    #[error("package declarations are read-only; rename an imported local binding instead")]
    ReadOnly,
}

#[derive(Clone)]
enum NameValue {
    Unknown,
    Function { file: FileId, body: usize },
    Module(FileId),
    Builtin(Value),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NameOrigin {
    Declaration(BindingId),
    ImportCycle(BindingId),
}

/// One import source a host resolver can establish a target for.
///
/// A literal path is decoded by the source index itself; a name or expression the index cannot
/// decode is the statement the host must evaluate through the official compiler semantics.
#[derive(Clone, Debug)]
pub enum ImportQuery<'a> {
    /// A decoded literal path, relative to the file that spells it.
    Path {
        /// File holding the import statement.
        file: FileId,
        /// Decoded string the statement names.
        path: &'a str,
    },
    /// An import statement whose source the index cannot decode itself.
    Expression {
        /// File holding the import statement.
        file: FileId,
        /// Complete source-expression bytes within that file.
        range: Range<usize>,
    },
}

/// One import statement whose target only the official compiler semantics can establish.
///
/// The graph reports these from [`NameGraph::unresolved_item`] and
/// [`NameGraph::unresolved_imports`]; a host that can prove the target resolves them, then
/// supplies the answers to a second graph as [`ImportQuery`] results.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingImport {
    /// File holding the statement.
    pub file: FileId,
    /// Index of the statement in that file's imports.
    pub statement: usize,
}

/// Source/import identities shared by queries for one immutable set of source versions.
pub struct NameGraph {
    sources: HashMap<FileId, Arc<SourceNames>>,
    files: Vec<FileId>,
    bindings: Vec<Binding>,
    locals: HashMap<FileId, Vec<BindingId>>,
    lexical: HashMap<(FileId, usize), BTreeMap<String, Vec<BindingId>>>,
    values: Vec<NameValue>,
    imported: Vec<Option<BindingId>>,
    origins: Vec<NameOrigin>,
    wildcards: HashMap<(FileId, usize), BTreeMap<String, BindingId>>,
    targets: HashMap<(FileId, usize), FileId>,
    exports: HashMap<FileId, BTreeMap<String, BindingId>>,
    occurrences: HashMap<FileId, Vec<NameOccurrence>>,
    global: Option<Module>,
    math: Option<Module>,
    standard: Option<Value>,
}

impl NameGraph {
    /// Resolves supplied sources without host I/O or expression evaluation.
    ///
    /// `resolve` establishes the target of each import statement the graph asks about: a literal
    /// path from the source index, or an expression the host confirmed through the official
    /// compiler semantics. `library` supplies real native metadata. `check` may abort
    /// construction, propagating its error without a partial graph.
    pub fn new<E>(
        sources: impl IntoIterator<Item = Arc<SourceNames>>,
        resolve: impl Fn(ImportQuery<'_>) -> Option<FileId>,
        library: Option<&Library>,
        check: impl Fn() -> Result<(), E>,
    ) -> Result<Self, E> {
        let sources: HashMap<_, _> = sources
            .into_iter()
            .map(|src| (src.source().id(), src))
            .collect();
        let mut files: Vec<_> = sources.keys().copied().collect();
        files.sort_unstable_by_key(|&file| build::file_order(file));
        let mut graph = Self {
            sources,
            files,
            bindings: Vec::new(),
            locals: HashMap::new(),
            wildcards: HashMap::new(),
            targets: HashMap::new(),
            imported: Vec::new(),
            origins: Vec::new(),
            lexical: HashMap::new(),
            values: Vec::new(),
            exports: HashMap::new(),
            occurrences: HashMap::new(),
            global: library.map(|lib| lib.global.clone()),
            math: library.map(|lib| lib.math.clone()),
            standard: library.map(|lib| lib.std.read().clone()),
        };
        graph.index_declarations(&resolve, &check)?;
        graph.refresh_exports();
        graph.expand_wildcards(&check)?;
        graph.values = (0..graph.bindings.len())
            .map(|id| {
                check()?;
                Ok(graph.value(id, &mut BTreeSet::new()))
            })
            .collect::<Result<_, E>>()?;
        graph.resolve_origins(&check)?;
        graph.index_occurrences(&check)?;
        Ok(graph)
    }

    /// Returns the exact snapshot addressed by this graph's ranges for `file`.
    pub fn source(&self, file: FileId) -> Option<&SourceNames> {
        self.sources.get(&file).map(AsRef::as_ref)
    }

    /// Iterates distinct rooted paths independently of interner or hash-map order.
    pub fn sources(&self) -> impl Iterator<Item = &SourceNames> {
        self.files.iter().map(|file| self.sources[file].as_ref())
    }

    /// Looks up a binding index produced by this graph; indices from other graphs are invalid.
    pub fn binding(&self, id: BindingId) -> &Binding {
        &self.bindings[id]
    }

    /// Returns source-order spellings, or an empty slice for an unindexed file.
    pub fn occurrences(&self, file: FileId) -> &[NameOccurrence] {
        self.occurrences.get(&file).map_or(&[], Vec::as_slice)
    }

    /// Selects an identifier at a byte cursor, including its trailing boundary.
    pub fn selected(&self, file: FileId, cursor: usize) -> Option<&NameOccurrence> {
        let occs = self.occurrences(file);
        let after = occs.partition_point(|occ| occ.range.start <= cursor);
        occs[..after].last().filter(|occ| cursor <= occ.range.end)
    }

    /// Classifies only an exact indexed spelling, never an arbitrary overlapping range.
    pub fn class_at(&self, file: FileId, range: &Range<usize>) -> Option<NameClass> {
        let occ = self.selected(file, range.start)?;
        (occ.range == *range).then_some(occ.class).flatten()
    }
}
