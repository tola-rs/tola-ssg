//! The bibliographies the site names: their files, their parses, and the entries they declare.

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use anyhow::Result;
use hayagriva::citationberg::LocaleCode;
use tola_build::cancellation::BuildCancellation;
use tola_build::check::SourceCompilation;
use tola_build::config::ResolvedSiteConfig;
use tola_typst::ContentDigest;
use tola_typst::TypstWorld;
use tola_typst::typst::World;
use tola_typst::typst::foundations::{NativeElement, Packed, PathOrStr, Str, StyleChain};
use tola_typst::typst::loading::DataSource;
use tola_typst::typst::model::{BibliographyElem, CslStyle};
use tola_typst::typst::syntax::{FileId, LinkedNode, Source, SyntaxKind, ast};
use tola_typst::typst::text::{Lang, Region};

use super::keys::yaml_keys;

/// One entry a bibliography file declares.
struct Entry {
    /// The byte range of the key itself, which is what a definition points at.
    range: Range<usize>,
    /// The entry itself, which a citation style renders.
    entry: Arc<hayagriva::Entry>,
    /// Who wrote it, when, and what it is called, which answers when no style can.
    described: Arc<str>,
}

/// The entries one bibliography file declares, parsed once while its bytes are unchanged.
struct ParsedBibliography {
    /// Every entry the file declares, by the key a citation writes.
    entries: BTreeMap<String, Entry>,
}

/// The site's bibliography files, as one lane last parsed them.
///
/// Every citation answer reads the files the site's bibliographies name, and parsing a large
/// bibliography again for each request costs more than the answer. A file is parsed again only when
/// its bytes changed, identified by the same content digest a compiler read records.
#[derive(Default)]
pub(crate) struct BibliographyCache {
    /// The parses the site's bibliography files produced, most recently read last.
    files: Vec<CachedBibliography>,
}

/// One file's parse, reused while its bytes are unchanged.
struct CachedBibliography {
    file: FileId,
    digest: ContentDigest,
    /// The entries the file declares, absent when no reader understood its text.
    parsed: Option<Arc<ParsedBibliography>>,
}

/// How many bibliography files one lane keeps parsed: more than a site usually names, and few
/// enough that a site naming a corpus of them holds only the newest.
const RETAINED_BIBLIOGRAPHIES: usize = 16;

impl BibliographyCache {
    /// The entries one site file declares, or `None` when its text could not be read.
    ///
    /// A refused text is remembered like an accepted one: the same bytes are refused the same way,
    /// and a citation answer must not parse a broken file anew on every request.
    fn read(&mut self, world: &TypstWorld, file: FileId) -> Option<Arc<ParsedBibliography>> {
        let bytes = world.file(file).ok()?;
        let digest = ContentDigest::of(bytes.as_slice());
        if let Some(index) = self
            .files
            .iter()
            .position(|cached| cached.file == file && cached.digest == digest)
        {
            let cached = self.files.remove(index);
            let parsed = cached.parsed.clone();
            self.files.push(cached);
            return parsed;
        }
        let parsed = ParsedBibliography::read(file, std::str::from_utf8(bytes.as_slice()).ok()?)
            .map(Arc::new);
        self.files.push(CachedBibliography {
            file,
            digest,
            parsed: parsed.clone(),
        });
        if self.files.len() > RETAINED_BIBLIOGRAPHIES {
            self.files.remove(0);
        }
        parsed
    }
}

impl ParsedBibliography {
    /// Read the entries one bibliography file declares.
    ///
    /// Nothing is read from a file whose format no bibliography uses, or whose text its reader
    /// refused: the site's own diagnostic points into the file that did not load.
    fn read(file: FileId, text: &str) -> Option<Self> {
        let format = match file.vpath().extension() {
            Some(extension) if extension.eq_ignore_ascii_case("bib") => Format::BibLaTeX,
            Some(extension)
                if extension.eq_ignore_ascii_case("yml")
                    || extension.eq_ignore_ascii_case("yaml") =>
            {
                Format::Yaml
            }
            _ => return None,
        };
        // Hayagriva reads the entries; each format's own reader says where a key sits, which the
        // reading drops and a definition needs.
        let (parsed, written) = match format {
            Format::BibLaTeX => (
                hayagriva::io::from_biblatex_str(text).ok(),
                biblatex::RawBibliography::parse(text)
                    .map(|written| {
                        written
                            .entries
                            .iter()
                            .map(|entry| (entry.v.key.v.to_owned(), entry.v.key.span.clone()))
                            .collect()
                    })
                    .unwrap_or_default(),
            ),
            Format::Yaml => (hayagriva::io::from_yaml_str(text).ok(), yaml_keys(text)),
        };
        let mut positions = BTreeMap::new();
        for (key, range) in written {
            positions.insert(key, range);
        }
        let mut entries = BTreeMap::new();
        // A bibliography reader refuses a duplicated key, so `parsed` declares each key once.
        for entry in parsed? {
            let Some(range) = positions.get(entry.key()) else {
                continue;
            };
            entries.insert(
                entry.key().to_owned(),
                Entry {
                    range: range.clone(),
                    described: Arc::from(describe(&entry)),
                    entry: Arc::new(entry),
                },
            );
        }
        Some(Self { entries })
    }
}

/// The style and language terms one `bibliography(…)` call renders its entries with.
#[derive(Clone)]
pub(super) struct StyleLocale {
    pub(super) style: CslStyle,
    pub(super) locale: LocaleCode,
}

/// One file the site's bibliographies read, and how the call that names it renders.
struct BibliographyFile {
    file: FileId,
    /// How the call that names the file renders its entries, absent for a call the site did not
    /// compile.
    rendering: Option<StyleLocale>,
    parsed: Arc<ParsedBibliography>,
}

impl BibliographyFile {
    /// One entry of this file as the call that names it declares it.
    fn declared<'a>(&'a self, entry: &'a Entry) -> Declared<'a> {
        Declared {
            file: self.file,
            range: entry.range.clone(),
            described: &entry.described,
            entry: &entry.entry,
            rendering: self.rendering.as_ref(),
        }
    }
}

/// One entry as a bibliography call of the site declares it.
pub(super) struct Declared<'a> {
    /// The file the entry is written in.
    pub(super) file: FileId,
    /// The byte range of the key itself, which is what a definition points at.
    pub(super) range: Range<usize>,
    /// Who wrote it, when, and what it is called, which answers when no style can.
    pub(super) described: &'a str,
    /// The entry itself, which a citation style renders.
    pub(super) entry: &'a hayagriva::Entry,
    /// How the call that declares it renders its entries.
    pub(super) rendering: Option<&'a StyleLocale>,
}

/// The entries every bibliography the site names declares, by the key a citation writes.
///
/// A key several calls declare answers with the first that declares it, in document order.
#[derive(Default)]
pub(super) struct Bibliography {
    /// The files the site's bibliographies read, in the order their calls name them.
    files: Vec<BibliographyFile>,
    /// Whether the site names any bibliography, which decides how a key no entry reaches answers: a
    /// site that names none leaves the reference to its labels.
    pub(super) named: bool,
    /// The named files whose text no reader understood, which a key no entry reaches reports.
    pub(super) unreadable: Vec<FileId>,
}

impl Bibliography {
    /// One entry by the key a citation writes.
    pub(super) fn entry(&self, key: &str) -> Option<Declared<'_>> {
        self.files
            .iter()
            .find_map(|file| Some(file.declared(file.parsed.entries.get(key)?)))
    }

    /// Every entry the site's bibliographies declare, in the order their files declare them.
    pub(super) fn entries(&self) -> impl Iterator<Item = Declared<'_>> {
        self.files.iter().flat_map(|file| {
            file.parsed
                .entries
                .values()
                .map(|entry| file.declared(entry))
        })
    }

    /// Read every bibliography the site names.
    ///
    /// `open` is the source the editor is answering about, which names a bibliography even before
    /// the author saves it.
    pub(super) fn read(
        compilation: &SourceCompilation,
        config: &ResolvedSiteConfig,
        open: FileId,
        cache: &mut BibliographyCache,
        cancellation: &BuildCancellation,
    ) -> Result<Self> {
        let world = compilation.world();
        let mut bibliography = Self::default();
        // A compiled site says which files each of its bibliographies reads.
        for realized in compiled_bibliographies(compilation) {
            cancellation.ensure_active()?;
            bibliography.named = true;
            for file in realized.files {
                cancellation.ensure_active()?;
                bibliography.add_file(world, file, Some(&realized.rendering), cache);
            }
        }
        // A program that did not compile realizes no more than the open document's own calls: the
        // rest of the site is searched for the calls its sources spell.
        if compilation.bundle().is_none() {
            bibliography.spelled(compilation, config, open, cache, cancellation)?;
        }
        Ok(bibliography)
    }

    /// Add the entries one file declares, read through `cache`.
    fn add_file(
        &mut self,
        world: &TypstWorld,
        file: FileId,
        rendering: Option<&StyleLocale>,
        cache: &mut BibliographyCache,
    ) {
        let Some(parsed) = cache.read(world, file) else {
            if !self.unreadable.contains(&file) {
                self.unreadable.push(file);
            }
            return;
        };
        // A file several calls name is read once, under the first call that names it.
        if self.files.iter().any(|named| named.file == file) {
            return;
        }
        self.files.push(BibliographyFile {
            file,
            rendering: rendering.cloned(),
            parsed,
        });
    }

    /// Add the entries of every `bibliography(…)` call the site's sources spell.
    fn spelled(
        &mut self,
        compilation: &SourceCompilation,
        config: &ResolvedSiteConfig,
        open: FileId,
        cache: &mut BibliographyCache,
        cancellation: &BuildCancellation,
    ) -> Result<()> {
        let world = compilation.world();
        for id in spelled_sources(compilation, config, open, cancellation)? {
            cancellation.ensure_active()?;
            let Ok(source) = world.source(id) else {
                continue;
            };
            let (named, paths) = bibliography_paths(&source);
            self.named |= named;
            for path in paths {
                cancellation.ensure_active()?;
                let Some(file) = resolve(id, &path) else {
                    continue;
                };
                self.add_file(world, file, None, cache);
            }
        }
        Ok(())
    }
}

/// The sources a citation key's spellings are read from.
///
/// A compiled site says which sources its documents hold, and those are the ones that can spell a
/// citation; a program that did not compile leaves the site's own files, searched on disk, and the
/// source the editor is answering about.
pub(super) fn spelled_sources(
    compilation: &SourceCompilation,
    config: &ResolvedSiteConfig,
    open: FileId,
    cancellation: &BuildCancellation,
) -> Result<Vec<FileId>> {
    let mut sources: Vec<FileId> = match compilation.bundle() {
        Some(_) => compilation
            .realized_documents(cancellation)?
            .into_iter()
            .flat_map(|document| document.sources)
            .collect(),
        None => crate::files::site_files(config),
    };
    sources.push(open);
    // Only a source is read as Typst, and a citation is spelled in a source.
    sources.retain(|id| id.vpath().extension() == Some("typ"));
    sources.sort_by_key(|id| id.vpath().get_without_slash().to_owned());
    sources.dedup();
    Ok(sources)
}

/// One bibliography the compiled site realizes: how it renders its entries and the files it reads.
struct CompiledBibliography {
    /// The style and language the call renders its entries with.
    rendering: StyleLocale,
    /// The site files the call names, in the order it names them.
    files: Vec<FileId>,
}

/// Every bibliography the compiled site realizes, in document order.
fn compiled_bibliographies(compilation: &SourceCompilation) -> Vec<CompiledBibliography> {
    let Some(introspector) = compilation.introspector() else {
        return Vec::new();
    };
    introspector
        .query(&BibliographyElem::ELEM.select())
        .iter()
        .filter_map(|element| {
            element
                .to_packed::<BibliographyElem>()
                .and_then(bibliography_of)
        })
        .collect()
}

/// The files one realized `bibliography(…)` call reads, and how it renders them.
fn bibliography_of(element: &Packed<BibliographyElem>) -> Option<CompiledBibliography> {
    let caller = element.span().id()?;
    let rendering = StyleLocale {
        style: element.style.get_cloned(StyleChain::default()).derived,
        locale: locale(
            element.lang.unwrap_or(Lang::ENGLISH),
            element.region.flatten(),
        ),
    };
    let files = element
        .sources
        .source
        .0
        .iter()
        .filter_map(|source| match source {
            DataSource::Path(path) => resolve(caller, path),
            // Raw bytes name no file the editor could point into.
            DataSource::Bytes(_) => None,
        })
        .collect();
    Some(CompiledBibliography { rendering, files })
}

/// The CSL locale code a call written in one language reads its terms in.
fn locale(lang: Lang, region: Option<Region>) -> LocaleCode {
    let mut code = String::with_capacity(5);
    code.push_str(lang.as_str());
    if let Some(region) = region {
        code.push('-');
        code.push_str(region.as_str());
    }
    LocaleCode(code)
}

/// The site file one bibliography source path names, resolved from the file that wrote it.
fn resolve(caller: FileId, path: &PathOrStr) -> Option<FileId> {
    Some(FileId::new(path.resolve(caller).ok()?))
}

/// The bibliography formats the site's sources name.
enum Format {
    /// A BibLaTeX `.bib` file.
    BibLaTeX,
    /// A Hayagriva `.yml` or `.yaml` file.
    Yaml,
}

/// The paths one source passes to `bibliography(…)`, and whether it makes such a call at all.
fn bibliography_paths(source: &Source) -> (bool, Vec<PathOrStr>) {
    let mut named = false;
    let mut paths = Vec::new();
    collect_paths(&LinkedNode::new(source.root()), &mut named, &mut paths);
    (named, paths)
}

fn collect_paths(node: &LinkedNode<'_>, named: &mut bool, paths: &mut Vec<PathOrStr>) {
    if node.kind() == SyntaxKind::FuncCall
        && let Some(call) = node.cast::<ast::FuncCall>()
        && let ast::Expr::Ident(name) = call.callee()
        && name.get().as_str() == "bibliography"
    {
        *named = true;
        for item in call.args().items() {
            match item {
                ast::Arg::Pos(expression) => collect_argument(&expression, paths),
                ast::Arg::Named(named) if named.name().get().as_str() == "path" => {
                    collect_argument(&named.expr(), paths);
                }
                _ => {}
            }
        }
    }
    for child in node.children() {
        collect_paths(&child, named, paths);
    }
}

/// The paths one `bibliography(…)` argument spells: a string, a `path(…)` call around one, or an
/// array of arguments. Anything else is computed too far away to read here.
fn collect_argument(expression: &ast::Expr, paths: &mut Vec<PathOrStr>) {
    match expression {
        ast::Expr::Str(text) => paths.push(PathOrStr::Str(Str::from(text.get()))),
        ast::Expr::Array(array) => {
            for item in array.items() {
                if let ast::ArrayItem::Pos(item) = item {
                    collect_argument(&item, paths);
                }
            }
        }
        ast::Expr::FuncCall(call) => {
            let ast::Expr::Ident(name) = call.callee() else {
                return;
            };
            if name.get().as_str() != "path" {
                return;
            }
            if let Some(ast::Arg::Pos(ast::Expr::Str(text))) = call.args().items().next() {
                paths.push(PathOrStr::Str(Str::from(text.get())));
            }
        }
        _ => {}
    }
}

/// What one entry's own fields say: who wrote it, when, and what it is called.
fn describe(entry: &hayagriva::Entry) -> String {
    let authors = entry
        .authors()
        .unwrap_or_default()
        .iter()
        .map(|person| person.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let mut described = authors;
    // A date the entry itself leaves out is what it was published in.
    if let Some(date) = entry.date_any() {
        if !described.is_empty() {
            described.push_str(", ");
        }
        described.push_str(&date.to_string());
    }
    if let Some(title) = entry.title() {
        if !described.is_empty() {
            described.push_str(" — ");
        }
        described.push_str(&title.to_string());
    }
    described
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `path:` argument names the file it reads exactly as a positional one does.
    #[test]
    fn named_path_argument_names_its_file() {
        let file = tola_typst::typst::syntax::RootedPath::new(
            tola_typst::typst::syntax::VirtualRoot::Project,
            tola_typst::typst::syntax::VirtualPath::new("/content/post.typ").unwrap(),
        )
        .intern();
        let source = Source::new(file, "#bibliography(path: \"refs.bib\")\n".to_owned());
        let (named, paths) = bibliography_paths(&source);
        assert!(named);
        let resolved: Vec<String> = paths
            .iter()
            .map(|path| {
                path.resolve(source.id())
                    .expect("the path resolves")
                    .vpath()
                    .get_with_slash()
                    .to_owned()
            })
            .collect();
        assert_eq!(resolved, ["/content/refs.bib"]);
    }

    #[test]
    fn yaml_definitions_cover_written_keys() {
        for (written, key, spelling) in [
            ("plain", "plain", "plain"),
            ("'single'", "single", "single"),
            ("\"double\"", "double", "double"),
            ("'isn''t'", "isn't", "isn''t"),
            (r#""escaped\u002dkey""#, "escaped-key", r"escaped\u002dkey"),
            (r#""quoted\"key""#, "quoted\"key", r#"quoted\"key"#),
            ("'中文'", "中文", "中文"),
        ] {
            let text = format!("{written}:\n  type: book\n  title: Example\n");
            let file = tola_typst::typst::syntax::RootedPath::new(
                tola_typst::typst::syntax::VirtualRoot::Project,
                tola_typst::typst::syntax::VirtualPath::new("/refs.yml").unwrap(),
            )
            .intern();
            let parsed = ParsedBibliography::read(file, &text).expect("the bibliography parses");
            let declared = parsed.entries.get(key).expect("the key is declared");
            assert_eq!(&text[declared.range.clone()], spelling, "{written}");
        }
    }
}
