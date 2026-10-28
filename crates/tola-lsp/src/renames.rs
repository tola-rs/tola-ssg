//! The paths a file rename moves, and the edit that keeps every source reaching them.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use anyhow::Result;
use lsp_types::{TextEdit, Uri, WorkspaceEdit};
use tola_build::cancellation::BuildCancellation;
use tola_build::config::ResolvedSiteConfig;
use tola_typst::typst::foundations::Repr;
use tola_typst::typst::syntax::{LinkedNode, Source, ast};
use tola_typst_syntax::syntax::node_at_range;

use crate::links;

/// The edit that keeps every import pointing at a file the editor renames.
///
/// A rename moves a file, so the paths that reach it change: every source that writes one is
/// edited to the path that reaches the new location from its own directory.
pub(super) fn imports(
    config: &ResolvedSiteConfig,
    client_root: &crate::uri::ClientRoot,
    view: &crate::sources::SourceView,
    files: &[(PathBuf, PathBuf)],
    boundary: &tola_typst::SourceBoundary,
    disk: &mut crate::analysis::DiskSources,
    cancellation: &BuildCancellation,
) -> Result<Option<WorkspaceEdit>> {
    let root = tola_build::filesystem::normalize_existing_prefix(config.get_root());
    let sources = crate::analysis::site_sources(&root, view, boundary, disk, cancellation)?;
    #[expect(clippy::mutable_key_type, reason = "the protocol keys edits by URI")]
    let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
    for names in sources {
        cancellation.ensure_active()?;
        let source = names.source();
        let edits = source_edits(source, &root, files, cancellation)?;
        if edits.is_empty() {
            continue;
        }
        let source_path = root.join(source.id().vpath().get_without_slash());
        let Ok(uri) = client_root.address(&source_path) else {
            continue;
        };
        changes.insert(uri, edits);
    }
    Ok((!changes.is_empty()).then(|| WorkspaceEdit {
        changes: Some(changes),
        ..WorkspaceEdit::default()
    }))
}

/// The edits that keep one source's links reaching the files they name after the rename.
fn source_edits(
    source: &Source,
    root: &Path,
    files: &[(PathBuf, PathBuf)],
    cancellation: &BuildCancellation,
) -> Result<Vec<TextEdit>> {
    let source_path = root.join(source.id().vpath().get_without_slash());
    let moved_source = moved_path(&source_path, root, files);
    let new_source = moved_source.as_deref().unwrap_or(&source_path);
    let mut edits = Vec::new();
    for link in links::declared(source) {
        cancellation.ensure_active()?;
        let Some(target) = link.site_path(root) else {
            continue;
        };
        let moved_target = moved_path(&target, root, files);
        let new_target = moved_target.as_deref().unwrap_or(&target);
        let Some(literal) = node_at_range(source, &link.literal) else {
            continue;
        };
        let Some(written) = literal.cast::<ast::Str>() else {
            continue;
        };
        let rooted = written.get().starts_with('/');
        let spell = |writer: &Path, target: &Path| {
            if rooted {
                target.strip_prefix(root).ok().map(|path| {
                    let path = path
                        .components()
                        .map(|part| part.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/");
                    format!("/{path}")
                })
            } else {
                relative_path(writer, target)
            }
        };
        let Some(new_path) = spell(new_source, new_target) else {
            continue;
        };
        if spell(&source_path, &target).as_ref() == Some(&new_path) {
            continue;
        }
        edits.push(TextEdit {
            range: link.range,
            new_text: quoted_path(&new_path),
        });
        if let Some(alias) = module_alias(source, &literal, &new_path) {
            edits.push(alias);
        }
    }
    Ok(edits)
}

/// The text that writes `path` inside a Typst string, quotation marks excluded.
fn quoted_path(path: &str) -> String {
    let quoted = path.repr();
    quoted[1..quoted.len() - 1].to_owned()
}

fn module_alias(source: &Source, literal: &LinkedNode<'_>, replacement: &str) -> Option<TextEdit> {
    let import = literal.parent()?.cast::<ast::ModuleImport>()?;
    if import.new_name().is_some() || import.imports().is_some() {
        return None;
    }
    let name = import.bare_name().ok()?;
    if Path::new(replacement)
        .file_stem()
        .and_then(|stem| stem.to_str())
        == Some(name.as_str())
    {
        return None;
    }
    Some(TextEdit {
        range: crate::position::utf16_range(
            source.lines(),
            literal.range().end..literal.range().end,
        )?,
        new_text: format!(" as {name}"),
    })
}

/// The path that reaches `to` from the directory of `from`, with `/` separators.
fn relative_path(from: &Path, to: &Path) -> Option<String> {
    let directory: Vec<Component<'_>> = from.parent()?.components().collect();
    let target: Vec<Component<'_>> = to.components().collect();
    let common = directory
        .iter()
        .zip(&target)
        .take_while(|(left, right)| left == right)
        .count();
    let mut parts = vec!["..".to_owned(); directory.len() - common];
    parts.extend(
        target[common..]
            .iter()
            .map(|component| component.as_os_str().to_string_lossy().into_owned()),
    );
    Some(parts.join("/"))
}

/// The destination of one site path under the editor's file or directory renames.
///
/// Renaming the site root or an ancestor changes no site-relative path.
fn moved_path(path: &Path, root: &Path, files: &[(PathBuf, PathBuf)]) -> Option<PathBuf> {
    files.iter().rev().find_map(|(old, new)| {
        let inside = old.strip_prefix(root).ok()?;
        if inside.as_os_str().is_empty() {
            return None;
        }
        Some(new.join(path.strip_prefix(old).ok()?))
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    /// A site root reachable without symlinks: resolved paths take the target's spelling.
    fn temp_site() -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        (directory, root)
    }

    fn rename(
        config: &ResolvedSiteConfig,
        root: &Path,
        files: &[(PathBuf, PathBuf)],
    ) -> Option<WorkspaceEdit> {
        imports(
            config,
            &crate::uri::ClientRoot::new(root),
            &crate::sources::SourceView::default(),
            files,
            &tola_typst::SourceBoundary::new(root, true),
            &mut crate::analysis::DiskSources::default(),
            &BuildCancellation::new(),
        )
        .unwrap()
    }

    /// The edits the rename makes to the site's `content/document.typ`.
    fn document_edits(edit: WorkspaceEdit, root: &Path) -> Vec<TextEdit> {
        let uri = crate::uri::from_file_path(&root.join("content/document.typ")).unwrap();
        edit.changes
            .expect("edits by file")
            .into_iter()
            .find_map(|(target, edits)| (target == uri).then_some(edits))
            .expect("the edit targets the importing document")
    }

    #[test]
    fn moved_importer_rewrites_its_link() {
        let (_directory, root) = temp_site();
        let config = site_config(
            &root,
            "#include \"head.typ\"\n",
            &[("content/head.typ", "Head\n")],
        );
        let edit = rename(
            &config,
            &root,
            &[(root.join("content/document.typ"), root.join("document.typ"))],
        )
        .expect("moving the importer changes its relative link");
        assert_eq!(document_edits(edit, &root)[0].new_text, "content/head.typ");
    }
    #[test]
    fn rename_updates_unsaved_importer() {
        let (_directory, root) = temp_site();
        let config = site_config(&root, "Body\n", &[("content/head.typ", "Head\n")]);
        let draft = root.join("content/draft.typ");
        let uri = crate::uri::from_file_path(&draft).unwrap();
        let mut open = crate::sources::OpenSources::new(&root);
        open.apply(lsp_server::Notification::new(
            "textDocument/didOpen".into(),
            lsp_types::DidOpenTextDocumentParams {
                text_document: lsp_types::TextDocumentItem {
                    uri: uri.clone(),
                    language_id: "typst".into(),
                    version: 1,
                    text: "#include \"head.typ\"\n".into(),
                },
            },
        ))
        .unwrap();
        let edit = imports(
            &config,
            &crate::uri::ClientRoot::new(&root),
            &open.view(),
            &[(
                root.join("content/head.typ"),
                root.join("content/title.typ"),
            )],
            &tola_typst::SourceBoundary::new(&root, true),
            &mut crate::analysis::DiskSources::default(),
            &BuildCancellation::new(),
        )
        .unwrap()
        .expect("the unsaved importer follows the renamed file");
        let changes = edit.changes.unwrap().into_iter().collect::<Vec<_>>();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].0, uri);
        assert_eq!(changes[0].1[0].new_text, "title.typ");
        assert!(!draft.exists());
    }

    #[test]
    fn escaped_root_path_keeps_root_spelling() {
        let (_directory, root) = temp_site();
        let config = site_config(
            &root,
            "#include \"\\u{2f}templates/head.typ\"\n",
            &[("templates/head.typ", "Head\n")],
        );
        let edit = rename(
            &config,
            &root,
            &[(
                root.join("templates/head.typ"),
                root.join("templates/title.typ"),
            )],
        )
        .unwrap();
        assert_eq!(
            document_edits(edit, &root)[0].new_text,
            "/templates/title.typ"
        );
    }

    #[test]
    fn rename_reaches_linked_sources() {
        for (directory, relation) in [
            (".hidden", "import"),
            ("ignored", "import"),
            (".hidden", "include"),
            ("ignored", "include"),
        ] {
            let (_directory, root) = temp_site();
            let helper = format!("{directory}/helper.typ");
            let config = site_config(
                &root,
                &format!("#{relation} \"../{helper}\"\n"),
                &[
                    (
                        &helper,
                        "#let marker = 1\n#include \"../content/head.typ\"\n",
                    ),
                    ("content/head.typ", "Head\n"),
                ],
            );
            std::fs::write(root.join(".ignore"), format!("{directory}/\n")).unwrap();
            let edit = rename(
                &config,
                &root,
                &[(
                    root.join("content/head.typ"),
                    root.join("content/title.typ"),
                )],
            )
            .expect("a reached source keeps its own path current");
            let changes = edit.changes.unwrap().into_iter().collect::<Vec<_>>();
            assert_eq!(changes.len(), 1);
            assert_eq!(
                changes[0].0,
                crate::uri::from_file_path(&root.join(helper)).unwrap()
            );
            assert_eq!(changes[0].1[0].new_text, "../content/title.typ");
        }
    }

    #[test]
    fn folder_rename_updates_resource_paths() {
        let cases = [
            (
                "#image(\"../assets/photo.png\")\n",
                &[("assets/photo.png", "image bytes")][..],
                &["../images/photo.png"][..],
            ),
            (
                "#bibliography((\"../assets/refs.bib\", \"../assets/more.bib\"))\n",
                &[("assets/refs.bib", ""), ("assets/more.bib", "")][..],
                &["../images/refs.bib", "../images/more.bib"][..],
            ),
        ];
        for (document, files, expected) in cases {
            let (_directory, root) = temp_site();
            let config = site_config(&root, document, files);
            let edit = rename(
                &config,
                &root,
                &[(root.join("assets"), root.join("images"))],
            )
            .unwrap();
            let edits = document_edits(edit, &root);
            assert_eq!(
                edits
                    .iter()
                    .map(|edit| edit.new_text.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }

    #[test]
    fn rename_rewrites_reaching_paths() {
        for (moved, expected) in [
            (
                ("templates/partials/head.typ", "templates/head.typ"),
                "../templates/head.typ",
            ),
            (("templates", "layouts"), "../layouts/partials/head.typ"),
        ] {
            let (_directory, root) = temp_site();
            let config = site_config(
                &root,
                "#import \"../templates/partials/head.typ\": head\nBody\n",
                &[("templates/partials/head.typ", "Head\n")],
            );
            let edit = rename(&config, &root, &[(root.join(moved.0), root.join(moved.1))])
                .expect("one edit");
            let mut files = edit.changes.expect("edits by file").into_values();
            let edits = files.next().expect("the importing file");
            assert!(files.next().is_none(), "only the importer is edited");
            assert_eq!(edits.len(), 1);
            assert_eq!(edits[0].new_text, expected, "{moved:?}");
        }
    }

    /// A folder above the site holds the whole site, so renaming it moves nothing the site's own
    /// paths name.
    #[test]
    fn rename_above_site_leaves_links_alone() {
        let (_directory, workspace) = temp_site();
        let root = workspace.join("site");
        let config = site_config(
            &root,
            "#include \"head.typ\"\nBody\n",
            &[("content/head.typ", "Head\n")],
        );
        let renamed = [(workspace.clone(), workspace.join("renamed"))];
        assert!(rename(&config, &root, &renamed).is_none());
    }
    /// A site below `root` whose `content/document.typ` holds `document`, with every other file
    /// written as named and its empty `tola.toml` loaded.
    fn site_config(root: &Path, document: &str, files: &[(&str, &str)]) -> Arc<ResolvedSiteConfig> {
        std::fs::create_dir_all(root.join("content")).unwrap();
        std::fs::write(root.join("content/document.typ"), document).unwrap();
        for (name, text) in files {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        crate::compiler::tests::site_configuration(root, "")
    }
}
