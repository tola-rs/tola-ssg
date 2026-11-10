//! Rename edits for an established name graph.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use typst_syntax::{FileId, VirtualRoot};

use crate::names::{DeclarationKind, OccurrenceKind};

use super::{BindingId, NameEdit, NameGraph, RenameError};

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
                        && self.imported[id].is_none_or(|target| !renamed.contains(&target)) =>
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
            let mut spelled = self.unresolved_import_names(file, statement);
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
                .is_some_and(|bindings| bindings.iter().any(|id| !renamed.contains(id)))
            {
                return Err(RenameError::Conflict(new_name.into()));
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
