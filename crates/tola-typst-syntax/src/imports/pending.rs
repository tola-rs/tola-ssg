//! Pending and unresolved import queries on [`NameGraph`].

use std::collections::{BTreeSet, HashMap};
use std::ops::Range;

use typst_syntax::{FileId, VirtualRoot};

use crate::names::{ImportSource, Initializer, NamePath, OccurrenceKind, SourceNames};

use super::{BindingId, NameGraph, NameOrigin, PendingImport};

impl NameGraph {
    /// `id`'s declaration is an item of a statement whose interface is still unknown.
    pub(super) fn unresolved_item_of(&self, id: BindingId) -> Option<PendingImport> {
        let binding = &self.bindings[id];
        let decl = binding.declaration?;
        let Initializer::Import { import, path } =
            &self.sources[&binding.file].declarations()[decl].initializer
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

    /// The statements whose official target the identity at `cursor` still needs, in file and
    /// source order.
    ///
    /// Empty when the graph decides the spelling on its own: a literal path and a statement a
    /// host already proved are settled, and a local rename of an unresolved item stays complete
    /// because the graph keeps the external spelling by inserting an explicit alias.
    pub fn unresolved_imports(&self, file: FileId, cursor: usize) -> Vec<PendingImport> {
        let Some(sel) = self.selected(file, cursor) else {
            return Vec::new();
        };
        let Some(binding) = sel.binding else {
            return self.unestablished_imports(file, &sel.range);
        };
        let NameOrigin::Declaration(id) = self.canonical(binding) else {
            return Vec::new();
        };
        let declared = &self.bindings[id];
        let mut pending = Vec::new();
        for (file, stmt) in self.pending_statements() {
            // A package statement resolves inside its own package or Tola's injected module.
            if matches!(file.root(), VirtualRoot::Package(_))
                && matches!(declared.file.root(), VirtualRoot::Project)
            {
                continue;
            }
            let src = &self.sources[&file];
            let own = self.unresolved_item_of(id)
                == Some(PendingImport {
                    file,
                    statement: stmt,
                });
            let carries = self
                .unresolved_import_names(file, stmt)
                .contains(declared.name.as_str());
            let wildcard = src.imports()[stmt].wildcard
                && self
                    .exports
                    .get(&declared.file)
                    .and_then(|names| names.get(&declared.name))
                    .is_some_and(|export| *export == id)
                && !self.interface_established(file, stmt);
            if own || carries || wildcard {
                pending.push(PendingImport {
                    file,
                    statement: stmt,
                });
            }
        }
        pending
    }

    fn unestablished_imports(&self, file: FileId, range: &Range<usize>) -> Vec<PendingImport> {
        let src = &self.sources[&file];
        let Some((written, res)) = src
            .occurrences()
            .iter()
            .zip(self.occurrences(file))
            .find(|(written, _)| written.range == *range)
        else {
            return Vec::new();
        };
        if res.binding.is_some() || res.class.is_some() {
            return Vec::new();
        }
        let member_of = match &written.kind {
            OccurrenceKind::Name => None,
            OccurrenceKind::Field(path) => self.root_binding(file, path),
            _ => return Vec::new(),
        };
        let module_stmts: BTreeSet<usize> = src
            .declarations()
            .iter()
            .filter_map(|decl| match &decl.initializer {
                Initializer::Import { import, path } if path.is_empty() => Some(*import),
                _ => None,
            })
            .collect();
        let mut pending = Vec::new();
        for (stmt, import) in src.imports().iter().enumerate() {
            if self.interface_established(file, stmt) {
                continue;
            }
            let reached = match member_of {
                None => {
                    import.wildcard
                        && import.range.end <= range.start
                        && scope_reaches(src, import.scope, range.start)
                }
                Some(root) => {
                    module_stmts.contains(&stmt) && self.module_bindings(file, stmt).contains(&root)
                }
            };
            if reached {
                pending.push(PendingImport {
                    file,
                    statement: stmt,
                });
            }
        }
        pending
    }

    /// Statements whose source is not a decoded path and has no resolved target.
    pub(super) fn pending_statements(&self) -> Vec<(FileId, usize)> {
        let mut pending = Vec::new();
        for &file in &self.files {
            let src = &self.sources[&file];
            for (stmt, import) in src.imports().iter().enumerate() {
                if !matches!(import.source, ImportSource::Path(_))
                    && !self.targets.contains_key(&(file, stmt))
                {
                    pending.push((file, stmt));
                }
            }
        }
        pending
    }

    /// Names one pending statement can carry from an unestablished interface.
    pub(super) fn unresolved_import_names(&self, file: FileId, statement: usize) -> BTreeSet<&str> {
        let src = &self.sources[&file];
        let mut carried = BTreeSet::new();
        for (decl, spelled) in src.declarations().iter().enumerate() {
            let Initializer::Import { import, path } = &spelled.initializer else {
                continue;
            };
            if *import == statement
                && !path.is_empty()
                && self.imported[self.locals[&file][decl]].is_none()
            {
                carried.extend(path.iter().map(String::as_str));
            }
        }
        if self.interface_established(file, statement) {
            return carried;
        }
        let mut holders: HashMap<FileId, BTreeSet<BindingId>> = HashMap::new();
        for binding in self.module_bindings(file, statement) {
            holders
                .entry(self.bindings[binding].file)
                .or_default()
                .insert(binding);
        }
        for (holder, holder_bindings) in holders {
            let src = &self.sources[&holder];
            carried.extend(
                src.occurrences()
                    .iter()
                    .zip(self.occurrences(holder))
                    .filter_map(|(occ, res)| {
                        let OccurrenceKind::Field(path) = &occ.kind else {
                            return None;
                        };
                        (res.binding.is_none()
                            && self
                                .root_binding(holder, path)
                                .is_some_and(|root| holder_bindings.contains(&root)))
                        .then(|| src.text(&occ.range))
                    }),
            );
        }
        carried
    }

    fn module_bindings(&self, file: FileId, statement: usize) -> BTreeSet<BindingId> {
        let mut bindings: BTreeSet<BindingId> = self.sources[&file]
            .declarations()
            .iter()
            .enumerate()
            .filter(|(_, decl)| {
                matches!(
                    &decl.initializer,
                    Initializer::Import { import, path } if *import == statement && path.is_empty()
                )
            })
            .map(|(decl, _)| self.locals[&file][decl])
            .collect();
        if bindings.is_empty() {
            return bindings;
        }
        let mut taken_by: Vec<(BindingId, BindingId)> = Vec::new();
        for (id, binding) in self.bindings.iter().enumerate() {
            if let Some(target) = self.imported[id] {
                taken_by.push((target, id));
            }
            let Some(decl) = binding.declaration else {
                continue;
            };
            if let Initializer::Alias(alias) =
                &self.sources[&binding.file].declarations()[decl].initializer
                && let Some(root) = self.root_binding(binding.file, alias)
            {
                taken_by.push((root, id));
            }
        }
        loop {
            let before = bindings.len();
            for &(target, taker) in &taken_by {
                if bindings.contains(&target) {
                    bindings.insert(taker);
                }
            }
            if bindings.len() == before {
                break;
            }
        }
        bindings
    }

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
}

fn scope_reaches(src: &SourceNames, scope: usize, at: usize) -> bool {
    let mut cur = src.scope_at(at);
    loop {
        if cur == scope {
            return true;
        }
        match src.scopes()[cur].parent {
            Some(parent) => cur = parent,
            None => return false,
        }
    }
}
