//! Import identities shared by navigation, rename, and name classification.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Range;
use std::sync::Arc;

use typst_library::Library;
use typst_library::foundations::{Module, Value};
use typst_library::math::Mathy;
use typst_syntax::{FileId, VirtualRoot};

use crate::names::{
    DeclarationKind, ImportSource, Initializer, NamePath, OccurrenceKind, SourceNames,
};

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

/// A source spelling annotated against one immutable dependency view.
#[derive(Clone, Debug)]
pub struct ResolvedOccurrence {
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
#[derive(Debug)]
pub enum RenameError {
    /// The requested spelling is not one identifier in the official code grammar.
    InvalidIdentifier(String),
    /// A code identifier would become a different expression at a bare math use.
    InvalidMathIdentifier(String),
    /// A declaration would collide, or an existing use would resolve to another binding.
    Conflict(String),
    /// An import whose target is not established could spell the name in another file.
    UnknownImport(String),
    /// The selected declaration belongs to a package rather than the site's own sources.
    ReadOnly,
}

impl std::fmt::Display for RenameError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIdentifier(name) => {
                write!(formatter, "`{name}` is not a Typst identifier")
            }
            Self::InvalidMathIdentifier(name) => write!(
                formatter,
                "`{name}` would not resolve as this binding in math"
            ),
            Self::Conflict(name) => write!(
                formatter,
                "renaming to `{name}` would capture or collide with another name"
            ),
            Self::UnknownImport(file) => write!(
                formatter,
                "Tola could not rename across the import in `{file}`; resolve the import first"
            ),
            Self::ReadOnly => formatter.write_str(
                "package declarations are read-only; rename an imported local binding instead",
            ),
        }
    }
}
impl std::error::Error for RenameError {}

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
    occurrences: HashMap<FileId, Vec<ResolvedOccurrence>>,
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
            .map(|source| (source.source().id(), source))
            .collect();
        let mut files: Vec<_> = sources.keys().copied().collect();
        files.sort_unstable_by_key(|&file| file_order(file));
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
            global: library.map(|library| library.global.clone()),
            math: library.map(|library| library.math.clone()),
            standard: library.map(|library| library.std.read().clone()),
        };
        for &file in &graph.files {
            let source = &graph.sources[&file];
            check()?;
            let mut locals = Vec::with_capacity(source.declarations().len());
            for (declaration, name) in source.declarations().iter().enumerate() {
                locals.push(graph.bindings.len());
                graph.bindings.push(Binding {
                    file,
                    declaration: Some(declaration),
                    wildcard: None,
                    name: name.name.clone(),
                });
            }
            graph.locals.insert(file, locals);
            for (import, declaration) in source.imports().iter().enumerate() {
                let query = match &declaration.source {
                    ImportSource::Path(path) => ImportQuery::Path { file, path },
                    ImportSource::Name(_) | ImportSource::Unknown => ImportQuery::Expression {
                        file,
                        range: declaration.source_range.clone(),
                    },
                };
                if let Some(target) = resolve(query) {
                    graph.targets.insert((file, import), target);
                }
            }
        }
        graph.refresh_exports();
        // A wildcard component reaches a fixed point over a finite set of exported spellings.
        loop {
            check()?;
            let mut additions = Vec::new();
            for &file in &graph.files {
                let source = &graph.sources[&file];
                check()?;
                for (import, _) in source
                    .imports()
                    .iter()
                    .enumerate()
                    .filter(|(_, import)| import.wildcard)
                {
                    let mut add = |name: &str| {
                        if !graph
                            .wildcards
                            .get(&(file, import))
                            .is_some_and(|names| names.contains_key(name))
                        {
                            additions.push((file, import, name.to_owned()));
                        }
                    };
                    match graph.import_value(file, import, &mut BTreeSet::new()) {
                        NameValue::Module(target) => {
                            if let Some(exports) = graph.exports.get(&target) {
                                for name in exports.keys() {
                                    add(name);
                                }
                            }
                        }
                        NameValue::Builtin(value) => {
                            if let Some(scope) = value.scope() {
                                for (name, _) in scope.iter() {
                                    add(name.as_str());
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            if additions.is_empty() {
                break;
            }
            for (file, import, name) in additions {
                let id = graph.bindings.len();
                graph.bindings.push(Binding {
                    file,
                    declaration: None,
                    wildcard: Some(import),
                    name: name.clone(),
                });
                graph
                    .wildcards
                    .entry((file, import))
                    .or_default()
                    .insert(name, id);
            }
            graph.refresh_exports();
        }
        graph.values = (0..graph.bindings.len())
            .map(|id| {
                check()?;
                Ok(graph.value(id, &mut BTreeSet::new()))
            })
            .collect::<Result<_, E>>()?;
        graph.resolve_origins(&check)?;
        for &file in &graph.files {
            let source = &graph.sources[&file];
            check()?;
            let declared: BTreeMap<usize, usize> = source
                .declarations()
                .iter()
                .enumerate()
                .map(|(id, declaration)| (declaration.range.start, id))
                .collect();
            let occurrences = source
                .occurrences()
                .iter()
                .map(|occurrence| {
                    check()?;
                    let (binding, value) = match &occurrence.kind {
                        OccurrenceKind::Declaration(declaration) => {
                            let binding = graph.locals[&file][*declaration];
                            (Some(binding), graph.value(binding, &mut BTreeSet::new()))
                        }
                        OccurrenceKind::Name => {
                            let name = source.text(&occurrence.range);
                            let binding = graph.resolve(
                                file,
                                name,
                                occurrence.scope,
                                occurrence.range.start,
                                &BTreeSet::new(),
                                "",
                            );
                            let value = binding.map_or_else(
                                || graph.builtin(name, occurrence.math),
                                |binding| graph.value(binding, &mut BTreeSet::new()),
                            );
                            (binding, value)
                        }
                        OccurrenceKind::Field(path) => graph.field(
                            graph.path_value(file, path, &mut BTreeSet::new()),
                            source.text(&occurrence.range),
                            &mut BTreeSet::new(),
                        ),
                        OccurrenceKind::Argument(callee) => {
                            let binding = match graph.path_value(file, callee, &mut BTreeSet::new())
                            {
                                NameValue::Function { file: owner, body } => graph.sources[&owner]
                                    .named_parameters()
                                    .get(&body)
                                    .and_then(|parameters| {
                                        parameters.get(source.text(&occurrence.range))
                                    })
                                    .map(|&declaration| graph.locals[&owner][declaration]),
                                _ => None,
                            };
                            (binding, NameValue::Unknown)
                        }
                        OccurrenceKind::Import { import, path } => {
                            let target =
                                graph.import_path(file, *import, path, &mut BTreeSet::new());
                            let local = declared.get(&occurrence.range.start).copied();
                            match local {
                                Some(local) => (Some(graph.locals[&file][local]), target.1),
                                None => target,
                            }
                        }
                    };
                    Ok(ResolvedOccurrence {
                        range: occurrence.range.clone(),
                        binding,
                        class: class(&value),
                        declaration: matches!(occurrence.kind, OccurrenceKind::Declaration(_))
                            || declared.contains_key(&occurrence.range.start),
                    })
                })
                .collect::<Result<_, E>>()?;
            graph.occurrences.insert(file, occurrences);
        }
        Ok(graph)
    }

    fn resolve_origins<E>(&mut self, check: &impl Fn() -> Result<(), E>) -> Result<(), E> {
        self.imported = (0..self.bindings.len())
            .map(|id| {
                check()?;
                Ok(self.imported_binding(id))
            })
            .collect::<Result<_, E>>()?;
        let mut origins = vec![None; self.bindings.len()];
        let mut positions = vec![usize::MAX; self.bindings.len()];
        let mut path = Vec::new();
        for start in 0..self.bindings.len() {
            check()?;
            if origins[start].is_some() {
                continue;
            }
            let mut id = start;
            let origin = loop {
                if let Some(origin) = origins[id] {
                    break origin;
                }
                if positions[id] != usize::MAX {
                    // A cyclic import has a shared identity, but no invented definition.
                    break NameOrigin::ImportCycle(*path[positions[id]..].iter().min().unwrap());
                }
                positions[id] = path.len();
                path.push(id);
                match self.imported[id] {
                    Some(target) => id = target,
                    None => break NameOrigin::Declaration(id),
                }
            };
            for id in path.drain(..) {
                origins[id] = Some(origin);
                positions[id] = usize::MAX;
            }
        }
        self.origins = origins.into_iter().map(Option::unwrap).collect();
        Ok(())
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
    pub fn occurrences(&self, file: FileId) -> &[ResolvedOccurrence] {
        self.occurrences.get(&file).map_or(&[], Vec::as_slice)
    }

    /// Selects an identifier at a byte cursor, including its trailing boundary.
    pub fn selected(&self, file: FileId, cursor: usize) -> Option<&ResolvedOccurrence> {
        let occurrences = self.occurrences(file);
        let after = occurrences.partition_point(|occurrence| occurrence.range.start <= cursor);
        occurrences[..after]
            .last()
            .filter(|occurrence| cursor <= occurrence.range.end)
    }

    /// Classifies only an exact indexed spelling, never an arbitrary overlapping range.
    pub fn class_at(&self, file: FileId, range: &Range<usize>) -> Option<NameClass> {
        let occurrence = self.selected(file, range.start)?;
        (occurrence.range == *range)
            .then_some(occurrence.class)
            .flatten()
    }

    /// Follows imports to a declaration; an import-only cycle has no invented definition.
    ///
    /// An item imported through an interface this graph never resolved has no definition here
    /// either: the statement spells an external export, and the compiler remains its authority.
    pub fn definition(&self, binding: BindingId) -> Option<(FileId, Range<usize>)> {
        let binding = self.canonical(binding);
        let NameOrigin::Declaration(id) = binding else {
            return None;
        };
        if self.unresolved_item_of(id).is_some() {
            return None;
        }
        let binding = &self.bindings[id];
        let source = self.source(binding.file)?;
        let range = match binding.declaration {
            Some(declaration) => source.declarations()[declaration].range.clone(),
            None => source.imports()[binding.wildcard?].range.clone(),
        };
        Some((binding.file, range))
    }

    /// `id`'s declaration is an item of a statement whose interface is still unknown.
    fn unresolved_item_of(&self, id: BindingId) -> Option<PendingImport> {
        let binding = &self.bindings[id];
        let declaration = binding.declaration?;
        let Initializer::Import { import, path } =
            &self.sources[&binding.file].declarations()[declaration].initializer
        else {
            return None;
        };
        if path.is_empty() || self.imported_binding(id).is_some() {
            return None;
        }
        Some(PendingImport {
            file: binding.file,
            statement: *import,
        })
    }

    /// The statement one binding's identity still needs, when the binding is an item of an
    /// import whose interface the graph could not resolve.
    pub fn unresolved_item(&self, binding: BindingId) -> Option<PendingImport> {
        let NameOrigin::Declaration(id) = self.canonical(binding) else {
            return None;
        };
        self.unresolved_item_of(id)
    }

    /// Collects spellings sharing one import origin in rooted-path and source order.
    pub fn references(
        &self,
        binding: BindingId,
        include_declaration: bool,
    ) -> Vec<(FileId, &ResolvedOccurrence)> {
        let canonical = self.canonical(binding);
        self.files
            .iter()
            .flat_map(|&file| {
                self.occurrences(file)
                    .iter()
                    .map(move |occurrence| (file, occurrence))
            })
            .filter(|(_, occurrence)| {
                (include_declaration || !occurrence.declaration)
                    && occurrence
                        .binding
                        .is_some_and(|binding| self.canonical(binding) == canonical)
            })
            .collect()
    }

    /// The statements whose official target the identity at `cursor` still needs, in file and
    /// source order.
    ///
    /// Empty when the graph decides the spelling on its own: a literal path and a statement a
    /// host already proved are settled, and a local rename of an unresolved item stays complete
    /// because the graph keeps the external spelling by inserting an explicit alias.
    pub fn unresolved_imports(&self, file: FileId, cursor: usize) -> Vec<PendingImport> {
        let Some(selected) = self.selected(file, cursor) else {
            return Vec::new();
        };
        let Some(binding) = selected.binding else {
            return self.unresolved_carriers(file, &selected.range);
        };
        let NameOrigin::Declaration(id) = self.canonical(binding) else {
            return Vec::new();
        };
        let declared = &self.bindings[id];
        let mut pending = Vec::new();
        for (file, statement) in self.pending_statements() {
            // A package statement resolves inside its own package or the engine module Tola
            // injects, so no statement of a package can carry a site document's export.
            if matches!(file.root(), VirtualRoot::Package(_))
                && matches!(declared.file.root(), VirtualRoot::Project)
            {
                continue;
            }
            let source = &self.sources[&file];
            // The spelling's own identity is an item of this statement.
            let own = self.unresolved_item_of(id) == Some(PendingImport { file, statement });
            // The statement spells this declaration's name as an export it carries, through an
            // item or a module member, and the interface it spells it through is still unknown.
            let carries = self
                .carried_names(file, statement)
                .contains(declared.name.as_str());
            // A wildcard statement whose interface the graph never established can carry any
            // export of this file.
            let wildcard = source.imports()[statement].wildcard
                && self
                    .exports
                    .get(&declared.file)
                    .and_then(|names| names.get(&declared.name))
                    .is_some_and(|export| *export == id)
                && !self.interface_established(file, statement);
            if own || carries || wildcard {
                pending.push(PendingImport { file, statement });
            }
        }
        pending
    }

    /// The statements whose interface a spelling the graph cannot bind could arrive through.
    ///
    /// A wildcard whose interface the graph never established can bind any name this file leaves
    /// unresolved, and a module binding one introduces can supply the member a field read spells.
    /// A spelling the graph resolved is not one of them: a known value is evidence, not an
    /// unknown, and an argument label or an import path segment arrives through neither.
    fn unresolved_carriers(&self, file: FileId, range: &Range<usize>) -> Vec<PendingImport> {
        let source = &self.sources[&file];
        let Some((written, resolved)) = source
            .occurrences()
            .iter()
            .zip(self.occurrences(file))
            .find(|(written, _)| written.range == *range)
        else {
            return Vec::new();
        };
        if resolved.binding.is_some() || resolved.class.is_some() {
            return Vec::new();
        }
        let member_of = match &written.kind {
            OccurrenceKind::Name => None,
            OccurrenceKind::Field(path) => self.root_binding(file, path),
            _ => return Vec::new(),
        };
        let module_statements: BTreeSet<usize> = source
            .declarations()
            .iter()
            .filter_map(|spelled| match &spelled.initializer {
                Initializer::Import { import, path } if path.is_empty() => Some(*import),
                _ => None,
            })
            .collect();
        let mut pending = Vec::new();
        for (statement, import) in source.imports().iter().enumerate() {
            if self.interface_established(file, statement) {
                continue;
            }
            let reached = match member_of {
                None => {
                    import.wildcard
                        && import.range.end <= range.start
                        && scope_reaches(source, import.scope, range.start)
                }
                Some(root) => {
                    module_statements.contains(&statement)
                        && self.module_carriers(file, statement).contains(&root)
                }
            };
            if reached {
                pending.push(PendingImport { file, statement });
            }
        }
        pending
    }

    /// Every statement whose source the index did not resolve itself, in file and source order.
    ///
    /// A literal path is the source index's own evidence; a name or an expression it cannot
    /// decode still needs the official compiler's target.
    fn pending_statements(&self) -> Vec<(FileId, usize)> {
        let mut pending = Vec::new();
        for &file in &self.files {
            let source = &self.sources[&file];
            for (statement, import) in source.imports().iter().enumerate() {
                if !matches!(import.source, ImportSource::Path(_))
                    && !self.targets.contains_key(&(file, statement))
                {
                    pending.push((file, statement));
                }
            }
        }
        pending
    }

    /// Renames a local binding or site export while preserving explicit import aliases.
    ///
    /// Imported local names gain an alias instead of changing their external export. Invalid
    /// identifiers, capture, collisions, package declarations, and an export whose consumer
    /// spellings an unresolved import leaves unknown return [`RenameError`].
    pub fn rename(&self, binding: BindingId, new_name: &str) -> Result<Vec<NameEdit>, RenameError> {
        if !valid_identifier(new_name) {
            return Err(RenameError::InvalidIdentifier(new_name.into()));
        }
        let selected = &self.bindings[binding];
        if !matches!(selected.file.root(), VirtualRoot::Project) {
            return Err(RenameError::ReadOnly);
        }
        if selected.name == new_name {
            return Ok(Vec::new());
        }
        let mut renamed = BTreeSet::from([binding]);
        loop {
            let before = renamed.len();
            for (id, candidate) in self.bindings.iter().enumerate() {
                if !self.explicit_alias(id)
                    && self
                        .imported_binding(id)
                        .is_some_and(|target| renamed.contains(&target))
                    && matches!(candidate.file.root(), VirtualRoot::Project)
                {
                    renamed.insert(id);
                }
            }
            if before == renamed.len() {
                break;
            }
        }
        self.reject_capture(&renamed, new_name)?;
        if let Some(file) = self.unfinished_consumers(&renamed) {
            return Err(RenameError::UnknownImport(
                file.vpath().get_without_slash().to_owned(),
            ));
        }
        let mut edits: HashMap<FileId, BTreeMap<(usize, usize), String>> = HashMap::new();
        for &file in &self.files {
            let source = &self.sources[&file];
            if !matches!(file.root(), VirtualRoot::Project) {
                continue;
            }
            for (occurrence, resolved) in source.occurrences().iter().zip(self.occurrences(file)) {
                let changed = match &occurrence.kind {
                    OccurrenceKind::Import { import, path } => self
                        .import_path(file, *import, path, &mut BTreeSet::new())
                        .0
                        .is_some_and(|target| renamed.contains(&target)),
                    _ => resolved
                        .binding
                        .is_some_and(|target| renamed.contains(&target)),
                };
                if changed {
                    edits.entry(file).or_default().insert(
                        (occurrence.range.start, occurrence.range.end),
                        new_name.into(),
                    );
                }
            }
        }
        for &id in &renamed {
            let candidate = &self.bindings[id];
            let source = self.source(candidate.file).unwrap();
            if let Some(declaration) = candidate.declaration {
                let declaration = &source.declarations()[declaration];
                let replacement = match declaration.kind {
                    DeclarationKind::Import {
                        explicit_alias: false,
                        implicit: true,
                        ..
                    } => {
                        edits.entry(candidate.file).or_default().insert(
                            (declaration.range.end, declaration.range.end),
                            format!(" as {new_name}"),
                        );
                        continue;
                    }
                    DeclarationKind::Import {
                        explicit_alias: false,
                        ..
                    } if id == binding
                        && self
                            .imported_binding(id)
                            .is_none_or(|target| !renamed.contains(&target)) =>
                    {
                        format!("{} as {new_name}", declaration.name)
                    }
                    _ => new_name.into(),
                };
                edits.entry(candidate.file).or_default().insert(
                    (declaration.range.start, declaration.range.end),
                    replacement,
                );
            }
        }
        // A wildcard has no local spelling to replace; add one explicit alias beside it.
        if let Some(import) = selected.wildcard {
            let source = self.source(selected.file).unwrap();
            let import = &source.imports()[import];
            let prefix = if source.source().text()[..import.range.start].ends_with('#') {
                "\n#import "
            } else {
                "; import "
            };
            let replacement = format!(
                "{prefix}{}: {} as {new_name}",
                source.text(&import.source_range),
                selected.name
            );
            edits
                .entry(selected.file)
                .or_default()
                .insert((import.range.end, import.range.end), replacement);
        }
        Ok(self
            .files
            .iter()
            .filter_map(|file| edits.remove(file).map(|edits| (*file, edits)))
            .flat_map(|(file, edits)| {
                edits
                    .into_iter()
                    .map(move |((start, end), replacement)| NameEdit {
                        file,
                        range: start..end,
                        replacement,
                    })
            })
            .collect())
    }

    /// The first statement whose consumer spelling this rename would have to edit while its
    /// target stays unknown.
    ///
    /// A statement in a renamed binding's own file cannot import that file, and one in a package
    /// resolves inside its own package or the engine module Tola injects, so only another site
    /// file's statement leaves the edit set incomplete.
    fn unfinished_consumers(&self, renamed: &BTreeSet<BindingId>) -> Option<FileId> {
        for (file, statement) in self.pending_statements() {
            if !matches!(file.root(), VirtualRoot::Project) {
                continue;
            }
            let source = &self.sources[&file];
            let mut spelled = self.carried_names(file, statement);
            if source.imports()[statement].wildcard && !self.interface_established(file, statement)
            {
                // A wildcard whose interface the graph never established can bind any name this
                // file leaves unresolved.
                spelled.extend(
                    source
                        .occurrences()
                        .iter()
                        .zip(self.occurrences(file))
                        .filter(|(_, resolved)| {
                            resolved.binding.is_none() && resolved.class.is_none()
                        })
                        .map(|(occurrence, _)| source.text(&occurrence.range)),
                );
            }
            for name in spelled {
                let renames_it = renamed.iter().any(|&id| {
                    let binding = &self.bindings[id];
                    binding.file != file
                        && binding.name == name
                        && self
                            .exports
                            .get(&binding.file)
                            .and_then(|names| names.get(name))
                            .is_some_and(|export| renamed.contains(export))
                });
                if renames_it {
                    return Some(file);
                }
            }
        }
        None
    }

    /// The names one pending statement can carry from an interface this graph never established.
    ///
    /// An item the graph could not bind spells every segment of its path, because each one names a
    /// member of the unknown target; a module binding the statement introduces spells the members
    /// its field reads reach, in every file that holds the value. A spelling the graph resolved is
    /// absent: the edit set already addresses the name it resolves.
    fn carried_names(&self, file: FileId, statement: usize) -> BTreeSet<&str> {
        let source = &self.sources[&file];
        let mut carried = BTreeSet::new();
        for (declaration, spelled) in source.declarations().iter().enumerate() {
            let Initializer::Import { import, path } = &spelled.initializer else {
                continue;
            };
            if *import == statement
                && !path.is_empty()
                && self
                    .imported_binding(self.locals[&file][declaration])
                    .is_none()
            {
                carried.extend(path.iter().map(String::as_str));
            }
        }
        if self.interface_established(file, statement) {
            return carried;
        }
        let carriers = self.module_carriers(file, statement);
        let mut holders: Vec<(FileId, BTreeSet<BindingId>)> = Vec::new();
        for &carrier in &carriers {
            let holder = self.bindings[carrier].file;
            match holders.iter_mut().find(|(file, _)| *file == holder) {
                Some((_, roots)) => {
                    roots.insert(carrier);
                }
                None => holders.push((holder, BTreeSet::from([carrier]))),
            }
        }
        for (holder, carriers) in holders {
            let source = &self.sources[&holder];
            carried.extend(
                source
                    .occurrences()
                    .iter()
                    .zip(self.occurrences(holder))
                    .filter_map(|(occurrence, resolved)| {
                        let OccurrenceKind::Field(path) = &occurrence.kind else {
                            return None;
                        };
                        (resolved.binding.is_none()
                            && self
                                .root_binding(holder, path)
                                .is_some_and(|root| carriers.contains(&root)))
                        .then(|| source.text(&occurrence.range))
                    }),
            );
        }
        carried
    }

    /// The bindings that hold one statement's unknown module value, in every file: the module
    /// binding it introduces, and every binding that aliases or imports the same value.
    fn module_carriers(&self, file: FileId, statement: usize) -> BTreeSet<BindingId> {
        let mut carriers: BTreeSet<BindingId> = self.sources[&file]
            .declarations()
            .iter()
            .enumerate()
            .filter(|(_, spelled)| {
                matches!(
                    &spelled.initializer,
                    Initializer::Import { import, path } if *import == statement && path.is_empty()
                )
            })
            .map(|(declaration, _)| self.locals[&file][declaration])
            .collect();
        if carriers.is_empty() {
            return carriers;
        }
        // Every binding that takes the value of another: an item of some file's interface, or a
        // local alias of it. Following these edges reaches the files the value was passed to.
        let mut carried_by: Vec<(BindingId, BindingId)> = Vec::new();
        for (id, binding) in self.bindings.iter().enumerate() {
            if let Some(target) = self.imported_binding(id) {
                carried_by.push((target, id));
            }
            let Some(declaration) = binding.declaration else {
                continue;
            };
            if let Initializer::Alias(alias) =
                &self.sources[&binding.file].declarations()[declaration].initializer
                && let Some(root) = self.root_binding(binding.file, alias)
            {
                carried_by.push((root, id));
            }
        }
        loop {
            let before = carriers.len();
            for &(target, carrier) in &carried_by {
                if carriers.contains(&target) {
                    carriers.insert(carrier);
                }
            }
            if carriers.len() == before {
                break;
            }
        }
        carriers
    }

    /// The local binding a name path's first segment resolves to.
    fn root_binding(&self, file: FileId, path: &NamePath) -> Option<BindingId> {
        let first = path.segments.first()?;
        self.resolve(
            file,
            self.sources[&file].text(first),
            path.scope,
            path.at,
            &BTreeSet::new(),
            "",
        )
    }

    /// Whether the graph established every name one statement's interface binds.
    ///
    /// A module whose source this graph holds enumerates its exports, a native value enumerates
    /// its own scope, and a target the graph never read enumerates nothing.
    fn interface_established(&self, file: FileId, statement: usize) -> bool {
        match self.import_value(file, statement, &mut BTreeSet::new()) {
            NameValue::Module(target) => self.sources.contains_key(&target),
            NameValue::Builtin(value) => value.scope().is_some(),
            NameValue::Function { .. } | NameValue::Unknown => false,
        }
    }

    fn reject_capture(
        &self,
        renamed: &BTreeSet<BindingId>,
        new_name: &str,
    ) -> Result<(), RenameError> {
        for &id in renamed {
            let binding = &self.bindings[id];
            let (scope, _) = self.visibility(id);
            for (other, candidate) in self.bindings.iter().enumerate() {
                if !renamed.contains(&other)
                    && candidate.file == binding.file
                    && candidate.name == new_name
                    && self.visibility(other).0 == scope
                {
                    return Err(RenameError::Conflict(new_name.into()));
                }
            }
        }
        let mut valid_math_name = None;
        for &file in &self.files {
            let source = &self.sources[&file];
            for (occurrence, resolved) in source.occurrences().iter().zip(self.occurrences(file)) {
                if occurrence.math
                    && resolved
                        .binding
                        .is_some_and(|binding| renamed.contains(&binding))
                    && !*valid_math_name.get_or_insert_with(|| {
                        single_identifier(
                            &typst_syntax::parse_math(new_name),
                            typst_syntax::SyntaxKind::MathIdent,
                        )
                    })
                {
                    return Err(RenameError::InvalidMathIdentifier(new_name.into()));
                }
                if let OccurrenceKind::Argument(callee) = &occurrence.kind
                    && resolved.binding.is_some_and(|binding| renamed.contains(&binding))
                    && source.occurrences().iter().any(|other| {
                        matches!(&other.kind, OccurrenceKind::Argument(other_callee) if other_callee.at == callee.at)
                            && other.range != occurrence.range && source.text(&other.range) == new_name
                    }) {
                    return Err(RenameError::Conflict(new_name.into()));
                }
                if !matches!(occurrence.kind, OccurrenceKind::Name) {
                    continue;
                }
                let name = if resolved
                    .binding
                    .is_some_and(|binding| renamed.contains(&binding))
                {
                    new_name
                } else {
                    source.text(&occurrence.range)
                };
                if self.resolve(
                    file,
                    name,
                    occurrence.scope,
                    occurrence.range.start,
                    renamed,
                    new_name,
                ) != resolved.binding
                {
                    return Err(RenameError::Conflict(new_name.into()));
                }
            }
        }
        Ok(())
    }

    fn explicit_alias(&self, id: BindingId) -> bool {
        let binding = &self.bindings[id];
        binding.declaration.is_some_and(|declaration| {
            matches!(
                self.sources[&binding.file].declarations()[declaration].kind,
                DeclarationKind::Import {
                    explicit_alias: true,
                    ..
                }
            )
        })
    }

    fn visibility(&self, id: BindingId) -> (usize, usize) {
        let binding = &self.bindings[id];
        let source = &self.sources[&binding.file];
        match binding.declaration {
            Some(declaration) => {
                let declaration = &source.declarations()[declaration];
                (declaration.scope, declaration.visible_from)
            }
            None => {
                let import = &source.imports()[binding.wildcard.unwrap()];
                (import.scope, import.range.end)
            }
        }
    }

    fn resolve(
        &self,
        file: FileId,
        name: &str,
        mut scope: usize,
        at: usize,
        renamed: &BTreeSet<BindingId>,
        new_name: &str,
    ) -> Option<BindingId> {
        let source = &self.sources[&file];
        loop {
            let candidates = self
                .lexical
                .get(&(file, scope))
                .and_then(|names| names.get(name));
            let unchanged = candidates
                .into_iter()
                .flatten()
                .copied()
                .filter(|id| !renamed.contains(id));
            let changed = renamed.iter().copied().filter(|&id| {
                name == new_name && self.bindings[id].file == file && self.visibility(id).0 == scope
            });
            let found = unchanged
                .chain(changed)
                .filter(|&id| {
                    self.visibility(id).1 <= at
                        || self.bindings[id].declaration.is_some_and(|declaration| {
                            source.declarations()[declaration]
                                .recursive_body
                                .as_ref()
                                .is_some_and(|body| body.contains(&at))
                        })
                })
                .max_by_key(|&id| (self.visibility(id).1, id));
            if found.is_some() {
                return found;
            }
            scope = source.scopes()[scope].parent?;
        }
    }

    fn refresh_exports(&mut self) {
        self.exports.clear();
        self.lexical.clear();
        for (id, binding) in self.bindings.iter().enumerate() {
            let scope = self.visibility(id).0;
            self.lexical
                .entry((binding.file, scope))
                .or_default()
                .entry(binding.name.clone())
                .or_default()
                .push(id);
        }
        for &file in &self.files {
            let source = &self.sources[&file];
            let mut exports = BTreeMap::new();
            if let Some(names) = self.lexical.get(&(file, 0)) {
                for (name, bindings) in names {
                    if let Some(id) = bindings
                        .iter()
                        .copied()
                        .filter(|&id| self.visibility(id).1 <= source.source().text().len())
                        .max_by_key(|&id| (self.visibility(id).1, id))
                    {
                        exports.insert(name.clone(), id);
                    }
                }
            }
            self.exports.insert(file, exports);
        }
    }

    fn canonical(&self, id: BindingId) -> NameOrigin {
        self.origins[id]
    }

    fn imported_binding(&self, id: BindingId) -> Option<BindingId> {
        if let Some(binding) = self.imported.get(id) {
            return *binding;
        }
        let binding = &self.bindings[id];
        let source = &self.sources[&binding.file];
        if let Some(declaration) = binding.declaration {
            if let Initializer::Import { import, path } =
                &source.declarations()[declaration].initializer
            {
                return self
                    .import_path(binding.file, *import, path, &mut BTreeSet::from([id]))
                    .0;
            }
        } else if let Some(import) = binding.wildcard {
            return self
                .import_path(
                    binding.file,
                    import,
                    std::slice::from_ref(&binding.name),
                    &mut BTreeSet::from([id]),
                )
                .0;
        }
        None
    }

    fn value(&self, id: BindingId, seen: &mut BTreeSet<BindingId>) -> NameValue {
        if let Some(value) = self.values.get(id) {
            return value.clone();
        }
        if !seen.insert(id) {
            return NameValue::Unknown;
        }
        let binding = &self.bindings[id];
        let source = &self.sources[&binding.file];
        let value = if let Some(declaration) = binding.declaration {
            match &source.declarations()[declaration].initializer {
                Initializer::Function { body } => NameValue::Function {
                    file: binding.file,
                    body: body.start,
                },
                Initializer::Alias(path) => self.path_value(binding.file, path, seen),
                Initializer::Import { import, path } => {
                    self.import_path(binding.file, *import, path, seen).1
                }
                Initializer::Unknown => NameValue::Unknown,
            }
        } else {
            self.import_path(
                binding.file,
                binding.wildcard.unwrap(),
                std::slice::from_ref(&binding.name),
                seen,
            )
            .1
        };
        seen.remove(&id);
        value
    }

    /// Where a name resolves, as the compiler resolves it: a math spelling reads the math scope
    /// (`std` aside), a code spelling the standard library.
    fn builtin(&self, name: &str, math: bool) -> NameValue {
        if name == "std" {
            return self
                .standard
                .clone()
                .map_or(NameValue::Unknown, NameValue::Builtin);
        }
        let namespace = if math { &self.math } else { &self.global };
        let binding = namespace
            .as_ref()
            .and_then(|module| module.scope().get(name));
        binding.map_or(NameValue::Unknown, |binding| {
            NameValue::Builtin(binding.read().clone())
        })
    }

    fn path_value(
        &self,
        file: FileId,
        path: &NamePath,
        seen: &mut BTreeSet<BindingId>,
    ) -> NameValue {
        let source = &self.sources[&file];
        let Some(first) = path.segments.first() else {
            return NameValue::Unknown;
        };
        let name = source.text(first);
        let mut value = self
            .resolve(file, name, path.scope, path.at, &BTreeSet::new(), "")
            .map_or_else(
                || self.builtin(name, path.math),
                |binding| self.value(binding, seen),
            );
        for segment in &path.segments[1..] {
            value = self.field(value, source.text(segment), seen).1;
        }
        value
    }

    fn import_value(
        &self,
        file: FileId,
        import: usize,
        seen: &mut BTreeSet<BindingId>,
    ) -> NameValue {
        if let Some(&target) = self.targets.get(&(file, import)) {
            return NameValue::Module(target);
        }
        match &self.sources[&file].imports()[import].source {
            ImportSource::Name(path) => self.path_value(file, path, seen),
            _ => NameValue::Unknown,
        }
    }

    fn import_path(
        &self,
        file: FileId,
        import: usize,
        path: &[String],
        seen: &mut BTreeSet<BindingId>,
    ) -> (Option<BindingId>, NameValue) {
        let mut value = self.import_value(file, import, seen);
        let mut binding = None;
        for part in path {
            (binding, value) = self.field(value, part, seen);
        }
        (binding, value)
    }

    fn field(
        &self,
        value: NameValue,
        name: &str,
        seen: &mut BTreeSet<BindingId>,
    ) -> (Option<BindingId>, NameValue) {
        match value {
            NameValue::Module(file) => match self
                .exports
                .get(&file)
                .and_then(|exports| exports.get(name))
                .copied()
            {
                Some(binding) => (Some(binding), self.value(binding, seen)),
                None => (None, NameValue::Unknown),
            },
            NameValue::Builtin(value) => (
                None,
                value
                    .scope()
                    .and_then(|scope| scope.get(name))
                    .map_or(NameValue::Unknown, |binding| {
                        NameValue::Builtin(binding.read().clone())
                    }),
            ),
            _ => (None, NameValue::Unknown),
        }
    }
}

/// Whether `scope` encloses `at`, the path resolution walks outward from a spelling.
fn scope_reaches(source: &SourceNames, scope: usize, at: usize) -> bool {
    let mut current = source.scope_at(at);
    loop {
        if current == scope {
            return true;
        }
        match source.scopes()[current].parent {
            Some(parent) => current = parent,
            None => return false,
        }
    }
}

fn file_order(file: FileId) -> impl Ord {
    let path = file.get();
    let package = match path.root() {
        VirtualRoot::Project => None,
        VirtualRoot::Package(package) => Some((
            package.namespace.as_str(),
            package.name.as_str(),
            package.version,
        )),
    };
    (package, path.vpath().get_without_slash(), file.into_raw())
}

fn class(value: &NameValue) -> Option<NameClass> {
    Some(match value {
        NameValue::Function { .. } | NameValue::Builtin(Value::Func(_)) => NameClass::Function,
        // Typst supplies a math operator such as `sin` as packed content rather than a function,
        // and marks it `Mathy` to be laid out with the operands it takes. Spacing values the same
        // namespace supplies, such as `thin`, carry no such capability and stay plain values.
        NameValue::Builtin(Value::Content(content)) if content.can::<dyn Mathy>() => {
            NameClass::Function
        }
        NameValue::Module(_) | NameValue::Builtin(Value::Module(_)) => NameClass::Namespace,
        NameValue::Builtin(_) => NameClass::Value,
        NameValue::Unknown => return None,
    })
}

fn valid_identifier(name: &str) -> bool {
    typst_syntax::is_ident(name)
        && single_identifier(
            &typst_syntax::parse_code(name),
            typst_syntax::SyntaxKind::Ident,
        )
}

fn single_identifier(syntax: &typst_syntax::SyntaxNode, kind: typst_syntax::SyntaxKind) -> bool {
    let mut children = syntax.children();
    !syntax.diagnosis().errors
        && children.next().is_some_and(|node| node.kind() == kind)
        && children.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use typst_library::foundations::PathOrStr;
    use typst_syntax::{RootedPath, Source, VirtualPath};

    fn file(path: &str) -> FileId {
        RootedPath::new(VirtualRoot::Project, VirtualPath::new(path).unwrap()).intern()
    }

    fn graph(sources: &[(&str, &str)]) -> NameGraph {
        proven(sources, &[])
    }

    /// A name graph whose expression imports a host proved against the named target.
    ///
    /// Each `targets` entry names the statement's file, the bytes of its source expression, and
    /// the file that expression resolves to.
    fn proven(sources: &[(&str, &str)], targets: &[(&str, &str, &str)]) -> NameGraph {
        let texts: Vec<(FileId, &str)> = sources
            .iter()
            .map(|&(path, text)| (file(path), text))
            .collect();
        NameGraph::new(
            sources.iter().map(|(path, text)| {
                Arc::new(SourceNames::new(Source::new(file(path), (*text).into())))
            }),
            |query| match query {
                ImportQuery::Path { file, path } => PathOrStr::Str(path.into())
                    .resolve(file)
                    .ok()
                    .map(|path| path.intern()),
                ImportQuery::Expression {
                    file: holder,
                    range,
                } => {
                    let text = texts
                        .iter()
                        .find(|(id, _)| *id == holder)
                        .map(|(_, text)| *text)?;
                    targets
                        .iter()
                        .find(|&&(statement, expression, _)| {
                            file(statement) == holder && &text[range.clone()] == expression
                        })
                        .map(|&(_, _, target)| file(target))
                }
            },
            None,
            || Ok::<(), std::convert::Infallible>(()),
        )
        .unwrap()
    }

    fn binding_at(graph: &NameGraph, path: &str, marker: &str) -> BindingId {
        let file = file(path);
        let at = graph
            .source(file)
            .unwrap()
            .source()
            .text()
            .find(marker)
            .unwrap();
        graph.selected(file, at).unwrap().binding.unwrap()
    }

    fn renamed(graph: &NameGraph, binding: BindingId, name: &str) -> HashMap<FileId, String> {
        let mut sources: HashMap<_, _> = graph
            .sources()
            .map(|source| (source.source().id(), source.source().text().to_owned()))
            .collect();
        let mut edits = graph.rename(binding, name).unwrap();
        edits.sort_by_key(|edit| std::cmp::Reverse(edit.range.start));
        for edit in edits {
            sources
                .get_mut(&edit.file)
                .unwrap()
                .replace_range(edit.range, &edit.replacement);
        }
        sources
    }

    #[test]
    fn imported_rename_preserves_export() {
        let graph = graph(&[
            ("lib.typ", "#let greet() = [hello]"),
            ("main.typ", "#import \"lib.typ\": greet\n#greet()"),
        ]);
        let text = renamed(&graph, binding_at(&graph, "main.typ", "greet()"), "welcome");
        assert_eq!(
            text[&file("main.typ")],
            "#import \"lib.typ\": greet as welcome\n#welcome()"
        );
        assert_eq!(text[&file("lib.typ")], "#let greet() = [hello]");
    }

    #[test]
    fn export_rename_preserves_local_aliases() {
        let graph = graph(&[
            ("lib.typ", "#let greet() = [hello]"),
            ("alias.typ", "#import \"lib.typ\": greet as hello\n#hello()"),
            ("plain.typ", "#import \"lib.typ\": greet\n#greet()"),
            ("wild.typ", "#import \"lib.typ\": *\n#greet()"),
        ]);
        let text = renamed(&graph, binding_at(&graph, "lib.typ", "greet"), "welcome");
        assert_eq!(
            text[&file("alias.typ")],
            "#import \"lib.typ\": welcome as hello\n#hello()"
        );
        assert_eq!(
            text[&file("plain.typ")],
            "#import \"lib.typ\": welcome\n#welcome()"
        );
        assert_eq!(
            text[&file("wild.typ")],
            "#import \"lib.typ\": *\n#welcome()"
        );
    }

    #[test]
    fn field_reexports_follow_module_identity() {
        let graph = graph(&[
            ("lib.typ", "#let greet() = [hello]"),
            ("bridge.typ", "#import \"lib.typ\" as api\n#api.greet()"),
            (
                "main.typ",
                "#import \"bridge.typ\": api.greet as hello\n#hello()",
            ),
        ]);
        let text = renamed(&graph, binding_at(&graph, "lib.typ", "greet"), "welcome");
        assert_eq!(
            text[&file("main.typ")],
            "#import \"bridge.typ\": api.welcome as hello\n#hello()"
        );
        assert_eq!(
            text[&file("bridge.typ")],
            "#import \"lib.typ\" as api\n#api.welcome()"
        );
    }

    #[test]
    fn math_fields_are_not_lexical_spellings() {
        let graph = graph(&[("main.typ", "#let r = 10\n#r\n$arrow.r$")]);
        let binding = binding_at(&graph, "main.typ", "r = 10");
        let source = graph.source(file("main.typ")).unwrap().source().text();
        let references: Vec<_> = graph
            .references(binding, true)
            .into_iter()
            .filter(|(id, _)| *id == file("main.typ"))
            .map(|(_, occurrence)| occurrence.range.start)
            .collect();
        assert_eq!(
            references,
            [
                source.find("r = 10").unwrap(),
                source.find("#r").unwrap() + 1,
            ]
        );
        let text = renamed(&graph, binding, "width");
        assert_eq!(
            text[&file("main.typ")],
            "#let width = 10\n#width\n$arrow.r$"
        );
    }

    #[test]
    fn cyclic_wildcards_reach_declarations() {
        let graph = graph(&[
            ("a.typ", "#import \"b.typ\": *\n#let greet() = [hello]"),
            ("b.typ", "#import \"a.typ\": *"),
            ("main.typ", "#import \"b.typ\": *\n#greet()"),
        ]);
        let binding = binding_at(&graph, "main.typ", "greet");
        assert_eq!(graph.definition(binding).unwrap().0, file("a.typ"));
        let text = renamed(&graph, binding_at(&graph, "a.typ", "greet"), "welcome");
        assert_eq!(text[&file("main.typ")], "#import \"b.typ\": *\n#welcome()");
    }

    #[test]
    fn rename_refuses_conflicting_targets() {
        for (source, marker, name) in [
            ("#let old = 1\n#let taken = 2\n#old", "old", "taken"),
            ("#let old = 1\n#{ let taken = 2; old }", "old", "taken"),
            ("#let old = 1\n#taken\n#old", "old", "taken"),
            (
                "#let render(size: 2, ..rest) = size\n$render(size: 3, width: 4)$",
                "size",
                "width",
            ),
        ] {
            let graph = graph(&[("main.typ", source)]);
            assert!(
                matches!(
                    graph.rename(binding_at(&graph, "main.typ", marker), name),
                    Err(RenameError::Conflict(_))
                ),
                "{source}"
            );
        }
    }

    #[test]
    fn rename_rejects_non_identifiers() {
        let graph = graph(&[("main.typ", "#let old = 1\n#old")]);
        let binding = binding_at(&graph, "main.typ", "old");
        for name in ["", "let", "_", "two words", "x.y", "1name"] {
            assert!(
                matches!(
                    graph.rename(binding, name),
                    Err(RenameError::InvalidIdentifier(_))
                ),
                "{name}"
            );
        }
    }

    #[test]
    fn math_rename_rejects_operators() {
        let graph = graph(&[("main.typ", "#let amount = 1\n$amount$")]);
        let binding = binding_at(&graph, "main.typ", "amount");
        assert!(matches!(
            graph.rename(binding, "x-y"),
            Err(RenameError::InvalidMathIdentifier(_))
        ));
    }

    #[test]
    fn named_parameter_renames_call_labels() {
        for (source, expected) in [
            (
                "#let render(size: 2) = size\n#render(size: 3)",
                "#let render(width: 2) = width\n#render(width: 3)",
            ),
            // A multi-letter callee is a math call; a single letter stays math text beside a group.
            (
                "#let render(size: 2) = size\n$render(size: 3)$",
                "#let render(width: 2) = width\n$render(width: 3)$",
            ),
        ] {
            let graph = graph(&[("main.typ", source)]);
            let text = renamed(&graph, binding_at(&graph, "main.typ", "size"), "width");
            assert_eq!(text[&file("main.typ")], expected);
        }
    }

    #[test]
    fn shadowed_import_references_stay_local() {
        let graph = graph(&[
            ("lib.typ", "#let greet() = [hello]"),
            (
                "main.typ",
                "#import \"lib.typ\": greet\n#{ let greet = 1; greet }\n#greet()",
            ),
        ]);
        let references = graph.references(binding_at(&graph, "lib.typ", "greet"), false);
        let uses: Vec<_> = references
            .into_iter()
            .filter(|(id, _)| *id == file("main.typ"))
            .map(|(_, occurrence)| occurrence.range.start)
            .collect();
        assert_eq!(
            uses,
            [graph
                .source(file("main.typ"))
                .unwrap()
                .source()
                .text()
                .rfind("greet")
                .unwrap()]
        );
    }

    #[test]
    fn names_share_function_classification() {
        let graph = graph(&[
            ("lib.typ", "#let greet(x) = x"),
            (
                "main.typ",
                "#import \"lib.typ\": greet as welcome\n#let alias = welcome\n#alias",
            ),
        ]);
        let source = graph.source(file("main.typ")).unwrap();
        for needle in ["welcome", "alias"] {
            let at = source.source().text().rfind(needle).unwrap();
            assert_eq!(
                graph.class_at(source.source().id(), &(at..at + needle.len())),
                Some(NameClass::Function)
            );
        }
    }

    #[test]
    fn cyclic_imports_share_references() {
        let sources = [
            ("a.typ", "#import \"b.typ\": value\n#value"),
            ("b.typ", "#import \"a.typ\": value\n#value"),
        ];
        let graph = graph(&sources);
        let binding = binding_at(&graph, "a.typ", "value");
        let references: HashSet<_> = graph
            .references(binding, true)
            .into_iter()
            .map(|(file, occurrence)| (file, occurrence.range.start))
            .collect();
        let expected: HashSet<_> = sources
            .into_iter()
            .flat_map(|(path, text)| {
                [
                    (file(path), text.find("value").unwrap()),
                    (file(path), text.rfind("value").unwrap()),
                ]
            })
            .collect();
        assert_eq!(references, expected);
        assert!(graph.definition(binding).is_none());
    }

    #[test]
    fn unresolved_import_item_has_no_definition() {
        let graph = graph(&[(
            "main.typ",
            "#let base = \"lib\"\n#import base + \".typ\": greet\n#greet",
        )]);
        let binding = binding_at(&graph, "main.typ", "greet");
        assert!(graph.definition(binding).is_none());
    }

    #[test]
    fn unproven_imports_are_reported() {
        let sources = [
            (
                "main.typ",
                "#let base = \"lib\"\n#import base + \".typ\": greet\n#greet",
            ),
            ("lib.typ", "#let greet = [hello]"),
        ];
        let graph = proven(&sources, &[]);
        let main = file("main.typ");
        let statement = PendingImport {
            file: main,
            statement: 0,
        };
        let text = sources[0].1;
        assert_eq!(
            graph.unresolved_imports(main, text.find("greet").unwrap()),
            [statement],
            "the item"
        );
        assert_eq!(
            graph.unresolved_imports(main, text.rfind("greet").unwrap()),
            [statement],
            "the use"
        );
        assert_eq!(
            graph.unresolved_imports(file("lib.typ"), sources[1].1.find("greet").unwrap()),
            [statement],
            "the declaration the consumer may reach"
        );

        let named = [("main.typ", "#let api = [x]\n#import api: greet\n#greet")];
        let graph = proven(&named, &[]);
        assert_eq!(
            graph.unresolved_imports(main, named[0].1.rfind("greet").unwrap()),
            [PendingImport {
                file: main,
                statement: 0
            }]
        );
    }

    #[test]
    fn proven_dynamic_imports_share_one_identity() {
        for consumer in [
            "#let base = \"lib\"\n#import base + \".typ\": greet\n#greet",
            "#let base = \"lib\"\n#import base + \".typ\": *\n#greet",
        ] {
            let sources = [("main.typ", consumer), ("lib.typ", "#let greet = [hello]")];
            let main = file("main.typ");
            let lib = file("lib.typ");
            let use_at = consumer.rfind("greet").unwrap();
            assert_eq!(
                proven(&sources, &[]).unresolved_imports(main, use_at),
                [PendingImport {
                    file: main,
                    statement: 0
                }],
                "{consumer}"
            );
            let graph = proven(&sources, &[("main.typ", "base + \".typ\"", "lib.typ")]);
            assert!(
                graph.unresolved_imports(main, use_at).is_empty(),
                "{consumer}"
            );
            let use_binding = graph.selected(main, use_at).unwrap().binding.unwrap();
            assert_eq!(graph.definition(use_binding).unwrap().0, lib, "{consumer}");
            let declaration = binding_at(&graph, "lib.typ", "greet");
            let references = graph.references(declaration, true);
            let files: HashSet<_> = references.iter().map(|(file, _)| *file).collect();
            assert_eq!(files, HashSet::from([main, lib]), "{consumer}");
            assert!(
                references
                    .iter()
                    .any(|(file, occurrence)| *file == main && occurrence.range.start == use_at),
                "{consumer}"
            );
        }
    }

    /// A package's own dynamic wildcard imports the engine module Tola injects, never a site
    /// document, so a site declaration's references answer without the compiler lane.
    #[test]
    fn package_wildcards_cannot_carry_site_declarations() {
        let page = file("content/page.typ");
        let host = RootedPath::new(
            VirtualRoot::Package("@preview/tola-host:0.0.0".parse().unwrap()),
            VirtualPath::new("lib.typ").unwrap(),
        )
        .intern();
        let page_text = "#let marker = 1\n#marker\n";
        let graph = NameGraph::new(
            [
                Arc::new(SourceNames::new(Source::new(page, page_text.into()))),
                Arc::new(SourceNames::new(Source::new(
                    host,
                    "#import sys.inputs.at(\"__tola\"): *\n".into(),
                ))),
            ],
            |_| None,
            None,
            || Ok::<(), std::convert::Infallible>(()),
        )
        .unwrap();
        let at = page_text.rfind("marker").unwrap();
        assert!(graph.unresolved_imports(page, at).is_empty());
    }

    /// The same package statement leaves a site rename untouched: the spelling it leaves
    /// unresolved names the engine module's interface, never a site document's export.
    #[test]
    fn package_wildcards_do_not_block_site_renames() {
        let page = file("content/page.typ");
        let host = RootedPath::new(
            VirtualRoot::Package("@preview/tola-host:0.0.0".parse().unwrap()),
            VirtualPath::new("lib.typ").unwrap(),
        )
        .intern();
        let page_text = "#let marker = 1\n#marker\n";
        let graph = NameGraph::new(
            [
                Arc::new(SourceNames::new(Source::new(page, page_text.into()))),
                Arc::new(SourceNames::new(Source::new(
                    host,
                    "#import sys.inputs.at(\"__tola\"): *\n#marker\n".into(),
                ))),
            ],
            |_| None,
            None,
            || Ok::<(), std::convert::Infallible>(()),
        )
        .unwrap();
        let binding = graph
            .selected(page, page_text.find("marker").unwrap())
            .unwrap()
            .binding
            .unwrap();
        let renamed = graph.rename(binding, "renamed");
        assert!(renamed.is_ok(), "{renamed:?}");
    }

    /// A spelling that reaches a name through an unknown module interface sends its statement to
    /// the compiler lane, whether the cursor stands on the declaration or on the member a field
    /// read spells.
    #[test]
    fn module_members_report_their_unproven_statement() {
        let sources = [
            (
                "main.typ",
                "#let base = \"lib\"\n#import base as api\n#api.greet",
            ),
            ("lib.typ", "#let greet = [hello]"),
        ];
        let main = file("main.typ");
        let statement = PendingImport {
            file: main,
            statement: 0,
        };
        let graph = proven(&sources, &[]);
        assert_eq!(
            graph.unresolved_imports(main, sources[0].1.rfind("greet").unwrap()),
            [statement],
            "the member a field read spells"
        );
        assert_eq!(
            graph.unresolved_imports(file("lib.typ"), sources[1].1.find("greet").unwrap()),
            [statement],
            "the declaration the member reaches"
        );
        let proven = proven(&sources, &[("main.typ", "base", "lib.typ")]);
        assert!(
            proven
                .unresolved_imports(main, sources[0].1.rfind("greet").unwrap())
                .is_empty(),
            "a proved statement is settled"
        );
    }

    #[test]
    fn transitive_dynamic_imports_share_one_identity() {
        let sources = [
            (
                "main.typ",
                "#let base = \"mid\"\n#import base + \".typ\": greet\n#greet",
            ),
            (
                "mid.typ",
                "#let base = \"lib\"\n#import base + \".typ\": greet\n#greet",
            ),
            ("lib.typ", "#let greet = [hello]"),
        ];
        let main = file("main.typ");
        let mid = file("mid.typ");
        let use_at = sources[0].1.rfind("greet").unwrap();
        // The first proof reaches mid.typ; its own statement is what the identity still needs.
        assert_eq!(
            proven(&sources, &[("main.typ", "base + \".typ\"", "mid.typ")])
                .unresolved_imports(main, use_at),
            [PendingImport {
                file: mid,
                statement: 0
            }]
        );
        let graph = proven(
            &sources,
            &[
                ("main.typ", "base + \".typ\"", "mid.typ"),
                ("mid.typ", "base + \".typ\"", "lib.typ"),
            ],
        );
        assert_eq!(
            graph
                .definition(graph.selected(main, use_at).unwrap().binding.unwrap())
                .unwrap()
                .0,
            file("lib.typ")
        );
    }

    #[test]
    fn export_rename_refuses_unknown_dynamic_consumers() {
        let sources = [
            (
                "main.typ",
                "#let base = \"lib\"\n#import base + \".typ\": greet\n#greet",
            ),
            (
                "alias.typ",
                "#let base = \"lib\"\n#import base + \".typ\": greet as local\n#local",
            ),
            ("lib.typ", "#let greet = [hello]"),
        ];
        let graph = proven(&sources, &[]);
        let declaration = binding_at(&graph, "lib.typ", "greet");
        assert!(matches!(
            graph.rename(declaration, "welcome"),
            Err(RenameError::UnknownImport(file)) if file == "alias.typ"
        ));
    }

    /// A consumer that reaches a name through an unknown module interface: the statement binds the
    /// module itself, so only a field spells the export, and a rename must not leave it stale.
    #[test]
    fn export_rename_refuses_unknown_module_members() {
        let sources = [
            (
                "main.typ",
                "#let base = \"lib\"\n#import base as api\n#api.greet",
            ),
            ("lib.typ", "#let greet = [hello]"),
        ];
        let graph = proven(&sources, &[]);
        let declaration = binding_at(&graph, "lib.typ", "greet");
        assert!(matches!(
            graph.rename(declaration, "welcome"),
            Err(RenameError::UnknownImport(file)) if file == "main.typ"
        ));
    }

    /// An item path spells every segment in the unknown target's interface, so an intermediate
    /// segment a rename must edit refuses the rename just as the item's own name does.
    #[test]
    fn export_rename_refuses_unknown_nested_item_prefixes() {
        let sources = [
            (
                "consumer.typ",
                "#let base = \"holder\"\n#import base: mod.inner\n#inner",
            ),
            ("holder.typ", "#import \"sub.typ\" as mod"),
            ("sub.typ", "#let inner = 1"),
        ];
        let graph = proven(&sources, &[]);
        let declaration = binding_at(&graph, "holder.typ", "mod");
        assert!(matches!(
            graph.rename(declaration, "api"),
            Err(RenameError::UnknownImport(file)) if file == "consumer.typ"
        ));
    }

    /// A nested import through a module binding the graph established resolves without the host,
    /// so the rename is complete and must not be refused.
    #[test]
    fn nested_import_edits_item_segment() {
        let sources = [
            ("lib.typ", "#let greet = [hello]"),
            (
                "bridge.typ",
                "#import \"lib.typ\" as api\n#import api: greet as local\n#local",
            ),
        ];
        let graph = proven(&sources, &[]);
        let text = renamed(&graph, binding_at(&graph, "lib.typ", "greet"), "welcome");
        assert_eq!(
            text[&file("bridge.typ")],
            "#import \"lib.typ\" as api\n#import api: welcome as local\n#local"
        );
    }

    /// A module value an interface unknown to the graph carries into another file keeps its
    /// members under the rename contract: the chain that reads a member through the imported value
    /// either sends its statement to the compiler lane or refuses an incomplete rename.
    #[test]
    fn module_carrier_chain_refuses_rename() {
        let sources = [
            ("content/lib.typ", "#let greet = [hello]"),
            (
                "content/holder.typ",
                "#let base = \"lib.typ\"\n#import base as mod",
            ),
            (
                "content/chain.typ",
                "#import \"holder.typ\": mod\n#mod.greet",
            ),
        ];
        let graph = proven(&sources, &[]);
        let holder = file("content/holder.typ");
        let declaration = binding_at(&graph, "content/lib.typ", "greet");
        assert_eq!(
            graph.unresolved_imports(file("content/lib.typ"), sources[0].1.find("greet").unwrap()),
            [PendingImport {
                file: holder,
                statement: 0
            }],
            "the statement the chain's member read needs"
        );
        // The refusal names the statement whose interface is unknown, not the file that reads
        // through it: that statement is what the compiler lane can still establish.
        assert!(matches!(
            graph.rename(declaration, "welcome"),
            Err(RenameError::UnknownImport(file)) if file == "content/holder.typ"
        ));
    }

    /// The same chain with its statement proved edits the member a chained file reads, so a site
    /// rename never leaves that file behind.
    #[test]
    fn chained_module_member_follows_rename() {
        let sources = [
            ("content/lib.typ", "#let greet = [hello]"),
            (
                "content/holder.typ",
                "#let base = \"lib.typ\"\n#import base as mod",
            ),
            (
                "content/chain.typ",
                "#import \"holder.typ\": mod\n#mod.greet",
            ),
        ];
        let graph = proven(
            &sources,
            &[("content/holder.typ", "base", "content/lib.typ")],
        );
        let text = renamed(
            &graph,
            binding_at(&graph, "content/lib.typ", "greet"),
            "welcome",
        );
        assert_eq!(
            text[&file("content/chain.typ")],
            "#import \"holder.typ\": mod\n#mod.welcome"
        );
        assert_eq!(text[&file("content/holder.typ")], sources[1].1);
    }

    /// A local alias of the unknown module value holds the same interface, so a member read
    /// through it keeps the rename contract too.
    #[test]
    fn aliased_module_member_refuses_rename() {
        let sources = [
            ("lib.typ", "#let greet = [hello]"),
            (
                "main.typ",
                "#let base = \"lib\"\n#import base as mod\n#let api = mod\n#api.greet",
            ),
        ];
        let graph = proven(&sources, &[]);
        let declaration = binding_at(&graph, "lib.typ", "greet");
        assert!(matches!(
            graph.rename(declaration, "welcome"),
            Err(RenameError::UnknownImport(file)) if file == "main.typ"
        ));
    }

    #[test]
    fn export_rename_edits_proven_consumers() {
        for (consumer, expected) in [
            (
                "#let base = \"lib\"\n#import base + \".typ\": greet\n#greet",
                "#let base = \"lib\"\n#import base + \".typ\": welcome\n#welcome",
            ),
            (
                "#let base = \"lib\"\n#import base + \".typ\": *\n#greet",
                "#let base = \"lib\"\n#import base + \".typ\": *\n#welcome",
            ),
        ] {
            let graph = proven(
                &[("main.typ", consumer), ("lib.typ", "#let greet = [hello]")],
                &[("main.typ", "base + \".typ\"", "lib.typ")],
            );
            let text = renamed(&graph, binding_at(&graph, "lib.typ", "greet"), "welcome");
            assert_eq!(text[&file("main.typ")], expected);
            assert_eq!(text[&file("lib.typ")], "#let welcome = [hello]");
        }
    }

    #[test]
    fn local_alias_rename_keeps_the_external_name() {
        for (consumer, marker, expected) in [
            (
                "#let base = \"lib\"\n#import base + \".typ\": greet\n#greet",
                "greet",
                "#let base = \"lib\"\n#import base + \".typ\": greet as welcome\n#welcome",
            ),
            (
                "#let base = \"lib\"\n#import base + \".typ\": greet as local\n#local",
                "local",
                "#let base = \"lib\"\n#import base + \".typ\": greet as welcome\n#welcome",
            ),
        ] {
            let graph = proven(
                &[("main.typ", consumer), ("lib.typ", "#let greet = [hello]")],
                &[],
            );
            let text = renamed(&graph, binding_at(&graph, "main.typ", marker), "welcome");
            assert_eq!(text[&file("main.typ")], expected);
            assert_eq!(text[&file("lib.typ")], "#let greet = [hello]");
        }
    }

    #[test]
    fn package_definitions_remain_read_only() {
        let id = RootedPath::new(
            VirtualRoot::Package("@preview/demo:0.1.0".parse().unwrap()),
            VirtualPath::new("lib.typ").unwrap(),
        )
        .intern();
        let graph = NameGraph::new(
            [Arc::new(SourceNames::new(Source::new(
                id,
                "#let greet() = [hello]".into(),
            )))],
            |_| None,
            None,
            || Ok::<(), std::convert::Infallible>(()),
        )
        .unwrap();
        let at = graph
            .source(id)
            .unwrap()
            .source()
            .text()
            .find("greet")
            .unwrap();
        let binding = graph.selected(id, at).unwrap().binding.unwrap();
        assert!(matches!(
            graph.rename(binding, "welcome"),
            Err(RenameError::ReadOnly)
        ));
    }
}
