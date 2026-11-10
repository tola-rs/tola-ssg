//! Import identities shared by navigation, rename, and name classification.

mod rename;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Range;
use std::sync::Arc;

use typst_library::Library;
use typst_library::foundations::{Module, Value};
use typst_library::math::Mathy;
use typst_syntax::{FileId, VirtualRoot};

use crate::names::{ImportSource, Initializer, NamePath, OccurrenceKind, SourceNames};

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

    fn index_declarations<E>(
        &mut self,
        resolve: &impl Fn(ImportQuery<'_>) -> Option<FileId>,
        check: &impl Fn() -> Result<(), E>,
    ) -> Result<(), E> {
        for &file in &self.files {
            let source = &self.sources[&file];
            check()?;
            let mut locals = Vec::with_capacity(source.declarations().len());
            for (declaration, name) in source.declarations().iter().enumerate() {
                locals.push(self.bindings.len());
                self.bindings.push(Binding {
                    file,
                    declaration: Some(declaration),
                    wildcard: None,
                    name: name.name.clone(),
                });
            }
            self.locals.insert(file, locals);
            for (import, declaration) in source.imports().iter().enumerate() {
                let query = match &declaration.source {
                    ImportSource::Path(path) => ImportQuery::Path { file, path },
                    ImportSource::Name(_) | ImportSource::Unknown => ImportQuery::Expression {
                        file,
                        range: declaration.source_range.clone(),
                    },
                };
                if let Some(target) = resolve(query) {
                    self.targets.insert((file, import), target);
                }
            }
        }
        Ok(())
    }

    fn expand_wildcards<E>(&mut self, check: &impl Fn() -> Result<(), E>) -> Result<(), E> {
        // A wildcard component reaches a fixed point over a finite set of exported spellings.
        loop {
            check()?;
            let mut additions = Vec::new();
            for &file in &self.files {
                let source = &self.sources[&file];
                check()?;
                for (import, _) in source
                    .imports()
                    .iter()
                    .enumerate()
                    .filter(|(_, import)| import.wildcard)
                {
                    let mut add = |name: &str| {
                        if !self
                            .wildcards
                            .get(&(file, import))
                            .is_some_and(|names| names.contains_key(name))
                        {
                            additions.push((file, import, name.to_owned()));
                        }
                    };
                    match self.import_value(file, import, &mut BTreeSet::new()) {
                        NameValue::Module(target) => {
                            if let Some(exports) = self.exports.get(&target) {
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
                let id = self.bindings.len();
                self.bindings.push(Binding {
                    file,
                    declaration: None,
                    wildcard: Some(import),
                    name: name.clone(),
                });
                self.wildcards
                    .entry((file, import))
                    .or_default()
                    .insert(name, id);
            }
            self.refresh_exports();
        }
        Ok(())
    }

    fn index_occurrences<E>(&mut self, check: &impl Fn() -> Result<(), E>) -> Result<(), E> {
        for &file in &self.files {
            let source = &self.sources[&file];
            check()?;
            let occurrences = source
                .occurrences()
                .iter()
                .map(|occurrence| {
                    check()?;
                    let declaration = source.declaration_beginning_at(occurrence.range.start);
                    let (binding, value) = match &occurrence.kind {
                        OccurrenceKind::Declaration(declaration) => {
                            let binding = self.locals[&file][*declaration];
                            (Some(binding), self.value(binding, &mut BTreeSet::new()))
                        }
                        OccurrenceKind::Name => {
                            let name = source.text(&occurrence.range);
                            let binding = self.resolve(
                                file,
                                name,
                                occurrence.scope,
                                occurrence.range.start,
                                &BTreeSet::new(),
                                "",
                            );
                            let value = binding.map_or_else(
                                || self.builtin(name, occurrence.math),
                                |binding| self.value(binding, &mut BTreeSet::new()),
                            );
                            (binding, value)
                        }
                        OccurrenceKind::Field(path) => self.field(
                            self.path_value(file, path, &mut BTreeSet::new()),
                            source.text(&occurrence.range),
                            &mut BTreeSet::new(),
                        ),
                        OccurrenceKind::Argument(callee) => {
                            let binding = match self.path_value(file, callee, &mut BTreeSet::new())
                            {
                                NameValue::Function { file: owner, body } => self.sources[&owner]
                                    .named_parameters()
                                    .get(&body)
                                    .and_then(|parameters| {
                                        parameters.get(source.text(&occurrence.range))
                                    })
                                    .map(|&declaration| self.locals[&owner][declaration]),
                                _ => None,
                            };
                            (binding, NameValue::Unknown)
                        }
                        OccurrenceKind::Import { import, path } => {
                            let target =
                                self.import_path(file, *import, path, &mut BTreeSet::new());
                            match declaration {
                                Some(local) => (Some(self.locals[&file][local]), target.1),
                                None => target,
                            }
                        }
                    };
                    Ok(ResolvedOccurrence {
                        range: occurrence.range.clone(),
                        binding,
                        class: class(&value),
                        declaration: matches!(occurrence.kind, OccurrenceKind::Declaration(_))
                            || declaration.is_some(),
                    })
                })
                .collect::<Result<_, E>>()?;
            self.occurrences.insert(file, occurrences);
        }
        Ok(())
    }

    fn resolve_origins<E>(&mut self, check: &impl Fn() -> Result<(), E>) -> Result<(), E> {
        self.imported = (0..self.bindings.len())
            .map(|id| {
                check()?;
                Ok(self.read_imported_binding(id))
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
    /// The exports one import statement's module supplies, when this graph read the module.
    ///
    /// A builtin module enumerates its own scope; a target this graph never read supplies nothing.
    pub fn module_exports(&self, file: FileId, statement: usize) -> Vec<(String, NameClass)> {
        self.module_exports_at(file, statement, &[])
    }

    /// The exports the given path selects inside one import statement's module.
    pub fn module_exports_at(
        &self,
        file: FileId,
        statement: usize,
        path: &[String],
    ) -> Vec<(String, NameClass)> {
        self.module_exports_matching(file, statement, path, "", || {
            Ok::<(), std::convert::Infallible>(())
        })
        .unwrap_or_else(|error| match error {})
        .into_iter()
        .map(|export| (export.name, export.class))
        .collect()
    }

    /// Exports matching a prefix, retaining identities already selected by the module interface.
    pub fn module_exports_matching<E>(
        &self,
        file: FileId,
        statement: usize,
        path: &[String],
        prefix: &str,
        mut ensure_active: impl FnMut() -> Result<(), E>,
    ) -> Result<Vec<ModuleExport>, E> {
        ensure_active()?;
        let (_, value) = self.import_path(file, statement, path, &mut BTreeSet::new());
        Ok(self
            .exports_matching(value, prefix, &mut ensure_active)?
            .unwrap_or_default())
    }

    /// Source module members held by a binding; non-module and unresolved values have no interface.
    pub fn binding_exports<E>(
        &self,
        binding: BindingId,
        prefix: &str,
        mut ensure_active: impl FnMut() -> Result<(), E>,
    ) -> Result<Option<Vec<ModuleExport>>, E> {
        ensure_active()?;
        self.exports_matching(self.values[binding].clone(), prefix, &mut ensure_active)
    }

    fn exports_matching<E>(
        &self,
        value: NameValue,
        prefix: &str,
        ensure_active: &mut impl FnMut() -> Result<(), E>,
    ) -> Result<Option<Vec<ModuleExport>>, E> {
        ensure_active()?;
        let mut exports = Vec::new();
        match value {
            NameValue::Module(target) => {
                let Some(module) = self.exports.get(&target) else {
                    return Ok(None);
                };
                for (name, &binding) in module
                    .range(prefix.to_owned()..)
                    .take_while(|(name, _)| name.starts_with(prefix))
                {
                    ensure_active()?;
                    exports.push(ModuleExport {
                        name: name.clone(),
                        class: self.export_class(binding),
                        definition: self.binding_definition(binding),
                    });
                    ensure_active()?;
                }
            }
            NameValue::Builtin(value) => {
                let Some(scope) = value.scope() else {
                    return Ok(None);
                };
                for (name, binding) in scope.iter() {
                    ensure_active()?;
                    if name.starts_with(prefix) {
                        exports.push(ModuleExport {
                            name: name.to_string(),
                            class: class(&NameValue::Builtin(binding.read().clone())).unwrap(),
                            definition: Some(NameDefinition::Builtin(binding.read().clone())),
                        });
                    }
                }
            }
            NameValue::Function { .. } | NameValue::Unknown => return Ok(None),
        }
        ensure_active()?;
        Ok(Some(exports))
    }

    fn export_class(&self, id: BindingId) -> NameClass {
        let binding = &self.bindings[id];
        let Some(declaration) = binding.declaration else {
            return NameClass::Value;
        };
        let Some(source) = self.sources.get(&binding.file) else {
            return NameClass::Value;
        };
        match &source.declarations()[declaration].initializer {
            Initializer::Function { .. } => NameClass::Function,
            Initializer::Import { path, .. } if path.is_empty() => NameClass::Namespace,
            Initializer::Alias(path) if path.segments.is_empty() => NameClass::Namespace,
            _ => NameClass::Value,
        }
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

    /// The declaration or static library value one graph binding establishes.
    pub fn binding_definition(&self, binding: BindingId) -> Option<NameDefinition> {
        let declared = &self.bindings[binding];
        if let Some(declaration) = declared.declaration {
            return Some(NameDefinition::Source {
                file: declared.file,
                declaration,
            });
        }
        match &self.values[binding] {
            NameValue::Builtin(value) => Some(NameDefinition::Builtin(value.clone())),
            _ => {
                let (file, range) = self.definition(binding)?;
                let declaration = self.sources[&file].declared_at(range.start)?;
                Some(NameDefinition::Source { file, declaration })
            }
        }
    }

    /// The declaration selected by a member path inside an import's established interface.
    pub fn import_definition(
        &self,
        file: FileId,
        statement: usize,
        path: &[String],
    ) -> Option<NameDefinition> {
        let (binding, value) = self.import_path(file, statement, path, &mut BTreeSet::new());
        if let Some(binding) = binding {
            return self.binding_definition(binding);
        }
        match value {
            NameValue::Builtin(value) => Some(NameDefinition::Builtin(value)),
            _ => None,
        }
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
        if path.is_empty() || self.imported[id].is_some() {
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
            return self.unestablished_imports(file, &selected.range);
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
                .unresolved_import_names(file, statement)
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
    fn unestablished_imports(&self, file: FileId, range: &Range<usize>) -> Vec<PendingImport> {
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
                        && self.module_bindings(file, statement).contains(&root)
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

    /// The names one pending statement can carry from an interface this graph never established.
    ///
    /// An item the graph could not bind spells every segment of its path, because each one names a
    /// member of the unknown target; a module binding the statement introduces spells the members
    /// its field reads reach, in every file that holds the value. A spelling the graph resolved is
    /// absent: the edit set already addresses the name it resolves.
    fn unresolved_import_names(&self, file: FileId, statement: usize) -> BTreeSet<&str> {
        let source = &self.sources[&file];
        let mut carried = BTreeSet::new();
        for (declaration, spelled) in source.declarations().iter().enumerate() {
            let Initializer::Import { import, path } = &spelled.initializer else {
                continue;
            };
            if *import == statement
                && !path.is_empty()
                && self.imported[self.locals[&file][declaration]].is_none()
            {
                carried.extend(path.iter().map(String::as_str));
            }
        }
        if self.interface_established(file, statement) {
            return carried;
        }
        let module_bindings = self.module_bindings(file, statement);
        let mut holders: HashMap<FileId, BTreeSet<BindingId>> = HashMap::new();
        for binding in module_bindings {
            holders
                .entry(self.bindings[binding].file)
                .or_default()
                .insert(binding);
        }
        for (holder, holder_bindings) in holders {
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
                                .is_some_and(|root| holder_bindings.contains(&root)))
                        .then(|| source.text(&occurrence.range))
                    }),
            );
        }
        carried
    }

    /// The bindings that hold one statement's unknown module value, in every file: the module
    /// binding it introduces, and every binding that aliases or imports the same value.
    fn module_bindings(&self, file: FileId, statement: usize) -> BTreeSet<BindingId> {
        let mut module_bindings: BTreeSet<BindingId> = self.sources[&file]
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
        if module_bindings.is_empty() {
            return module_bindings;
        }
        // Every binding that takes the value of another: an item of some file's interface, or a
        // local alias of it. Following these edges reaches the files the value was passed to.
        let mut taken_by: Vec<(BindingId, BindingId)> = Vec::new();
        for (id, binding) in self.bindings.iter().enumerate() {
            if let Some(target) = self.imported[id] {
                taken_by.push((target, id));
            }
            let Some(declaration) = binding.declaration else {
                continue;
            };
            if let Initializer::Alias(alias) =
                &self.sources[&binding.file].declarations()[declaration].initializer
                && let Some(root) = self.root_binding(binding.file, alias)
            {
                taken_by.push((root, id));
            }
        }
        loop {
            let before = module_bindings.len();
            for &(target, taker) in &taken_by {
                if module_bindings.contains(&target) {
                    module_bindings.insert(taker);
                }
            }
            if module_bindings.len() == before {
                break;
            }
        }
        module_bindings
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
    pub fn interface_established(&self, file: FileId, statement: usize) -> bool {
        match self.import_value(file, statement, &mut BTreeSet::new()) {
            NameValue::Module(target) => self.sources.contains_key(&target),
            NameValue::Builtin(value) => value.scope().is_some(),
            NameValue::Function { .. } | NameValue::Unknown => false,
        }
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

    fn read_imported_binding(&self, id: BindingId) -> Option<BindingId> {
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
    fn export_enumeration_propagates_interruption() {
        let library: String = (0..32)
            .map(|index| format!("#let alpha{index} = 1\n"))
            .collect();
        let graph = graph(&[
            ("lib.typ", &library),
            ("main.typ", "#import \"lib.typ\" as library\n#library"),
        ]);
        let binding = binding_at(&graph, "main.typ", "library");
        for module in [false, true] {
            let mut checked = 0;
            let mut check = || {
                checked += 1;
                if checked >= 8 {
                    Err("interrupted")
                } else {
                    Ok(())
                }
            };
            let error = if module {
                graph
                    .module_exports_matching(file("main.typ"), 0, &[], "a", &mut check)
                    .err()
                    .expect("interrupted enumeration")
            } else {
                graph
                    .binding_exports(binding, "a", &mut check)
                    .err()
                    .expect("interrupted enumeration")
            };
            assert_eq!(error, "interrupted");
        }
    }

    #[test]
    fn module_exports_name_imported_declarations() {
        let graph = graph(&[
            ("lib.typ", "#let greet() = [hello]\n#let api = 1\n"),
            ("main.typ", "#import \"lib.typ\": greet\n#greet()"),
        ]);
        let exports = graph.module_exports(file("main.typ"), 0);
        assert!(exports.contains(&("greet".to_owned(), NameClass::Function)));
        assert!(exports.contains(&("api".to_owned(), NameClass::Value)));
    }

    #[test]
    fn rename_import_as_local_alias() {
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
    fn rename_checks_capture_and_collisions() {
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
    fn unresolved_dynamic_import() {
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
        let binding = binding_at(&graph, "main.typ", "greet");
        assert!(graph.definition(binding).is_none());
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
    fn dynamic_imports_after_evaluation() {
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
