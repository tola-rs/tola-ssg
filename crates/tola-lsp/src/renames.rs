//! Import/include/resource path rewrites when files or directories are renamed.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use anyhow::Result;
use lsp_types::{TextEdit, Uri, WorkspaceEdit};
use tola_build::cancellation::BuildCancellation;
use tola_build::config::SiteConfig;
use tola_typst::typst::foundations::Repr;
use tola_typst::typst::syntax::{LinkedNode, Source, ast};
use tola_typst_syntax::syntax::node_at_range;

use crate::links;

/// Compute workspace edits keeping source links pointed at moved files.
pub(super) fn imports(
    cfg: &SiteConfig,
    client_root: &crate::uri::ClientRoot,
    view: &crate::sources::SourceView,
    files: &[(PathBuf, PathBuf)],
    boundary: &tola_typst::SourceBoundary,
    disk: &mut crate::analysis::DiskSources,
    cancel: &BuildCancellation,
) -> Result<Option<WorkspaceEdit>> {
    let root = tola_input::normalize_existing_prefix(cfg.root());
    let sources = crate::analysis::site_sources(&root, view, boundary, disk, cancel)?;
    let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
    for names in sources {
        cancel.ensure_active()?;
        let src = names.source();
        let edits = source_edits(src, &root, files, cancel)?;
        if edits.is_empty() {
            continue;
        }
        let src_path = root.join(src.id().vpath().get_without_slash());
        let Ok(uri) = client_root.address(&src_path) else {
            continue;
        };
        changes.insert(uri, edits);
    }
    Ok((!changes.is_empty()).then(|| WorkspaceEdit {
        changes: Some(changes),
        ..WorkspaceEdit::default()
    }))
}

fn source_edits(
    src: &Source,
    root: &Path,
    files: &[(PathBuf, PathBuf)],
    cancel: &BuildCancellation,
) -> Result<Vec<TextEdit>> {
    let src_path = root.join(src.id().vpath().get_without_slash());
    let moved_src = moved_path(&src_path, root, files);
    let new_src = moved_src.as_deref().unwrap_or(&src_path);
    let mut edits = Vec::new();
    for link in links::declared(src) {
        cancel.ensure_active()?;
        let Some(target) = link.site_path(root) else {
            continue;
        };
        let moved_target = moved_path(&target, root, files);
        let new_target = moved_target.as_deref().unwrap_or(&target);
        let Some(lit) = node_at_range(src, &link.literal) else {
            continue;
        };
        let Some(written) = lit.cast::<ast::Str>() else {
            continue;
        };
        let rooted = written.get().starts_with('/');
        let spell = |writer: &Path, dst: &Path| {
            if rooted {
                dst.strip_prefix(root).ok().map(|path| {
                    let path = path
                        .components()
                        .map(|part| part.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/");
                    format!("/{path}")
                })
            } else {
                relative_path(writer, dst)
            }
        };
        let Some(new_path) = spell(new_src, new_target) else {
            continue;
        };
        if spell(&src_path, &target).as_ref() == Some(&new_path) {
            continue;
        }
        edits.push(TextEdit {
            range: link.range,
            new_text: quoted_path(&new_path),
        });
        if let Some(alias) = module_alias(src, &lit, &new_path) {
            edits.push(alias);
        }
    }
    Ok(edits)
}

fn quoted_path(path: &str) -> String {
    let quoted = path.repr();
    quoted[1..quoted.len() - 1].to_owned()
}

fn module_alias(src: &Source, lit: &LinkedNode<'_>, replacement: &str) -> Option<TextEdit> {
    let import = lit.parent()?.cast::<ast::ModuleImport>()?;
    if import.new_name().is_some() || import.imports().is_some() {
        return None;
    }
    let name = import.bare_name().ok()?;
    if Path::new(replacement).file_stem().and_then(|s| s.to_str()) == Some(name.as_str()) {
        return None;
    }
    Some(TextEdit {
        range: crate::position::utf16_range(src.lines(), lit.range().end..lit.range().end)?,
        new_text: format!(" as {name}"),
    })
}

fn relative_path(from: &Path, to: &Path) -> Option<String> {
    let dir: Vec<Component<'_>> = from.parent()?.components().collect();
    let target: Vec<Component<'_>> = to.components().collect();
    let common = dir.iter().zip(&target).take_while(|(a, b)| a == b).count();
    let mut parts = vec!["..".to_owned(); dir.len() - common];
    parts.extend(
        target[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    Some(parts.join("/"))
}

fn moved_path(path: &Path, root: &Path, files: &[(PathBuf, PathBuf)]) -> Option<PathBuf> {
    // Renaming the site root or an ancestor changes no site-relative path.
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

    fn temp_site() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        (dir, root)
    }

    fn rename(
        cfg: &SiteConfig,
        root: &Path,
        files: &[(PathBuf, PathBuf)],
    ) -> Option<WorkspaceEdit> {
        imports(
            cfg,
            &crate::uri::ClientRoot::new(root),
            &crate::sources::SourceView::default(),
            files,
            &tola_typst::SourceBoundary::new(root, true),
            &mut crate::analysis::DiskSources::default(),
            &BuildCancellation::new(),
        )
        .unwrap()
    }

    fn document_edits(edit: WorkspaceEdit, root: &Path) -> Vec<TextEdit> {
        let uri = crate::uri::from_file_path(&root.join("content/document.typ")).unwrap();
        edit.changes
            .expect("edits by file")
            .into_iter()
            .find_map(|(target, edits)| (target == uri).then_some(edits))
            .expect("the edit targets the importing document")
    }

    #[test]
    fn rename_path_rewrite() {
        for (doc, files, moved, expected) in [
            (
                "#include \"head.typ\"\n",
                &[("content/head.typ", "Head\n")][..],
                ("content/document.typ", "document.typ"),
                &["content/head.typ"][..],
            ),
            (
                "#image(\"../assets/photo.png\")\n",
                &[("assets/photo.png", "image bytes")][..],
                ("assets", "images"),
                &["../images/photo.png"][..],
            ),
            (
                "#bibliography((\"../assets/refs.bib\", \"../assets/more.bib\"))\n",
                &[("assets/refs.bib", ""), ("assets/more.bib", "")][..],
                ("assets", "images"),
                &["../images/refs.bib", "../images/more.bib"][..],
            ),
            (
                "#import \"../templates/partials/head.typ\": head\nBody\n",
                &[("templates/partials/head.typ", "Head\n")][..],
                ("templates/partials/head.typ", "templates/head.typ"),
                &["../templates/head.typ"][..],
            ),
            (
                "#import \"../templates/partials/head.typ\": head\nBody\n",
                &[("templates/partials/head.typ", "Head\n")][..],
                ("templates", "layouts"),
                &["../layouts/partials/head.typ"][..],
            ),
        ] {
            let (_dir, root) = temp_site();
            let cfg = site_cfg(&root, doc, files);
            let edit = rename(&cfg, &root, &[(root.join(moved.0), root.join(moved.1))])
                .expect("the rename rewrites the reaching path");
            let edits = document_edits(edit, &root);
            assert_eq!(
                edits
                    .iter()
                    .map(|edit| edit.new_text.as_str())
                    .collect::<Vec<_>>(),
                expected,
                "{moved:?}"
            );
        }
    }

    #[test]
    fn rename_updates_unsaved_importer() {
        let (_dir, root) = temp_site();
        let cfg = site_cfg(&root, "Body\n", &[("content/head.typ", "Head\n")]);
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
            &cfg,
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
    fn escaped_root_path() {
        let (_dir, root) = temp_site();
        let cfg = site_cfg(
            &root,
            "#include \"\\u{2f}templates/head.typ\"\n",
            &[("templates/head.typ", "Head\n")],
        );
        let edit = rename(
            &cfg,
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
        for (dir, rel) in [
            (".hidden", "import"),
            ("ignored", "import"),
            (".hidden", "include"),
            ("ignored", "include"),
        ] {
            let (_dir, root) = temp_site();
            let helper = format!("{dir}/helper.typ");
            let cfg = site_cfg(
                &root,
                &format!("#{rel} \"../{helper}\"\n"),
                &[
                    (
                        &helper,
                        "#let marker = 1\n#include \"../content/head.typ\"\n",
                    ),
                    ("content/head.typ", "Head\n"),
                ],
            );
            std::fs::write(root.join(".ignore"), format!("{dir}/\n")).unwrap();
            let edit = rename(
                &cfg,
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
    fn rename_above_site_ignored() {
        let (_dir, ws) = temp_site();
        let root = ws.join("site");
        let cfg = site_cfg(
            &root,
            "#include \"head.typ\"\nBody\n",
            &[("content/head.typ", "Head\n")],
        );
        let renamed = [(ws.clone(), ws.join("renamed"))];
        assert!(rename(&cfg, &root, &renamed).is_none());
    }

    fn site_cfg(root: &Path, doc: &str, files: &[(&str, &str)]) -> Arc<SiteConfig> {
        std::fs::create_dir_all(root.join("content")).unwrap();
        std::fs::write(root.join("content/document.typ"), doc).unwrap();
        for (name, text) in files {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        crate::compiler::tests::site_configuration(root, "")
    }
}
