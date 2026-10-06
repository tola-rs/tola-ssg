//! `typst-ide` queries over the same checked world used for site compilation.

use std::sync::OnceLock;

use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionTextEdit, InsertTextFormat, Range, TextEdit,
};
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

/// The checked world `typst-ide` answers over, plus the site inventories a
/// completion position may read.
///
/// `typst-ide` asks for site files and importable packages only in path and
/// package positions, so both are produced on first ask for one query: a
/// completion for a name, member, or argument performs no walk at all, and the
/// file list one request may ask for twice is walked once. Nothing here survives
/// the query, and both are held in the view because `IdeWorld` answers them by
/// value or by reference.
struct IdeView<'w, 'p> {
    world: &'w TypstWorld,
    config: Option<&'w ResolvedSiteConfig>,
    package_access: Option<&'p PackageAccess<'p>>,
    files: OnceLock<Vec<FileId>>,
    packages: OnceLock<Vec<(PackageSpec, Option<EcoString>)>>,
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
        match self.config {
            Some(config) => self.files.get_or_init(|| files::site_files(config)).clone(),
            None => Vec::new(),
        }
    }

    fn packages(&self) -> &[(PackageSpec, Option<EcoString>)] {
        let Some(package_access) = self.package_access else {
            return &[];
        };
        self.packages
            .get_or_init(|| importable_packages(package_access))
    }
}

pub(super) fn completion(
    world: &TypstWorld,
    config: &ResolvedSiteConfig,
    package_access: &PackageAccess<'_>,
    source: &Source,
    cursor: usize,
) -> Option<(Range, Vec<CompletionItem>)> {
    let view = IdeView {
        world,
        config: Some(config),
        package_access: Some(package_access),
        files: OnceLock::new(),
        packages: OnceLock::new(),
    };
    let (from, completions) =
        typst_ide::autocomplete(&view, None::<&dyn Output>, source, cursor, false)?;
    let range = position::utf16_range(source.lines(), from..cursor)?;
    Some((
        range,
        completions
            .into_iter()
            .map(|completion| protocol_completion(completion, &range))
            .collect(),
    ))
}

/// The tooltip Typst's own reader answers at one cursor, with its own section shapes left to the
/// caller.
pub(super) fn tooltip(world: &TypstWorld, source: &Source, cursor: usize) -> Option<Tooltip> {
    let view = IdeView {
        world,
        config: None,
        package_access: None,
        files: OnceLock::new(),
        packages: OnceLock::new(),
    };
    typst_ide::tooltip(&view, None::<&dyn Output>, source, cursor, Side::After)
}

pub(super) enum Definition {
    Span(Span),
    File(FileId),
}

pub(super) fn definition(world: &TypstWorld, source: &Source, cursor: usize) -> Option<Definition> {
    let view = IdeView {
        world,
        config: None,
        package_access: None,
        files: OnceLock::new(),
        packages: OnceLock::new(),
    };
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
    // A path label is the quoted string an author would type, while the completion itself has
    // the quotes the position needs.
    let label = match kind {
        CompletionKind::Path => label.trim_matches('"').to_owned(),
        _ => label.to_string(),
    };
    let text = apply.map_or_else(|| label.clone(), |apply| apply.to_string());
    let snippet = text.contains("${");
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
