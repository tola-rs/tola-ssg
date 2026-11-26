use std::collections::HashSet;

use anyhow::Result;
use lsp_types::{Location, Uri};
use tola_typst::typst::foundations::{Func, Str, Value};
use tola_typst::typst::syntax::{
    FileId, LinkedNode, RootedPath, Source, Span, SyntaxKind, VirtualRoot,
};
use tola_typst::typst::{World, WorldExt};
use tola_typst_syntax::names::{Declaration, DeclarationKind, SourceNames};

use super::Semantic;
use super::format::function_docs;
use crate::position;

pub(in crate::query) struct CandidateSource {
    pub file: FileId,
    pub site_path: String,
    pub content_path: Option<String>,
}

impl Semantic<'_> {
    pub(in crate::query) fn project_sources(&self) -> Vec<FileId> {
        let mut files = Vec::new();
        for read in self.comp.file_reads() {
            let path = match read.evidence().locator() {
                tola_typst::ReadLocator::Root(path)
                | tola_typst::ReadLocator::ProvidedRoot(path) => path,
                _ => continue,
            };
            if path.extension().is_none_or(|ext| ext != "typ") {
                continue;
            }
            let Ok(vpath) =
                tola_typst::typst::syntax::VirtualPath::new(path.to_string_lossy().as_ref())
            else {
                continue;
            };
            let id = RootedPath::new(VirtualRoot::Project, vpath).intern();
            if !files.contains(&id) {
                files.push(id);
            }
        }
        files
    }

    pub(in crate::query) fn world_source(&self, file: FileId) -> Option<Source> {
        self.comp.world().source(file).ok()
    }

    pub(in crate::query) fn candidate_sources(
        &mut self,
        node: &LinkedNode<'_>,
    ) -> Result<Vec<CandidateSource>> {
        let root = tola_typst_syntax::syntax::root_expression(node.clone());
        let mut srcs = Vec::new();
        for (val, _) in self.known_values(&root)? {
            srcs.extend(record_source(&val));
        }
        Ok(ordered(srcs))
    }

    pub(in crate::query) fn record_files(
        &mut self,
        node: &LinkedNode<'_>,
    ) -> Result<HashSet<FileId>> {
        let mut files = HashSet::new();
        for (val, _) in self.known_values(node)? {
            let Value::Array(records) = val else {
                continue;
            };
            for record in records.iter() {
                let Value::Dict(record) = record else {
                    continue;
                };
                let Ok(Value::Dyn(dyn_val)) = record.get("file") else {
                    continue;
                };
                let Some(file) = dyn_val.downcast::<RootedPath>() else {
                    continue;
                };
                if matches!(file.root(), VirtualRoot::Project) {
                    files.insert(file.clone().intern());
                }
            }
        }
        Ok(files)
    }

    pub(in crate::query) fn candidate_location(
        &mut self,
        candidate: &CandidateSource,
        client_root: &crate::uri::ClientRoot,
    ) -> Option<Location> {
        let uri = self.source_uri(candidate.file, client_root)?;
        let range = self
            .source_names(candidate.file)
            .and_then(|names| candidate_binding(&names))
            .and_then(|range| {
                let src = self.comp.world().source(candidate.file).ok()?;
                position::utf16_range(src.lines(), range)
            })
            .unwrap_or_default();
        Some(Location { uri, range })
    }

    pub(in crate::query) fn location(
        &self,
        span: Span,
        client_root: &crate::uri::ClientRoot,
    ) -> Option<Location> {
        let id = span.id()?;
        self.source_location(id, self.comp.world().range(span)?, client_root)
    }

    fn source_location(
        &self,
        file: FileId,
        bytes: std::ops::Range<usize>,
        client_root: &crate::uri::ClientRoot,
    ) -> Option<Location> {
        let src = self.comp.world().source(file).ok()?;
        Some(Location {
            uri: self.source_uri(file, client_root)?,
            range: position::utf16_range(src.lines(), bytes)?,
        })
    }

    pub(in crate::query) fn target_location(
        &self,
        def: super::super::editing::Definition,
        client_root: &crate::uri::ClientRoot,
    ) -> Option<Location> {
        match def {
            super::super::editing::Definition::Span(span) => self.location(span, client_root),
            super::super::editing::Definition::File(id) => {
                self.source_location(id, 0..0, client_root)
            }
        }
    }

    pub(in crate::query) fn source_uri(
        &self,
        id: FileId,
        client_root: &crate::uri::ClientRoot,
    ) -> Option<Uri> {
        if let VirtualRoot::Package(spec) = id.root()
            && crate::identity::embedded_source(id).is_none()
        {
            let world = self.comp.world();
            if world.local_packages().package(spec).is_some()
                || world.local_packages().owns_namespace(&spec.namespace)
            {
                let path = world.local_package_path(id).ok()??;
                return client_root.address(&path).ok();
            }
            return self
                .comp
                .file_reads()
                .iter()
                .chain(&self.accessed.reads)
                .find_map(|read| {
                    let matches = match read.evidence().locator() {
                        tola_typst::ReadLocator::Package { package, path }
                        | tola_typst::ReadLocator::ProvidedPackage { package, path } => {
                            package == spec
                                && path == std::path::Path::new(id.vpath().get_without_slash())
                        }
                        _ => false,
                    };
                    if matches && let tola_typst::ReadOrigin::Disk(path) = read.origin() {
                        return crate::uri::from_file_path(path.as_path()).ok();
                    }
                    None
                });
        }
        crate::identity::client_uri(id, self.comp.world().root(), client_root).ok()
    }

    pub(in crate::query) fn value_docs(&self, span: Span, val: Option<&Value>) -> Option<String> {
        self.source_docs(span).or_else(|| match val {
            Some(Value::Func(func)) => self.source_docs(func.span()),
            _ => None,
        })
    }

    pub(super) fn function_documentation(&self, func: &Func, def: Option<Span>) -> Option<String> {
        function_docs(func)
            .map(str::to_owned)
            .or_else(|| def.and_then(|span| self.source_docs(span)))
            .or_else(|| self.source_docs(func.span()))
    }

    pub(in crate::query) fn source_docs(&self, span: Span) -> Option<String> {
        let src = self.comp.world().source(span.id()?).ok()?;
        let mut node = src.find(span)?;
        if let Some(docs) = tola_typst_syntax::docs::declaration_docs(&src, node.range().start) {
            return Some(docs);
        }
        if node.kind() == SyntaxKind::Params {
            node = node.parent()?.clone();
        }
        if node.kind() != SyntaxKind::Closure {
            return None;
        }
        while let Some(parent) = node.parent() {
            match parent.kind() {
                SyntaxKind::Parenthesized => node = parent.clone(),
                SyntaxKind::LetBinding => {
                    return tola_typst_syntax::docs::declaration_docs(&src, parent.range().start);
                }
                _ => break,
            }
        }
        None
    }
}

fn ordered(mut srcs: Vec<CandidateSource>) -> Vec<CandidateSource> {
    srcs.sort_by(|l, r| l.site_path.cmp(&r.site_path));
    srcs.dedup_by(|l, r| l.site_path == r.site_path);
    srcs
}

fn record_source(val: &Value) -> Option<CandidateSource> {
    let Value::Dict(record) = val else {
        return None;
    };
    let Ok(Value::Dyn(dyn_val)) = record.get(tola_build::SourceDescriptorField::File.name()) else {
        return None;
    };
    let file = dyn_val.downcast::<RootedPath>()?;
    if !matches!(file.root(), VirtualRoot::Project) {
        return None;
    }
    let content_path = record
        .get(tola_build::SourceDescriptorField::Path.name())
        .ok()
        .and_then(|path| path.clone().cast::<Str>().ok())
        .map(|path| path.to_string());
    Some(CandidateSource {
        file: file.clone().intern(),
        site_path: file.vpath().get_without_slash().to_owned(),
        content_path,
    })
}

fn candidate_binding(names: &SourceNames) -> Option<std::ops::Range<usize>> {
    last_top_level_declaration(names, |decl| {
        decl.name == "page" && matches!(decl.kind, DeclarationKind::Let)
    })
    .map(|decl| decl.range.clone())
}

pub(super) fn last_top_level_declaration(
    names: &SourceNames,
    matches: impl Fn(&Declaration) -> bool,
) -> Option<&Declaration> {
    names
        .scopes()
        .first()?
        .declarations
        .iter()
        .rev()
        .map(|&id| &names.declarations()[id])
        .find(|decl| matches(decl))
}
