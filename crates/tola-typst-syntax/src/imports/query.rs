//! Export, definition, reference, and lexical queries on [`NameGraph`].

use std::collections::BTreeSet;
use std::ops::Range;

use typst_syntax::FileId;

use crate::names::Initializer;

use super::build::class;
use super::{
    BindingId, ModuleExport, NameClass, NameDefinition, NameGraph, NameOccurrence, NameOrigin,
    NameValue, PendingImport,
};

impl NameGraph {
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
        .unwrap_or_else(|err| match err {})
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
        let (_, val) = self.import_path(file, statement, path, &mut BTreeSet::new());
        Ok(self
            .exports_matching(val, prefix, &mut ensure_active)?
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
        val: NameValue,
        prefix: &str,
        ensure_active: &mut impl FnMut() -> Result<(), E>,
    ) -> Result<Option<Vec<ModuleExport>>, E> {
        ensure_active()?;
        let mut exports = Vec::new();
        match val {
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
            NameValue::Builtin(val) => {
                let Some(scope) = val.scope() else {
                    return Ok(None);
                };
                for (name, binding) in scope.iter() {
                    ensure_active()?;
                    if name.starts_with(prefix) {
                        let read = binding.read().clone();
                        exports.push(ModuleExport {
                            name: name.to_string(),
                            class: class(&NameValue::Builtin(read.clone())).unwrap(),
                            definition: Some(NameDefinition::Builtin(read)),
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
        let Some(decl) = binding.declaration else {
            return NameClass::Value;
        };
        let Some(src) = self.sources.get(&binding.file) else {
            return NameClass::Value;
        };
        match &src.declarations()[decl].initializer {
            Initializer::Function { .. } => NameClass::Function,
            Initializer::Import { path, .. } if path.is_empty() => NameClass::Namespace,
            Initializer::Alias(path) if path.segments.is_empty() => NameClass::Namespace,
            _ => NameClass::Value,
        }
    }

    /// Follows imports to a declaration; an import-only cycle has no invented definition.
    ///
    /// An item imported through an interface this graph never resolved has no definition here
    /// either: the statement spells an external export, and the compiler remains its authority.
    pub fn definition(&self, binding: BindingId) -> Option<(FileId, Range<usize>)> {
        let NameOrigin::Declaration(id) = self.canonical(binding) else {
            return None;
        };
        if self.unresolved_item_of(id).is_some() {
            return None;
        }
        let binding = &self.bindings[id];
        let src = self.source(binding.file)?;
        let range = match binding.declaration {
            Some(decl) => src.declarations()[decl].range.clone(),
            None => src.imports()[binding.wildcard?].range.clone(),
        };
        Some((binding.file, range))
    }

    /// The declaration or static library value one graph binding establishes.
    pub fn binding_definition(&self, binding: BindingId) -> Option<NameDefinition> {
        let declared = &self.bindings[binding];
        if let Some(decl) = declared.declaration {
            return Some(NameDefinition::Source {
                file: declared.file,
                declaration: decl,
            });
        }
        match &self.values[binding] {
            NameValue::Builtin(val) => Some(NameDefinition::Builtin(val.clone())),
            _ => {
                let (file, range) = self.definition(binding)?;
                let decl = self.sources[&file].declared_at(range.start)?;
                Some(NameDefinition::Source {
                    file,
                    declaration: decl,
                })
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
        let (binding, val) = self.import_path(file, statement, path, &mut BTreeSet::new());
        if let Some(binding) = binding {
            return self.binding_definition(binding);
        }
        match val {
            NameValue::Builtin(val) => Some(NameDefinition::Builtin(val)),
            _ => None,
        }
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
    ) -> Vec<(FileId, &NameOccurrence)> {
        let canonical = self.canonical(binding);
        self.files
            .iter()
            .flat_map(|&file| self.occurrences(file).iter().map(move |occ| (file, occ)))
            .filter(|(_, occ)| {
                (include_declaration || !occ.declaration)
                    && occ
                        .binding
                        .is_some_and(|id| self.canonical(id) == canonical)
            })
            .collect()
    }

    /// Whether the graph established every name one statement's interface binds.
    ///
    /// A module whose source this graph holds enumerates its exports, a native value enumerates
    /// its own scope, and a target the graph never read enumerates nothing.
    pub fn interface_established(&self, file: FileId, statement: usize) -> bool {
        match self.import_value(file, statement, &mut BTreeSet::new()) {
            NameValue::Module(target) => self.sources.contains_key(&target),
            NameValue::Builtin(val) => val.scope().is_some(),
            NameValue::Function { .. } | NameValue::Unknown => false,
        }
    }

    pub(super) fn visibility(&self, id: BindingId) -> (usize, usize) {
        let binding = &self.bindings[id];
        let src = &self.sources[&binding.file];
        match binding.declaration {
            Some(decl) => {
                let decl = &src.declarations()[decl];
                (decl.scope, decl.visible_from)
            }
            None => {
                let import = &src.imports()[binding.wildcard.unwrap()];
                (import.scope, import.range.end)
            }
        }
    }

    pub(super) fn resolve(
        &self,
        file: FileId,
        name: &str,
        mut scope: usize,
        at: usize,
        renamed: &BTreeSet<BindingId>,
        new_name: &str,
    ) -> Option<BindingId> {
        let src = &self.sources[&file];
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
                        || self.bindings[id].declaration.is_some_and(|decl| {
                            src.declarations()[decl]
                                .recursive_body
                                .as_ref()
                                .is_some_and(|body| body.contains(&at))
                        })
                })
                .max_by_key(|&id| (self.visibility(id).1, id));
            if found.is_some() {
                return found;
            }
            scope = src.scopes()[scope].parent?;
        }
    }

    pub(super) fn canonical(&self, id: BindingId) -> NameOrigin {
        self.origins[id]
    }

    pub(super) fn value(&self, id: BindingId, seen: &mut BTreeSet<BindingId>) -> NameValue {
        if let Some(val) = self.values.get(id) {
            return val.clone();
        }
        if !seen.insert(id) {
            return NameValue::Unknown;
        }
        let binding = &self.bindings[id];
        let src = &self.sources[&binding.file];
        let val = if let Some(decl) = binding.declaration {
            match &src.declarations()[decl].initializer {
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
        val
    }

    /// Where a name resolves: math spellings read `math` (`std` aside), code spellings read `global`.
    pub(super) fn builtin(&self, name: &str, math: bool) -> NameValue {
        if name == "std" {
            return self
                .standard
                .clone()
                .map_or(NameValue::Unknown, NameValue::Builtin);
        }
        let ns = if math { &self.math } else { &self.global };
        let binding = ns.as_ref().and_then(|module| module.scope().get(name));
        binding.map_or(NameValue::Unknown, |binding| {
            NameValue::Builtin(binding.read().clone())
        })
    }

    pub(super) fn path_value(
        &self,
        file: FileId,
        path: &crate::names::NamePath,
        seen: &mut BTreeSet<BindingId>,
    ) -> NameValue {
        let src = &self.sources[&file];
        let Some(first) = path.segments.first() else {
            return NameValue::Unknown;
        };
        let name = src.text(first);
        let mut val = self
            .resolve(file, name, path.scope, path.at, &BTreeSet::new(), "")
            .map_or_else(|| self.builtin(name, path.math), |id| self.value(id, seen));
        for seg in &path.segments[1..] {
            val = self.field(val, src.text(seg), seen).1;
        }
        val
    }

    pub(super) fn import_value(
        &self,
        file: FileId,
        import: usize,
        seen: &mut BTreeSet<BindingId>,
    ) -> NameValue {
        if let Some(&target) = self.targets.get(&(file, import)) {
            return NameValue::Module(target);
        }
        match &self.sources[&file].imports()[import].source {
            crate::names::ImportSource::Name(path) => self.path_value(file, path, seen),
            _ => NameValue::Unknown,
        }
    }

    pub(super) fn import_path(
        &self,
        file: FileId,
        import: usize,
        path: &[String],
        seen: &mut BTreeSet<BindingId>,
    ) -> (Option<BindingId>, NameValue) {
        let mut val = self.import_value(file, import, seen);
        let mut binding = None;
        for part in path {
            (binding, val) = self.field(val, part, seen);
        }
        (binding, val)
    }

    pub(super) fn field(
        &self,
        val: NameValue,
        name: &str,
        seen: &mut BTreeSet<BindingId>,
    ) -> (Option<BindingId>, NameValue) {
        match val {
            NameValue::Module(file) => match self
                .exports
                .get(&file)
                .and_then(|exports| exports.get(name))
                .copied()
            {
                Some(binding) => (Some(binding), self.value(binding, seen)),
                None => (None, NameValue::Unknown),
            },
            NameValue::Builtin(val) => (
                None,
                val.scope()
                    .and_then(|scope| scope.get(name))
                    .map_or(NameValue::Unknown, |binding| {
                        NameValue::Builtin(binding.read().clone())
                    }),
            ),
            _ => (None, NameValue::Unknown),
        }
    }
}
