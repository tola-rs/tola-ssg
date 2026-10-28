//! `typst-ide` queries over the same checked world used for site compilation.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use anyhow::Result;
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionTextEdit, InsertTextFormat, Range, TextEdit,
};
use tola_build::cancellation::BuildCancellation;
use tola_build::config::ResolvedSiteConfig;
use tola_typst::TypstWorld;
use tola_typst::typst::World;
use tola_typst::typst::diag::EcoString;
use tola_typst::typst::foundations::{Bytes, Datetime, Duration, Output};
use tola_typst::typst::syntax::package::PackageSpec;
use tola_typst::typst::syntax::{FileId, Side, Source, Span};
use tola_typst::typst::text::{Font, FontBook};
use tola_typst::typst::utils::LazyHash;
use tola_typst::typst::{Library, diag};
use typst_ide::{Completion, CompletionKind, Definition as UpstreamDefinition, IdeWorld, Tooltip};

use crate::files;
use crate::packages::{PackageAccess, importable_packages};
use crate::position;

/// The checked world `typst-ide` answers over, plus the site inventories a completion reads lazily.
struct IdeView<'w, 'p> {
    world: &'w TypstWorld,
    inventories: Option<IdeInventories<'w, 'p>>,
    overrides: &'w [(PathBuf, Arc<str>)],
    files: OnceLock<Result<Vec<FileId>>>,
    packages: OnceLock<Vec<(PackageSpec, Option<EcoString>)>>,
}

struct IdeInventories<'w, 'p> {
    config: &'w ResolvedSiteConfig,
    boundary: &'w tola_typst::SourceBoundary,
    cancellation: &'w BuildCancellation,
    package_access: &'p PackageAccess<'p>,
}

impl<'w> IdeView<'w, 'static> {
    fn without_inventories(world: &'w TypstWorld) -> Self {
        Self {
            world,
            inventories: None,
            overrides: &[],
            files: OnceLock::new(),
            packages: OnceLock::new(),
        }
    }
}

impl World for IdeView<'_, '_> {
    fn library(&self) -> &LazyHash<Library> {
        self.world.library()
    }

    fn book(&self) -> &LazyHash<FontBook> {
        self.world.book()
    }

    fn main(&self) -> FileId {
        self.world.main()
    }

    fn source(&self, id: FileId) -> diag::FileResult<Source> {
        self.world.source(id)
    }

    fn file(&self, id: FileId) -> diag::FileResult<Bytes> {
        self.world.file(id)
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.world.font(index)
    }

    fn today(&self, offset: Option<Duration>) -> Option<Datetime> {
        self.world.today(offset)
    }
}

impl IdeWorld for IdeView<'_, '_> {
    fn upcast(&self) -> &dyn World {
        self
    }

    fn files(&self) -> Vec<FileId> {
        let Some(inventories) = &self.inventories else {
            return Vec::new();
        };
        match self.files.get_or_init(|| {
            files::completable_file_ids(
                inventories.config,
                self.overrides,
                inventories.boundary,
                inventories.cancellation,
            )
        }) {
            Ok(files) => files.clone(),
            Err(_) => Vec::new(),
        }
    }

    fn packages(&self) -> &[(PackageSpec, Option<EcoString>)] {
        let Some(inventories) = &self.inventories else {
            return &[];
        };
        self.packages
            .get_or_init(|| importable_packages(inventories.package_access))
    }
}

pub(super) fn completion(
    world: &TypstWorld,
    config: &ResolvedSiteConfig,
    package_access: &PackageAccess<'_>,
    source: &Source,
    cursor: usize,
    cancellation: &BuildCancellation,
    overrides: &[(PathBuf, Arc<str>)],
) -> Result<Option<(Range, Vec<CompletionItem>)>> {
    cancellation.ensure_active()?;
    let view = IdeView {
        world,
        inventories: Some(IdeInventories {
            config,
            boundary: &package_access.boundary,
            cancellation,
            package_access,
        }),
        overrides,
        files: OnceLock::new(),
        packages: OnceLock::new(),
    };
    let completions = typst_ide::autocomplete(&view, None::<&dyn Output>, source, cursor, false);
    // `IdeWorld::files` cannot return failures; the request carries the scan's failure instead.
    if let Some(Err(error)) = view.files.into_inner() {
        return Err(error);
    }
    cancellation.ensure_active()?;
    let Some((from, completions)) = completions else {
        return Ok(None);
    };
    let Some(range) = position::utf16_range(source.lines(), from..cursor) else {
        return Ok(None);
    };
    let prefix = super::completion::typed_prefix(source, cursor);
    Ok(Some((
        range,
        completions
            .into_iter()
            .filter(|completion| matches_prefix(completion, prefix))
            .map(|completion| protocol_completion(completion, &range))
            .collect(),
    )))
}

/// Whether the reader's completion can still match what the author typed. A name has to start with
/// the typed prefix; a snippet, path, label, font, or package is matched on its own text.
fn matches_prefix(completion: &Completion, prefix: &str) -> bool {
    match completion.kind {
        CompletionKind::Func
        | CompletionKind::Type
        | CompletionKind::Param
        | CompletionKind::Constant
        | CompletionKind::Symbol(_) => super::completion::name_matches(&completion.label, prefix),
        _ => true,
    }
}

/// The tooltip Typst's own reader answers at one cursor, with its own section shapes left to the
/// caller.
pub(super) fn tooltip(world: &TypstWorld, source: &Source, cursor: usize) -> Option<Tooltip> {
    let view = IdeView::without_inventories(world);
    typst_ide::tooltip(&view, None::<&dyn Output>, source, cursor, Side::After)
}

pub(super) enum Definition {
    Span(Span),
    File(FileId),
}

pub(super) fn definition(world: &TypstWorld, source: &Source, cursor: usize) -> Option<Definition> {
    let view = IdeView::without_inventories(world);
    match typst_ide::definition(&view, None::<&dyn Output>, source, cursor, Side::After)? {
        UpstreamDefinition::Span(span) => Some(Definition::Span(span)),
        UpstreamDefinition::File(id) => Some(Definition::File(id)),
        // The standard library has no source the editor could open.
        UpstreamDefinition::Std(_) => None,
    }
}

fn protocol_completion(completion: Completion, range: &Range) -> CompletionItem {
    let Completion {
        kind,
        label,
        apply,
        detail,
    } = completion;
    let text = apply.map_or_else(|| label.to_string(), |apply| apply.to_string());
    // A path label is the quoted string an author would type, while the completion itself has
    // the quotes the position needs.
    let label = match kind {
        CompletionKind::Path => label.trim_matches('"').to_owned(),
        _ => label.to_string(),
    };
    let snippet = matches!(
        kind,
        CompletionKind::Syntax
            | CompletionKind::Func
            | CompletionKind::Type
            | CompletionKind::Param
    ) && text.contains("${");
    let text = if snippet {
        crate::completion::typst_snippet(&text)
    } else {
        text
    };
    CompletionItem {
        label,
        kind: Some(completion_kind(&kind)),
        detail: detail.map(|detail| detail.to_string()),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range: *range,
            new_text: text,
        })),
        insert_text_format: snippet.then_some(InsertTextFormat::SNIPPET),
        ..Default::default()
    }
}

fn completion_kind(kind: &CompletionKind) -> CompletionItemKind {
    match kind {
        CompletionKind::Syntax => CompletionItemKind::SNIPPET,
        CompletionKind::Func => CompletionItemKind::FUNCTION,
        CompletionKind::Type => CompletionItemKind::CLASS,
        CompletionKind::Param => CompletionItemKind::VARIABLE,
        CompletionKind::Constant => CompletionItemKind::CONSTANT,
        CompletionKind::Path => CompletionItemKind::FILE,
        CompletionKind::Package => CompletionItemKind::MODULE,
        CompletionKind::Label => CompletionItemKind::REFERENCE,
        CompletionKind::Font => CompletionItemKind::VALUE,
        CompletionKind::Symbol(_) => CompletionItemKind::CONSTANT,
    }
}
