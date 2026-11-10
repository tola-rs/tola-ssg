//! The metadata shapes the site's program declares through its `parse-sources` calls, and their
//! documentation.

use std::collections::HashSet;

use anyhow::Result;
use tola_build::check::SourceCompilation;
use tola_build::config::ResolvedSiteConfig;
use tola_build::{SourceDescriptorField, SourceDescriptorFieldKind};
use tola_typst::typst::World;
use tola_typst::typst::foundations::Value;
use tola_typst::typst::syntax::ast::{self, AstNode};
use tola_typst::typst::syntax::{FileId, LinkedNode, Source, VirtualRoot};

use crate::sentence::joined;

use super::context::Selection;
use super::hover::SECTION_JOIN;
use super::imports::calls_package_member;
use super::records::origin::SourceExpression;
use super::schema;
use super::semantic::Semantic;

/// One descriptor field as an answer line: the key, and the kind of value it has.
pub(super) fn descriptor_line(field: SourceDescriptorField) -> String {
    format!("`{}: {}`", field.name(), descriptor_type(field.kind()))
}

/// The type a descriptor field's kind reads as, in Typst's own code spelling.
pub(super) fn descriptor_type(kind: SourceDescriptorFieldKind) -> &'static str {
    match kind {
        SourceDescriptorFieldKind::Str => "str",
        SourceDescriptorFieldKind::Path => "path",
        SourceDescriptorFieldKind::StrArray => "array",
        SourceDescriptorFieldKind::Metadata => "dictionary",
    }
}

/// The declaration one record's schema expression resolves, when the world observed exactly one.
pub(super) fn declared_schema(
    schema: &SourceExpression,
    semantics: &mut Semantic<'_>,
) -> Result<Option<schema::OutputDescription>> {
    let Some(node) = schema.node() else {
        return Ok(None);
    };
    declared_schema_at(&node, semantics)
}

/// The declaration one schema argument node resolves, when the world observed exactly one.
pub(super) fn declared_schema_at(
    schema: &LinkedNode<'_>,
    semantics: &mut Semantic<'_>,
) -> Result<Option<schema::OutputDescription>> {
    let observed = semantics.trace(schema.span())?;
    if !tola_typst::observation_is_complete(&observed) {
        return Ok(None);
    }
    let Some(((declaration, _), others)) = observed.split_first() else {
        return Ok(None);
    };
    if others.iter().any(|(value, _)| value != declaration) {
        return Ok(None);
    }
    Ok(semantics
        .inspect_schema(declaration)?
        .and_then(schema::OutputDescription::read))
}
/// One metadata shape the site program declares for its sources' `meta` field.
#[derive(Clone)]
pub(super) struct SiteSchema {
    /// The declared shape of one source's metadata.
    pub(super) shape: schema::OutputDescription,
    /// The schema expression, as the site program writes it.
    declared_as: String,
    /// The site program file that declares it, site-root relative.
    site_path: String,
    /// The content files the call's records were observed for; empty when none was observed.
    covered: HashSet<FileId>,
}

impl SiteSchema {
    /// How an answer names this declaration: the schema expression and the file writing it.
    pub(super) fn declaration(&self) -> String {
        format!("`{}` in `{}`", self.declared_as, self.site_path)
    }

    /// Whether this declaration declares `key`, whether required or optional.
    pub(super) fn declares(&self, key: &str) -> bool {
        self.shape.fields().iter().any(|(field, _)| field == key)
    }

    /// Whether this declaration accepts keys it does not declare.
    pub(super) fn accepts_unknown(&self) -> bool {
        self.shape.accepts_unknown()
    }

    /// Whether this declaration requires `key`, so a dictionary omitting it fails this call.
    pub(super) fn requires(&self, key: &str) -> bool {
        self.shape
            .at_path(&[key.to_owned()])
            .is_some_and(|field| field.is_required())
    }
}

/// The metadata shapes one source answers to, and whether the site declares more than one.
pub(super) struct SourceSchemas {
    /// The shapes this source's records are described by: the covering calls' shapes, or every
    /// declared shape when no call's records were observed to cover it.
    pub(super) shapes: Vec<SiteSchema>,
    /// Whether the site program declares more than one, so each answer names its declaration.
    pub(super) several: bool,
    /// Whether any call's records were observed to cover this source. Without one, the shapes are
    /// possible declarations rather than this source's own.
    pub(super) covered: bool,
}

/// One declared metadata key: the type its schema declares and, named by the declaration writing
/// each, the fields those declarations hold.
pub(super) struct DeclaredKey {
    pub(super) ty: Option<String>,
    pub(super) fields: Vec<(schema::DeclaredField, String)>,
}

/// The metadata schemas the site program's own `parse-sources` calls declare.
pub(super) fn site_schemas(
    compilation: &SourceCompilation,
    config: &ResolvedSiteConfig,
    semantics: &mut Semantic<'_>,
) -> Result<Vec<SiteSchema>> {
    let world = compilation.world();
    let Some(entry) = compilation.site_entry() else {
        return Ok(Vec::new());
    };
    let mut schemas = Vec::new();
    let mut visited = HashSet::new();
    let mut pending = vec![entry];
    while let Some(id) = pending.pop() {
        if !visited.insert(id) {
            continue;
        }
        let Ok(source) = world.source(id) else {
            continue;
        };
        let names = tola_typst_syntax::names::SourceNames::new(source.clone());
        let site_path = declaration_path(id, config);
        let mut nodes = vec![LinkedNode::new(source.root())];
        while let Some(node) = nodes.pop() {
            nodes.extend(node.children());
            let Some(call) = node.cast::<ast::FuncCall>() else {
                continue;
            };
            if let Some(schema) = site_schema(&node, &call, &names, semantics, &site_path)? {
                schemas.push(schema);
            }
        }
        pending.extend(program_imports(&source, &names, semantics, config, entry)?);
    }
    Ok(schemas)
}

/// How an answer spells a declaring file: site-root relative, or the package spec supplying it.
fn declaration_path(id: FileId, config: &ResolvedSiteConfig) -> String {
    match id.root() {
        VirtualRoot::Project => tola_build::filesystem::display_path(
            &config.get_root().join(id.vpath().get_without_slash()),
            config.get_root(),
        ),
        VirtualRoot::Package(package) => format!("{package}{}", id.vpath().get_with_slash()),
    }
}

/// The imports of one program file that may themselves declare the site's source schema.
///
/// Only a project file outside the content directory, or one of the driver's package, is followed.
fn program_imports(
    source: &Source,
    names: &tola_typst_syntax::names::SourceNames,
    semantics: &mut Semantic<'_>,
    config: &ResolvedSiteConfig,
    entry: FileId,
) -> Result<Vec<FileId>> {
    let mut files = Vec::new();
    for import in names.imports() {
        let Some(source_node) =
            tola_typst_syntax::syntax::node_at_range(source, &import.source_range)
        else {
            continue;
        };
        let Some(Value::Module(module)) = semantics.import(&source_node)? else {
            continue;
        };
        let Some(file) = module.file_id() else {
            continue;
        };
        if (file.root() == &VirtualRoot::Project
            && !config
                .get_root()
                .join(file.vpath().get_without_slash())
                .starts_with(&config.build().content_dir))
            || (matches!(entry.root(), VirtualRoot::Package(_)) && file.root() == entry.root())
        {
            files.push(file);
        }
    }
    Ok(files)
}

/// The metadata shape one `parse-sources` call declares, when the world observed its schema
/// argument and exactly one declaration answered.
pub(super) fn site_schema(
    node: &LinkedNode<'_>,
    call: &ast::FuncCall<'_>,
    names: &tola_typst_syntax::names::SourceNames,
    semantics: &mut Semantic<'_>,
    site_path: &str,
) -> Result<Option<SiteSchema>> {
    let Some(callee) = node.find(call.callee().span()) else {
        return Ok(None);
    };
    if !calls_package_member(
        names,
        &callee,
        tola_packages::SOURCE_PACKAGE,
        tola_packages::PARSE_SOURCES,
    ) {
        return Ok(None);
    }
    // `parse-sources` binds its schema positionally, so a call that names it cannot build and
    // declares nothing.
    let arguments = call.args().items().collect::<Vec<_>>();
    let [ast::Arg::Pos(records), ast::Arg::Pos(schema)] = arguments.as_slice() else {
        return Ok(None);
    };
    let (records, schema) = (records.span(), schema.span());
    let Some(schema) = node.find(schema) else {
        return Ok(None);
    };
    let Some(shape) = declared_schema_at(&schema, semantics)? else {
        return Ok(None);
    };
    let Some(records) = node.find(records) else {
        return Ok(None);
    };
    let covered = semantics.record_files(&records)?;
    Ok(Some(SiteSchema {
        shape,
        declared_as: declaration_text(names.text(&schema.range())),
        site_path: site_path.to_owned(),
        covered,
    }))
}

/// One declaration expression as an answer names it: its own bytes, on one line and bounded.
fn declaration_text(text: &str) -> String {
    let mut line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    const LIMIT: usize = 40;
    if line.chars().count() > LIMIT {
        line = line.chars().take(LIMIT).collect();
        line.push('…');
    }
    line
}

/// The documentation one declared key has, naming each declaration that writes it.
pub(super) fn declared_documentation(
    fields: &[(schema::DeclaredField, String)],
    schemas: &SourceSchemas,
) -> String {
    let mut documentation = if !schemas.several {
        fields
            .iter()
            .map(|(field, _)| field.section())
            .collect::<Vec<_>>()
            .join(SECTION_JOIN)
    } else {
        let mut declared: Vec<(schema::DeclaredField, Vec<String>)> = Vec::new();
        for (field, declaration) in fields {
            match declared.iter().position(|(known, _)| known == field) {
                Some(index) => {
                    let declarations = &mut declared[index].1;
                    if !declarations.contains(declaration) {
                        declarations.push(declaration.clone());
                    }
                }
                None => declared.push((field.clone(), vec![declaration.clone()])),
            }
        }
        declared
            .into_iter()
            .map(|(field, declarations)| {
                format!(
                    "{}\n\nDeclared by {}.",
                    field.section(),
                    joined(&declarations)
                )
            })
            .collect::<Vec<_>>()
            .join(SECTION_JOIN)
    };
    if !schemas.covered {
        documentation.push_str(&format!(
            "{SECTION_JOIN}no `parse-sources` call parses this source yet, \
             so these are the site's fields rather than its own"
        ));
    }
    documentation
}

/// The conflict one key has when a call parsing this source rejects it: the site builds only
/// when every call accepts the key, and a call requiring it leaves no accepted spelling.
pub(super) fn rejected_key_note(rejecting: usize, required: bool) -> String {
    let rejected = if rejecting == 1 {
        "One call parsing this source rejects this key".to_owned()
    } else {
        format!("{rejecting} calls parsing this source reject this key")
    };
    let consequence = if required {
        "while another requires it, so the site fails either way"
    } else {
        "so the site builds only when every call accepts it"
    };
    format!("{rejected} {consequence}.")
}
impl Selection<'_> {
    /// The metadata shapes this source's records answer to; uncovered sources take every shape.
    pub(super) fn meta_schemas(
        &self,
        compilation: &SourceCompilation,
        config: &ResolvedSiteConfig,
        semantics: &mut Semantic<'_>,
    ) -> Result<SourceSchemas> {
        let declared = site_schemas(compilation, config, semantics)?;
        let several = declared.len() > 1;
        let covering: Vec<SiteSchema> = declared
            .iter()
            .filter(|schema| schema.covered.contains(&self.source.id()))
            .cloned()
            .collect();
        let covered = !covering.is_empty();
        let shapes = if covered { covering } else { declared };
        Ok(SourceSchemas {
            shapes,
            several,
            covered,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, sync::Arc};
    use tola_build::cancellation::BuildCancellation;
    use tola_build::check::SourceDiagnosticSession;

    use crate::query::tests::load_configuration;

    #[test]
    fn package_entry_owns_schema() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("project");
        let module = directory.path().join("module");
        fs::create_dir_all(root.join("content")).unwrap();
        fs::create_dir(&module).unwrap();
        fs::write(
            root.join("tola.toml"),
            "[modules.notes]\npath='../module'\n[theme]\nmodule='notes'\n",
        )
        .unwrap();
        fs::write(
            module.join("tola-module.toml"),
            "\nname='notes'\nversion='1.0.0'\n[typst]\nentry='lib.typ'\n[site]\nentry='site.typ'\n",
        )
        .unwrap();
        fs::write(module.join("lib.typ"), "").unwrap();
        fs::write(
            module.join("site.typ"),
            "#import \"records.typ\": records\n#metadata(records)<records>\n",
        )
        .unwrap();
        fs::write(
            module.join("records.typ"),
            r#"
#import "@tola/source:0.0.0": all-sources, parse-sources
#import "@tola/schema:0.0.0" as s
#let declared = s.schema((title: str, category: s.optional(str)))
#let records = parse-sources(all-sources(), declared)
"#,
        )
        .unwrap();
        fs::write(
            root.join("content/post.typ"),
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"A\"))",
        )
        .unwrap();
        let config = load_configuration(&root).expect("site configuration");
        assert!(config.entry_file().is_none());
        assert!(!root.join("site.typ").exists());
        let cancellation = BuildCancellation::default();
        let mut session = SourceDiagnosticSession::with_resources(
            Arc::clone(&config),
            tola_build::BuildResources::new().with_input_scope(tola_build::InputScope::Offline),
        );
        let revision = session.inspect(Vec::new(), &cancellation).unwrap();
        assert!(revision.compiled(), "{:?}", revision.diagnostics());
        let compilation = revision.checked().unwrap();
        assert!(matches!(
            compilation.site_entry().unwrap().root(),
            VirtualRoot::Package(_)
        ));
        let mut semantics = Semantic::new(compilation, &cancellation);
        let schemas = site_schemas(compilation, &config, &mut semantics).unwrap();
        assert_eq!(schemas.len(), 1);
        assert!(schemas[0].declares("category"));
        assert!(
            schemas[0]
                .declaration()
                .contains("@tola-modules/notes:0.0.0/records.typ")
        );
    }
}
