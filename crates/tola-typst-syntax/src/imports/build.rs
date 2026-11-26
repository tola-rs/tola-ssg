//! Construction passes for [`NameGraph`].

use std::collections::{BTreeMap, BTreeSet};

use typst_library::foundations::Value;
use typst_library::math::Mathy;
use typst_syntax::{FileId, VirtualRoot};

use crate::names::{ImportSource, Initializer, Occurrence, OccurrenceKind, SourceNames};

use super::{
    Binding, BindingId, ImportQuery, NameClass, NameGraph, NameOccurrence, NameOrigin, NameValue,
};

impl NameGraph {
    pub(super) fn index_declarations<E>(
        &mut self,
        resolve: &impl Fn(ImportQuery<'_>) -> Option<FileId>,
        check: &impl Fn() -> Result<(), E>,
    ) -> Result<(), E> {
        for &file in &self.files {
            let src = &self.sources[&file];
            check()?;
            let mut locals = Vec::with_capacity(src.declarations().len());
            for (decl, sym) in src.declarations().iter().enumerate() {
                locals.push(self.bindings.len());
                self.bindings.push(Binding {
                    file,
                    declaration: Some(decl),
                    wildcard: None,
                    name: sym.name.clone(),
                });
            }
            self.locals.insert(file, locals);
            for (import, decl) in src.imports().iter().enumerate() {
                let query = match &decl.source {
                    ImportSource::Path(path) => ImportQuery::Path { file, path },
                    ImportSource::Name(_) | ImportSource::Unknown => ImportQuery::Expression {
                        file,
                        range: decl.source_range.clone(),
                    },
                };
                if let Some(target) = resolve(query) {
                    self.targets.insert((file, import), target);
                }
            }
        }
        Ok(())
    }

    pub(super) fn expand_wildcards<E>(
        &mut self,
        check: &impl Fn() -> Result<(), E>,
    ) -> Result<(), E> {
        // Fixed point over a finite set of exported spellings.
        loop {
            check()?;
            let mut additions = Vec::new();
            for &file in &self.files {
                let src = &self.sources[&file];
                check()?;
                for (import, _) in src
                    .imports()
                    .iter()
                    .enumerate()
                    .filter(|(_, import)| import.wildcard)
                {
                    self.collect_wildcard_exports(file, import, &mut additions);
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

    fn collect_wildcard_exports(
        &self,
        file: FileId,
        import: usize,
        additions: &mut Vec<(FileId, usize, String)>,
    ) {
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
            NameValue::Builtin(val) => {
                if let Some(scope) = val.scope() {
                    for (name, _) in scope.iter() {
                        add(name.as_str());
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn index_occurrences<E>(
        &mut self,
        check: &impl Fn() -> Result<(), E>,
    ) -> Result<(), E> {
        for &file in &self.files {
            let src = &self.sources[&file];
            check()?;
            let occs = src
                .occurrences()
                .iter()
                .map(|occ| {
                    check()?;
                    Ok(self.resolve_occurrence(file, src, occ))
                })
                .collect::<Result<_, E>>()?;
            self.occurrences.insert(file, occs);
        }
        Ok(())
    }

    fn resolve_occurrence(
        &self,
        file: FileId,
        src: &SourceNames,
        occ: &Occurrence,
    ) -> NameOccurrence {
        let decl = src.declaration_beginning_at(occ.range.start);
        let (binding, val) = match &occ.kind {
            OccurrenceKind::Declaration(id) => {
                let binding = self.locals[&file][*id];
                (Some(binding), self.value(binding, &mut BTreeSet::new()))
            }
            OccurrenceKind::Name => {
                let name = src.text(&occ.range);
                let binding =
                    self.resolve(file, name, occ.scope, occ.range.start, &BTreeSet::new(), "");
                let val = binding.map_or_else(
                    || self.builtin(name, occ.math),
                    |id| self.value(id, &mut BTreeSet::new()),
                );
                (binding, val)
            }
            OccurrenceKind::Field(path) => self.field(
                self.path_value(file, path, &mut BTreeSet::new()),
                src.text(&occ.range),
                &mut BTreeSet::new(),
            ),
            OccurrenceKind::Argument(callee) => {
                let binding = match self.path_value(file, callee, &mut BTreeSet::new()) {
                    NameValue::Function { file: owner, body } => self.sources[&owner]
                        .named_parameters()
                        .get(&body)
                        .and_then(|params| params.get(src.text(&occ.range)))
                        .map(|&id| self.locals[&owner][id]),
                    _ => None,
                };
                (binding, NameValue::Unknown)
            }
            OccurrenceKind::Import { import, path } => {
                let target = self.import_path(file, *import, path, &mut BTreeSet::new());
                match decl {
                    Some(local) => (Some(self.locals[&file][local]), target.1),
                    None => target,
                }
            }
        };
        NameOccurrence {
            range: occ.range.clone(),
            binding,
            class: class(&val),
            declaration: matches!(occ.kind, OccurrenceKind::Declaration(_)) || decl.is_some(),
        }
    }

    pub(super) fn resolve_origins<E>(
        &mut self,
        check: &impl Fn() -> Result<(), E>,
    ) -> Result<(), E> {
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

    pub(super) fn refresh_exports(&mut self) {
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
            let src = &self.sources[&file];
            let mut exports = BTreeMap::new();
            if let Some(names) = self.lexical.get(&(file, 0)) {
                for (name, ids) in names {
                    if let Some(id) = ids
                        .iter()
                        .copied()
                        .filter(|&id| self.visibility(id).1 <= src.source().text().len())
                        .max_by_key(|&id| (self.visibility(id).1, id))
                    {
                        exports.insert(name.clone(), id);
                    }
                }
            }
            self.exports.insert(file, exports);
        }
    }

    fn read_imported_binding(&self, id: BindingId) -> Option<BindingId> {
        let binding = &self.bindings[id];
        let src = &self.sources[&binding.file];
        if let Some(decl) = binding.declaration {
            if let Initializer::Import { import, path } = &src.declarations()[decl].initializer {
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
}

pub(super) fn file_order(file: FileId) -> impl Ord {
    let path = file.get();
    let package = match path.root() {
        VirtualRoot::Project => None,
        VirtualRoot::Package(pkg) => Some((pkg.namespace.as_str(), pkg.name.as_str(), pkg.version)),
    };
    (package, path.vpath().get_without_slash(), file.into_raw())
}

pub(super) fn class(val: &NameValue) -> Option<NameClass> {
    Some(match val {
        NameValue::Function { .. } | NameValue::Builtin(Value::Func(_)) => NameClass::Function,
        // Typst supplies a math operator such as `sin` as packed content marked `Mathy`;
        // plain spacing values such as `thin` lack `Mathy` and remain values.
        NameValue::Builtin(Value::Content(content)) if content.can::<dyn Mathy>() => {
            NameClass::Function
        }
        NameValue::Module(_) | NameValue::Builtin(Value::Module(_)) => NameClass::Namespace,
        NameValue::Builtin(_) => NameClass::Value,
        NameValue::Unknown => return None,
    })
}
