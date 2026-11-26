//! Rename edits for an established name graph.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use typst_syntax::{FileId, SyntaxKind, SyntaxNode, VirtualRoot};

use crate::names::{DeclarationKind, OccurrenceKind};

use super::{BindingId, NameEdit, NameGraph, RenameError};

type EditMap = HashMap<FileId, BTreeMap<(usize, usize), String>>;

impl NameGraph {
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
        let renamed = self.renamed_bindings(binding);
        self.reject_capture(&renamed, new_name)?;
        if let Some(file) = self.unfinished_consumers(&renamed) {
            return Err(RenameError::UnknownImport(
                file.vpath().get_without_slash().to_owned(),
            ));
        }

        let mut edits = EditMap::new();
        self.collect_occurrence_edits(&renamed, new_name, &mut edits);
        self.apply_declaration_edits(binding, &renamed, new_name, &mut edits);
        self.apply_wildcard_edit(binding, new_name, &mut edits);

        Ok(self
            .files
            .iter()
            .filter_map(|file| edits.remove(file).map(|file_edits| (*file, file_edits)))
            .flat_map(|(file, file_edits)| {
                file_edits
                    .into_iter()
                    .map(move |((start, end), replacement)| NameEdit {
                        file,
                        range: start..end,
                        replacement,
                    })
            })
            .collect())
    }

    fn collect_occurrence_edits(
        &self,
        renamed: &BTreeSet<BindingId>,
        new_name: &str,
        edits: &mut EditMap,
    ) {
        for &file in &self.files {
            if !matches!(file.root(), VirtualRoot::Project) {
                continue;
            }
            let src = &self.sources[&file];
            for (occ, res) in src.occurrences().iter().zip(self.occurrences(file)) {
                let changed = match &occ.kind {
                    OccurrenceKind::Import { import, path } => self
                        .import_path(file, *import, path, &mut BTreeSet::new())
                        .0
                        .is_some_and(|target| renamed.contains(&target)),
                    _ => res.binding.is_some_and(|target| renamed.contains(&target)),
                };
                if changed {
                    edits
                        .entry(file)
                        .or_default()
                        .insert((occ.range.start, occ.range.end), new_name.into());
                }
            }
        }
    }

    fn apply_declaration_edits(
        &self,
        binding: BindingId,
        renamed: &BTreeSet<BindingId>,
        new_name: &str,
        edits: &mut EditMap,
    ) {
        for &id in renamed {
            let candidate = &self.bindings[id];
            let src = self.source(candidate.file).unwrap();
            let Some(decl) = candidate.declaration else {
                continue;
            };
            let decl = &src.declarations()[decl];
            let replacement = match decl.kind {
                DeclarationKind::Import {
                    explicit_alias: false,
                    implicit: true,
                    ..
                } => {
                    edits
                        .entry(candidate.file)
                        .or_default()
                        .insert((decl.range.end, decl.range.end), format!(" as {new_name}"));
                    continue;
                }
                DeclarationKind::Import {
                    explicit_alias: false,
                    ..
                } if id == binding
                    && self.imported[id].is_none_or(|target| !renamed.contains(&target)) =>
                {
                    format!("{} as {new_name}", decl.name)
                }
                _ => new_name.into(),
            };
            edits
                .entry(candidate.file)
                .or_default()
                .insert((decl.range.start, decl.range.end), replacement);
        }
    }

    fn apply_wildcard_edit(&self, binding: BindingId, new_name: &str, edits: &mut EditMap) {
        let selected = &self.bindings[binding];
        let Some(import) = selected.wildcard else {
            return;
        };
        let src = self.source(selected.file).unwrap();
        let import = &src.imports()[import];
        let prefix = if src.source().text()[..import.range.start].ends_with('#') {
            "\n#import "
        } else {
            "; import "
        };
        let replacement = format!(
            "{prefix}{}: {} as {new_name}",
            src.text(&import.source_range),
            selected.name
        );
        edits
            .entry(selected.file)
            .or_default()
            .insert((import.range.end, import.range.end), replacement);
    }

    fn renamed_bindings(&self, binding: BindingId) -> BTreeSet<BindingId> {
        let mut renamed = BTreeSet::from([binding]);
        loop {
            let before = renamed.len();
            for (id, candidate) in self.bindings.iter().enumerate() {
                if !self.explicit_alias(id)
                    && self.imported[id].is_some_and(|target| renamed.contains(&target))
                    && matches!(candidate.file.root(), VirtualRoot::Project)
                {
                    renamed.insert(id);
                }
            }
            if before == renamed.len() {
                break;
            }
        }
        renamed
    }

    /// First site-file statement whose consumer spelling this rename would have to edit while
    /// its target remains unknown.
    fn unfinished_consumers(&self, renamed: &BTreeSet<BindingId>) -> Option<FileId> {
        for (file, stmt) in self.pending_statements() {
            if !matches!(file.root(), VirtualRoot::Project) {
                continue;
            }
            let src = &self.sources[&file];
            let mut spelled = self.unresolved_import_names(file, stmt);
            if src.imports()[stmt].wildcard && !self.interface_established(file, stmt) {
                spelled.extend(
                    src.occurrences()
                        .iter()
                        .zip(self.occurrences(file))
                        .filter(|(_, res)| res.binding.is_none() && res.class.is_none())
                        .map(|(occ, _)| src.text(&occ.range)),
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

    fn reject_capture(
        &self,
        renamed: &BTreeSet<BindingId>,
        new_name: &str,
    ) -> Result<(), RenameError> {
        for &id in renamed {
            let binding = &self.bindings[id];
            let (scope, _) = self.visibility(id);
            let names = &self.lexical[&(binding.file, scope)];
            if names
                .get(new_name)
                .is_some_and(|ids| ids.iter().any(|id| !renamed.contains(id)))
            {
                return Err(RenameError::Conflict(new_name.into()));
            }
        }
        let mut valid_math = None;
        for &file in &self.files {
            self.check_file_capture(file, renamed, new_name, &mut valid_math)?;
        }
        Ok(())
    }

    fn check_file_capture(
        &self,
        file: FileId,
        renamed: &BTreeSet<BindingId>,
        new_name: &str,
        valid_math: &mut Option<bool>,
    ) -> Result<(), RenameError> {
        let src = &self.sources[&file];
        for (occ, res) in src.occurrences().iter().zip(self.occurrences(file)) {
            let is_renamed = res.binding.is_some_and(|id| renamed.contains(&id));
            if occ.math
                && is_renamed
                && !*valid_math.get_or_insert_with(|| {
                    single_identifier(&typst_syntax::parse_math(new_name), SyntaxKind::MathIdent)
                })
            {
                return Err(RenameError::InvalidMathIdentifier(new_name.into()));
            }
            if let OccurrenceKind::Argument(callee) = &occ.kind
                && is_renamed
                && src.occurrences().iter().any(|other| {
                    matches!(&other.kind, OccurrenceKind::Argument(other_callee) if other_callee.at == callee.at)
                        && other.range != occ.range
                        && src.text(&other.range) == new_name
                })
            {
                return Err(RenameError::Conflict(new_name.into()));
            }
            if !matches!(occ.kind, OccurrenceKind::Name) {
                continue;
            }
            let name = if is_renamed {
                new_name
            } else {
                src.text(&occ.range)
            };
            if self.resolve(file, name, occ.scope, occ.range.start, renamed, new_name)
                != res.binding
            {
                return Err(RenameError::Conflict(new_name.into()));
            }
        }
        Ok(())
    }

    fn explicit_alias(&self, id: BindingId) -> bool {
        let binding = &self.bindings[id];
        binding.declaration.is_some_and(|decl| {
            matches!(
                self.sources[&binding.file].declarations()[decl].kind,
                DeclarationKind::Import {
                    explicit_alias: true,
                    ..
                }
            )
        })
    }
}

fn valid_identifier(name: &str) -> bool {
    typst_syntax::is_ident(name)
        && single_identifier(&typst_syntax::parse_code(name), SyntaxKind::Ident)
}

fn single_identifier(node: &SyntaxNode, kind: SyntaxKind) -> bool {
    let mut children = node.children();
    !node.diagnosis().errors
        && children.next().is_some_and(|child| child.kind() == kind)
        && children.next().is_none()
}
