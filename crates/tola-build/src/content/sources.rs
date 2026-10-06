//! Immutable source descriptors exposed to the site program.

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use typst::foundations::{Array, Dict, IntoValue, Value};
use typst::syntax::{RootedPath, VirtualPath, VirtualRoot};

use super::slug::route_segments;
use super::{ContentId, ContentSourceLayout, ContentUnit};
use crate::config::ResolvedSiteConfig;
use crate::filesystem::normalize_path;
use crate::metadata::SourceMetadata;

/// One descriptor per discovered content file, independent of its inclusion count.
/// A source may appear in several output documents, or in none. Native locations
/// belong to realized occurrences, not this source set.
#[derive(Debug, Clone)]
pub struct ContentSource {
    source: PathBuf,
    source_layout: ContentSourceLayout,
    input: Arc<SourceInput>,
}

/// Exact semantic input exposed through lexical `current-source()` reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceFileInput {
    id: ContentId,
    file: RootedPath,
    path: VirtualPath,
    layout: ContentSourceLayout,
}

/// Exact semantic input exposed through `all-sources()`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceInput {
    file: Arc<SourceFileInput>,
    declaration: Option<SourceMetadataDeclaration>,
}

/// One field of a source descriptor the site program reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceDescriptorField {
    /// The identity the source is looked up by: its complete path below `build.content-dir`.
    Id,
    /// The source file addressed from the site root, the input `include` and `read` accept.
    File,
    /// The source's complete path below `build.content-dir`, extension included.
    Path,
    /// The last component of the source's path.
    Filename,
    /// The identity segments the file layout gives the source's default route.
    RouteSegments,
    /// The source's declared metadata.
    Metadata,
}

/// The kind of value one source descriptor field has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceDescriptorFieldKind {
    /// A `str`.
    Str,
    /// A `path`: the input `include` and `read` accept.
    Path,
    /// An `array` of `str`.
    StrArray,
    /// The source's declared metadata: a `dictionary`, or `none` when it declares none.
    Metadata,
}

impl SourceDescriptorField {
    /// Every field of an `all-sources()` source descriptor, in declaration order.
    ///
    /// The one authority on the descriptor's shape: the engine builds each descriptor dictionary
    /// from this list.
    pub const ALL: &'static [Self] = &[
        Self::Id,
        Self::File,
        Self::Path,
        Self::Filename,
        Self::RouteSegments,
        Self::Metadata,
    ];

    /// The descriptor key, spelled exactly as the site program reads it.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::File => "file",
            Self::Path => "path",
            Self::Filename => "filename",
            Self::RouteSegments => "route-segments",
            Self::Metadata => "meta",
        }
    }

    /// The kind of value the key has.
    pub const fn kind(self) -> SourceDescriptorFieldKind {
        match self {
            Self::Id | Self::Path | Self::Filename => SourceDescriptorFieldKind::Str,
            Self::File => SourceDescriptorFieldKind::Path,
            Self::RouteSegments => SourceDescriptorFieldKind::StrArray,
            Self::Metadata => SourceDescriptorFieldKind::Metadata,
        }
    }

    /// Whether `current-source()` — the lexical call's own descriptor — has the field.
    pub const fn in_current_source(self) -> bool {
        !matches!(self, Self::Metadata)
    }
}

/// One source's native declaration and the exact byte range of its call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceMetadataDeclaration {
    pub(crate) metadata: SourceMetadata,
    pub(crate) range: Range<usize>,
}

/// Ordered `all-sources()` input shared by compilation and source-analysis reuse.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SourcesInput {
    ordered: Arc<[Arc<SourceInput>]>,
    // Every metadata round reuses one file inventory, so its positions stay valid.
    by_file: Arc<BTreeMap<String, usize>>,
}

/// Native source descriptors, their lexical file lookup, and their origins.
#[derive(Debug, Clone, Default)]
pub(crate) struct SourceRecords {
    ordered: Array,
    by_file: Dict,
    origins: Dict,
}

impl SourceRecords {
    pub(crate) fn semantic_digest(&self) -> u128 {
        typst::utils::hash128(&self.ordered)
    }

    pub(crate) fn into_parts(self) -> (Array, Dict, Dict) {
        (self.ordered, self.by_file, self.origins)
    }
}

impl ContentSource {
    pub(crate) fn id(&self) -> &ContentId {
        &self.input.file.id
    }

    pub fn source(&self) -> &Path {
        &self.source
    }

    pub(crate) fn metadata(&self) -> Option<&SourceMetadata> {
        self.input
            .declaration
            .as_ref()
            .map(|declaration| &declaration.metadata)
    }

    pub(crate) fn source_layout(&self) -> ContentSourceLayout {
        self.source_layout
    }
}

impl SourceFileInput {
    /// The lexical descriptor `current-source()` returns, in `SourceDescriptorField::ALL` order.
    fn to_typst_dict(&self) -> Dict {
        SourceDescriptorField::ALL
            .iter()
            .filter(|field| field.in_current_source())
            .map(|field| (field.name().into(), self.field_value(*field, None)))
            .collect()
    }

    /// The value one descriptor field has for this source. `declaration` is the source's
    /// metadata declaration when the caller builds an `all-sources()` record.
    fn field_value(
        &self,
        field: SourceDescriptorField,
        declaration: Option<&SourceMetadataDeclaration>,
    ) -> Value {
        match field {
            SourceDescriptorField::Id => self.id.to_string().into_value(),
            SourceDescriptorField::File => self.file.clone().into_value(),
            SourceDescriptorField::Path => self.path.get_without_slash().into_value(),
            SourceDescriptorField::Filename => self.filename().into_value(),
            SourceDescriptorField::RouteSegments => route_segments(&self.id, self.layout)
                .into_iter()
                .map(IntoValue::into_value)
                .collect::<Array>()
                .into_value(),
            SourceDescriptorField::Metadata => declaration
                .map(|declaration| declaration.metadata.to_typst_dict())
                .into_value(),
        }
    }

    /// The last component of the source path.
    fn filename(&self) -> &str {
        self.path
            .get_without_slash()
            .rsplit('/')
            .next()
            .expect("a content file has a filename")
    }
}

impl SourceInput {
    /// The `all-sources()` descriptor dictionary, in `SourceDescriptorField::ALL` order.
    fn to_typst_dict(&self) -> Dict {
        SourceDescriptorField::ALL
            .iter()
            .map(|field| {
                (
                    field.name().into(),
                    self.file.field_value(*field, self.declaration.as_ref()),
                )
            })
            .collect()
    }
}

/// The source file and declaration range supplied to native source diagnostics.
fn source_origin(source: &SourceInput) -> Dict {
    let mut origin = Dict::new();
    origin.insert("file".into(), source.file.file.clone().into_value());
    origin.insert(
        "range".into(),
        source
            .declaration
            .as_ref()
            .map(|declaration| {
                let range = &declaration.range;
                Array::from_iter([
                    (range.start as i64).into_value(),
                    (range.end as i64).into_value(),
                ])
                .into_value()
            })
            .into_value(),
    );
    origin
}

impl SourcesInput {
    fn from_sources(sources: &[ContentSource], by_file: Arc<BTreeMap<String, usize>>) -> Self {
        Self {
            ordered: sources
                .iter()
                .map(|source| Arc::clone(&source.input))
                .collect::<Vec<_>>()
                .into(),
            by_file,
        }
    }

    pub(crate) fn get(&self, file: &RootedPath) -> Option<&Arc<SourceFileInput>> {
        if !matches!(file.root(), VirtualRoot::Project) {
            return None;
        }
        let position = self.by_file.get(file.vpath().get_with_slash())?;
        self.ordered.get(*position).map(|source| &source.file)
    }

    pub(crate) fn to_source_records(&self) -> SourceRecords {
        let mut by_file = Dict::new();
        let mut origins = Dict::new();
        let ordered = self
            .ordered
            .iter()
            .map(|source| {
                let file = source.file.to_typst_dict();
                by_file.insert(
                    source.file.file.vpath().get_with_slash().into(),
                    file.into_value(),
                );
                origins.insert(
                    source.file.path.get_without_slash().into(),
                    source_origin(source).into_value(),
                );
                source.to_typst_dict().into_value()
            })
            .collect();
        SourceRecords {
            ordered,
            by_file,
            origins,
        }
    }
}

/// Complete, immutable source set exposed to one site-program compilation.
#[derive(Debug, Clone, Default)]
pub struct SourceSet {
    sources: Arc<[ContentSource]>,
    inputs: SourcesInput,
}

impl SourceSet {
    pub(crate) fn without_metadata(
        units: &[ContentUnit],
        config: &ResolvedSiteConfig,
    ) -> Result<Self> {
        let mut sources = Vec::with_capacity(units.len());
        let site_root = normalize_path(config.get_root());
        for unit in units {
            let source = unit.source.clone();
            let input = Arc::new(SourceInput {
                file: Arc::new(SourceFileInput {
                    id: unit.id.clone(),
                    path: VirtualPath::virtualize(&unit.root, &source).with_context(|| {
                        format!(
                            "`{}` is outside `build.content-dir` (`{}`)",
                            crate::filesystem::display_path(&source, &site_root),
                            crate::filesystem::display_path(&unit.root, &site_root),
                        )
                    })?,
                    file: RootedPath::new(
                        VirtualRoot::Project,
                        VirtualPath::virtualize(&site_root, &source).with_context(|| {
                            format!(
                                "`{}` is outside the site root",
                                crate::filesystem::display_path(&source, &site_root)
                            )
                        })?,
                    ),
                    layout: unit.layout,
                }),
                declaration: None,
            });
            sources.push(ContentSource {
                source_layout: unit.layout,
                source,
                input,
            });
        }

        let inputs = SourcesInput::from_sources(
            &sources,
            Arc::new(
                sources
                    .iter()
                    .enumerate()
                    .map(|(position, source)| {
                        (
                            source.input.file.file.vpath().get_with_slash().to_owned(),
                            position,
                        )
                    })
                    .collect(),
            ),
        );

        Ok(Self {
            sources: sources.into(),
            inputs,
        })
    }

    pub(crate) fn with_metadata(
        &self,
        declarations: &BTreeMap<PathBuf, Option<SourceMetadataDeclaration>>,
    ) -> Self {
        let declared_input = |source: &ContentSource| SourceInput {
            declaration: declarations.get(source.source()).cloned().flatten(),
            ..(*source.input).clone()
        };
        if self
            .sources
            .iter()
            .all(|source| *source.input == declared_input(source))
        {
            return self.clone();
        }
        let sources = self
            .sources
            .iter()
            .map(|source| {
                let input = declared_input(source);
                if *source.input == input {
                    source.clone()
                } else {
                    ContentSource {
                        input: Arc::new(input),
                        ..source.clone()
                    }
                }
            })
            .collect::<Vec<_>>();
        let inputs = SourcesInput::from_sources(&sources, Arc::clone(&self.inputs.by_file));
        Self {
            sources: sources.into(),
            inputs,
        }
    }

    pub fn sources(&self) -> &[ContentSource] {
        &self.sources
    }

    pub(crate) fn inputs(&self) -> &SourcesInput {
        &self.inputs
    }
}

#[cfg(test)]
mod tests {
    fn content_units(root: &std::path::Path) -> anyhow::Result<Vec<crate::content::ContentUnit>> {
        super::super::discover_from_root(
            root,
            root,
            &root.join("site.typ"),
            &crate::cancellation::BuildCancellation::default(),
        )
    }

    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::content::ContentSourceLayout;
    use typst::foundations::{IntoValue, NativeElement};
    use typst::model::{EmphElem, StrongElem};
    use typst::text::TextElem;

    #[test]
    fn source_identity_keeps_metadata_apart() {
        let directory = TempDir::new().unwrap();
        let content = directory.path().join("content/posts/rust");
        fs::create_dir_all(&content).unwrap();
        let source = content.join("index.typ");
        fs::write(&source, "= Rust").unwrap();

        let config = crate::config::tests::load_test_config(directory.path(), "");
        let units = content_units(&directory.path().join("content")).unwrap();
        assert_eq!(units[0].layout, ContentSourceLayout::DirectoryIndex);
        assert_eq!(
            units[0].root,
            normalize_path(&directory.path().join("content"))
        );
        let scanned = BTreeMap::from([(
            units[0].source.clone(),
            Some(SourceMetadataDeclaration {
                metadata: SourceMetadata::from_dict(
                    [(
                        "title".into(),
                        typst::text::TextElem::packed("Rust").into_value(),
                    )]
                    .into_iter()
                    .collect(),
                ),
                range: 0..0,
            }),
        )]);

        let sources = SourceSet::without_metadata(&units, &config)
            .unwrap()
            .with_metadata(&scanned);
        let source = &sources.sources()[0];
        assert_eq!(source.id().as_path(), Path::new("posts/rust/index.typ"));
        assert_eq!(
            source.input.file.file.vpath().get_with_slash(),
            "/content/posts/rust/index.typ"
        );
        let dict = source.input.to_typst_dict();
        let path = dict
            .get("file")
            .unwrap()
            .clone()
            .cast::<RootedPath>()
            .unwrap();
        assert_eq!(path.root(), &VirtualRoot::Project);
        assert_eq!(
            path.vpath().get_with_slash(),
            "/content/posts/rust/index.typ"
        );
        let meta = dict.get("meta").unwrap().clone().cast::<Dict>().unwrap();
        assert_eq!(
            meta.get("title").unwrap(),
            &TextElem::packed("Rust").into_value()
        );
    }

    #[test]
    fn percent_paths_keep_source_spelling() {
        let directory = TempDir::new().unwrap();
        let content = directory.path().join("content");
        fs::create_dir_all(&content).unwrap();
        for filename in ["%41.typ", "100%.typ"] {
            fs::write(content.join(filename), "Document").unwrap();
        }
        let config = crate::config::tests::load_test_config(directory.path(), "");
        let units = content_units(&content).unwrap();
        let sources = SourceSet::without_metadata(&units, &config).unwrap();
        let files = sources
            .sources()
            .iter()
            .map(|source| {
                let fields = source.input.file.to_typst_dict();
                (
                    source.id().to_string(),
                    fields.get("route-segments").unwrap().clone(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            files,
            [
                (
                    "%41.typ".to_owned(),
                    Array::from_iter(["%41".into_value()]).into_value()
                ),
                (
                    "100%.typ".to_owned(),
                    Array::from_iter(["100%".into_value()]).into_value()
                ),
            ]
        );
        let file = sources.sources()[0]
            .input
            .file
            .to_typst_dict()
            .get("file")
            .unwrap()
            .clone()
            .cast::<RootedPath>()
            .unwrap();
        assert_eq!(file.vpath().get_with_slash(), "/content/%41.typ");
    }

    #[test]
    fn metadata_comparison_normalizes_values() {
        let directory = TempDir::new().unwrap();
        let content = directory.path().join("content");
        fs::create_dir_all(&content).unwrap();
        let source = content.join("post.typ");
        fs::write(&source, "Post").unwrap();

        let config = crate::config::tests::load_test_config(directory.path(), "");
        let units = content_units(&content).unwrap();
        let metadata = |content: typst::foundations::Content| {
            SourceMetadata::from_dict(
                [("summary".into(), content.into_value())]
                    .into_iter()
                    .collect(),
            )
        };
        let strong = BTreeMap::from([(
            units[0].source.clone(),
            Some(SourceMetadataDeclaration {
                metadata: metadata(StrongElem::new(TextElem::packed("important")).pack()),
                range: 0..0,
            }),
        )]);
        let emphasis = BTreeMap::from([(
            units[0].source.clone(),
            Some(SourceMetadataDeclaration {
                metadata: metadata(EmphElem::new(TextElem::packed("important")).pack()),
                range: 0..0,
            }),
        )]);

        let sources = SourceSet::without_metadata(&units, &config).unwrap();
        let first = sources.with_metadata(&strong);
        let same = sources.with_metadata(&strong);
        let changed = sources.with_metadata(&emphasis);

        assert_eq!(first.inputs(), same.inputs());
        assert_ne!(first.inputs(), changed.inputs());
    }

    /// The declared list is the shape a real source's descriptor dictionaries hold.
    #[test]
    fn descriptor_fields_match_engine_values() {
        let directory = TempDir::new().unwrap();
        let content = directory.path().join("content/posts");
        fs::create_dir_all(&content).unwrap();
        fs::write(content.join("deep.typ"), "Deep").unwrap();
        let config = crate::config::tests::load_test_config(directory.path(), "");
        let units = content_units(&directory.path().join("content")).unwrap();
        let declarations = BTreeMap::from([(
            units[0].source.clone(),
            Some(SourceMetadataDeclaration {
                metadata: SourceMetadata::from_dict(
                    [("title".into(), TextElem::packed("Deep").into_value())]
                        .into_iter()
                        .collect(),
                ),
                range: 0..0,
            }),
        )]);
        let sources = SourceSet::without_metadata(&units, &config)
            .unwrap()
            .with_metadata(&declarations);
        let (ordered, by_file, _) = sources.inputs().to_source_records().into_parts();

        fn type_name(kind: SourceDescriptorFieldKind) -> &'static str {
            match kind {
                SourceDescriptorFieldKind::Str => "str",
                SourceDescriptorFieldKind::Path => "path",
                SourceDescriptorFieldKind::StrArray => "array",
                SourceDescriptorFieldKind::Metadata => "dictionary",
            }
        }
        fn descriptor_fields(value: &Value) -> Vec<(&str, &str)> {
            match value {
                Value::Dict(dict) => dict
                    .iter()
                    .map(|(name, value)| (name.as_str(), value.ty().short_name()))
                    .collect(),
                other => panic!("a descriptor is a dictionary, not {other:?}"),
            }
        }

        let record = SourceDescriptorField::ALL
            .iter()
            .map(|field| (field.name(), type_name(field.kind())))
            .collect::<Vec<_>>();
        assert_eq!(descriptor_fields(&ordered.at(0, None).unwrap()), record);

        let lexical = SourceDescriptorField::ALL
            .iter()
            .filter(|field| field.in_current_source())
            .map(|field| (field.name(), type_name(field.kind())))
            .collect::<Vec<_>>();
        assert_eq!(
            descriptor_fields(by_file.get("/content/posts/deep.typ").unwrap()),
            lexical
        );
    }

    /// Every descriptor key reads as Tola's Typst surface spells it: lowercase words joined by `-`.
    #[test]
    fn descriptor_keys_use_hyphenated_spelling() {
        for field in SourceDescriptorField::ALL {
            let name = field.name();
            assert!(
                name.split('-').all(|word| {
                    !word.is_empty()
                        && word.chars().all(|character| {
                            character.is_ascii_lowercase() || character.is_ascii_digit()
                        })
                }),
                "`{name}` is not hyphenated"
            );
        }
    }
}
