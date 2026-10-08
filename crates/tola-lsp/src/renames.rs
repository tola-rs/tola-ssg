//! The paths a file rename moves, and the edit that keeps every source reaching them.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use anyhow::Result;
use lsp_types::{TextEdit, Uri, WorkspaceEdit};
use tola_build::config::ResolvedSiteConfig;
use tola_typst::typst::foundations::Repr;
use tola_typst::typst::syntax::{FileId, Source};

use crate::links;

/// The edit that keeps every import pointing at a file the editor renames.
///
/// A rename moves a file, so the paths that reach it change: every source that writes one is
/// edited to the path that reaches the new location from its own directory.
pub(super) fn imports(
    config: &ResolvedSiteConfig,
    client_root: &crate::uri::ClientRoot,
    overrides: &[(PathBuf, std::sync::Arc<str>)],
    files: &[(PathBuf, PathBuf)],
    boundary: &tola_typst::SourceBoundary,
) -> Result<Option<WorkspaceEdit>> {
    let root = tola_build::filesystem::normalize_existing_prefix(config.get_root());
    let site = crate::files::site_files(config);
    let moved = moved_paths(&site, &root, files);
    #[expect(clippy::mutable_key_type, reason = "the protocol keys edits by URI")]
    let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
    for id in site.iter().copied() {
        // The same file is named twice: an editor edit is keyed by the path the configuration root
        // spells, and a link target by the path the filesystem resolves.
        let configured = config.get_root().join(id.vpath().get_without_slash());
        let resolved = root.join(id.vpath().get_without_slash());
        if resolved.extension() != Some(std::ffi::OsStr::new("typ")) {
            continue;
        }
        if boundary.check(&resolved).is_err() {
            continue;
        }
        let text = match overrides.iter().find(|(known, _)| {
            *known == configured
                || tola_build::filesystem::normalize_existing_prefix(known) == resolved
        }) {
            Some((_, text)) => text.to_string(),
            None => match std::fs::read_to_string(&resolved) {
                Ok(text) => text,
                Err(_) => continue,
            },
        };
        let source = Source::new(id, text);
        let mut edits = Vec::new();
        for link in links::paths(&source, &root).unwrap_or_default() {
            let Some(target) = link.target.as_ref() else {
                continue;
            };
            let Ok(target) = crate::uri::to_site_path(target.as_str()) else {
                continue;
            };
            let new_writer = moved.get(&resolved).unwrap_or(&resolved);
            let new_target = moved.get(&target).unwrap_or(&target);
            let Some(start) = crate::position::byte_offset(source.lines(), link.range.start).ok()
            else {
                continue;
            };
            let rooted = source.text()[start..].starts_with('/');
            let spell = |writer: &Path, target: &Path| {
                if rooted {
                    target.strip_prefix(&root).ok().map(|path| {
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
            let Some(new_path) = spell(new_writer, new_target) else {
                continue;
            };
            if spell(&resolved, &target).as_ref() == Some(&new_path) {
                continue;
            }
            let literal = new_path.as_str().repr();
            edits.push(TextEdit {
                range: link.range,
                new_text: literal[1..literal.len() - 1].to_owned(),
            });
        }
        if edits.is_empty() {
            continue;
        }
        let Ok(uri) = client_root.address(&resolved) else {
            continue;
        };
        changes.insert(uri, edits);
    }
    Ok((!changes.is_empty()).then(|| WorkspaceEdit {
        changes: Some(changes),
        ..WorkspaceEdit::default()
    }))
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

/// The site files a rename moves, keyed by the path they are read at.
///
/// A folder rename moves every site file under it, and the suffix past the folder is what joins the
/// new folder. A rename of the site root or of a folder above it moves the site's files without
/// changing any path between them, and a file the editor renames outside the site is no site file
/// at all: the site's own file list is what caps both.
fn moved_paths(
    site: &[FileId],
    root: &Path,
    files: &[(PathBuf, PathBuf)],
) -> HashMap<PathBuf, PathBuf> {
    let mut moved = HashMap::new();
    for id in site {
        let resolved = root.join(id.vpath().get_without_slash());
        for (old, new) in files {
            let Ok(inside) = old.strip_prefix(root) else {
                continue;
            };
            if inside.as_os_str().is_empty() {
                continue;
            }
            if let Ok(suffix) = resolved.strip_prefix(old) {
                moved.insert(resolved.clone(), new.join(suffix));
            }
        }
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A site root reachable without symlinks: resolved paths take the target's spelling.
    fn temp_site() -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        (directory, root)
    }

    /// The edits `imports` answers with for `files`, over the resolved root and no overrides.
    fn rename(
        config: &ResolvedSiteConfig,
        root: &Path,
        files: &[(PathBuf, PathBuf)],
    ) -> Option<WorkspaceEdit> {
        imports(
            config,
            &crate::uri::ClientRoot::new(root),
            &[],
            files,
            &tola_typst::SourceBoundary::new(root, true),
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
    fn moved_folder_keeps_internal_links() {
        let (_directory, root) = temp_site();
        let config = site_config(
            &root,
            "#include \"head.typ\"\n",
            &[("content/head.typ", "Head\n")],
        );
        assert!(
            rename(
                &config,
                &root,
                &[(root.join("content"), root.join("pages"))]
            )
            .is_none()
        );
    }

    #[test]
    fn rooted_link_escapes_written_path() {
        let (_directory, root) = temp_site();
        let config = site_config(
            &root,
            "#include \"/content/head.typ\"\n",
            &[("content/head.typ", "Head\n")],
        );
        let edit = rename(
            &config,
            &root,
            &[(
                root.join("content/head.typ"),
                root.join("content/quoted\"head.typ"),
            )],
        )
        .expect("renaming the target changes its root path");
        assert_eq!(
            document_edits(edit, &root)[0].new_text,
            "/content/quoted\\\"head.typ"
        );
    }

    #[test]
    fn every_kind_of_path_is_rewritten() {
        let (_directory, root) = temp_site();
        let config = site_config(
            &root,
            "#include \"head.typ\"\n#read(\"data.json\")\n",
            &[
                ("content/head.typ", "Head\n"),
                ("content/data.json", "{}\n"),
                ("README.md", "#include \"content/head.typ\"\n"),
                ("notes.txt", "#read(\"content/data.json\")\n"),
            ],
        );
        let edit = rename(
            &config,
            &root,
            &[
                (root.join("content/head.typ"), root.join("content/part.typ")),
                (
                    root.join("content/data.json"),
                    root.join("content/site.json"),
                ),
            ],
        )
        .expect("one edit");
        assert_eq!(edit.changes.as_ref().unwrap().len(), 1);
        let edits = document_edits(edit, &root);
        let texts: Vec<&str> = edits.iter().map(|edit| edit.new_text.as_str()).collect();
        assert_eq!(texts, ["part.typ", "site.json"], "{edits:?}");
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

    #[test]
    fn edit_keys_keep_the_client_root_spelling() {
        let (directory, root) = temp_site();
        let spelled = directory.path().join("client");
        let config = site_config(
            &root,
            "#include \"head.typ\"\nBody\n",
            &[("content/head.typ", "Head\n")],
        );
        let edit = imports(
            &config,
            &crate::uri::ClientRoot::with_resolved(&spelled, &root),
            &[],
            &[(root.join("content/head.typ"), root.join("content/part.typ"))],
            &tola_typst::SourceBoundary::new(&root, true),
        )
        .unwrap()
        .expect("one edit");
        let key = edit
            .changes
            .expect("edits by file")
            .into_keys()
            .next()
            .expect("the importing file");
        assert_eq!(
            key,
            crate::uri::from_file_path(&spelled.join("content/document.typ")).unwrap()
        );
    }

    #[test]
    fn overrides_match_through_root_spelling() {
        let (_directory, root) = temp_site();
        let config = site_config(
            &root,
            "#include \"head.typ\"\n",
            &[("content/head.typ", "Head\n")],
        );
        let spelled = root.join("content").join("..").join("content/document.typ");
        let overrides: [(PathBuf, std::sync::Arc<str>); 1] = [(
            spelled,
            std::sync::Arc::from("#include \"../content/head.typ\"\n"),
        )];
        let edit = imports(
            &config,
            &crate::uri::ClientRoot::new(&root),
            &overrides,
            &[(root.join("content/head.typ"), root.join("content/part.typ"))],
            &tola_typst::SourceBoundary::new(&root, true),
        )
        .unwrap()
        .expect("one edit");
        let uri = crate::uri::from_file_path(&root.join("content/document.typ")).unwrap();
        let edits = edit.changes.expect("edits by file").remove(&uri).unwrap();
        assert_eq!(edits.len(), 1);
        let edit = &edits[0];
        let source = Source::detached(overrides[0].1.to_string());
        let start = crate::position::byte_offset(source.lines(), edit.range.start).unwrap();
        let end = crate::position::byte_offset(source.lines(), edit.range.end).unwrap();
        let mut text = source.text().to_owned();
        text.replace_range(start..end, &edit.new_text);
        assert_eq!(text, "#include \"part.typ\"\n");
    }

    /// A site below `root` whose `content/document.typ` holds `document`, with every other file
    /// written as named and its empty `tola.toml` loaded.
    fn site_config(root: &Path, document: &str, files: &[(&str, &str)]) -> ResolvedSiteConfig {
        std::fs::create_dir_all(root.join("content")).unwrap();
        std::fs::write(root.join("content/document.typ"), document).unwrap();
        for (name, text) in files {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let configuration = root.join("tola.toml");
        std::fs::write(&configuration, "").unwrap();
        tola_build::config::loading::load_site_config(
            Some(&configuration),
            tola_typst::PackageLocations::default(),
            &tola_build::config::loading::BuildOverrides::default(),
        )
        .unwrap()
        .into_config()
    }
}
