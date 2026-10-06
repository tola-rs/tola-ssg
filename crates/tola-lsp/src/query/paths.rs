//! The two spellings a site path may be written in, and the conversion between them.
//!
//! A path names one file either from the site root or from the file that writes it. Both spellings
//! name the same file, so a cursor on a path a source declares offers the spelling the author has
//! not written.

use lsp_types::TextEdit;
use tola_typst::typst::foundations::PathOrStr;
use tola_typst::typst::syntax::{RootedPath, Source, VirtualPath, VirtualRoot, ast};

use crate::position;

/// The action that rewrites the path the source declares at `cursor` into the spelling the author
/// has not written. A path with only one spelling offers no action.
pub(crate) fn rewrites(
    source: &Source,
    cursor: usize,
    written_range: Option<lsp_types::Range>,
) -> Vec<(String, TextEdit)> {
    // `/` names the site root, so a document inside a package has no site spelling to convert to.
    if !matches!(source.id().root(), VirtualRoot::Project) {
        return Vec::new();
    }
    // An asset argument names an output the site creates, from the site root, so its two spellings
    // are what the author wrote and the same path with the root's slash. No file answers either, so
    // this decides before the file-based path below does.
    if let Some(argument) = tola_typst_syntax::syntax::path_argument(source, cursor)
        && argument.base == tola_typst_syntax::syntax::PathBase::Site
    {
        let written = source
            .text()
            .get(argument.range.clone())
            .unwrap_or_default();
        let spelling = match written.strip_prefix('/') {
            Some(rest) => rest.to_owned(),
            None => format!("/{written}"),
        };
        let Some(range) = position::utf16_range(source.lines(), argument.range) else {
            return Vec::new();
        };
        // `/` names the site root, which is no output the site publishes: a path with one
        // spelling offers no action.
        if spelling.is_empty() || !no_escape_needed(&spelling) {
            return Vec::new();
        }
        return vec![(
            format!("replace `{written}` with `{spelling}`"),
            TextEdit {
                range,
                new_text: spelling,
            },
        )];
    }
    // `crate::links` owns which strings are paths and where they sit, so the range it finds is the
    // one this edits, while the file behind the path comes from Typst's own resolution below.
    let Some(range) = written_range else {
        return Vec::new();
    };
    let Some(written) = written(source, cursor) else {
        return Vec::new();
    };
    let Some(target) = named_file(source, &written) else {
        return Vec::new();
    };
    let spelling = if written.starts_with('/') {
        let Some(relative) = relative_spelling(target.vpath(), source.id().vpath()) else {
            return Vec::new();
        };
        relative
    } else {
        target.vpath().get_with_slash().to_owned()
    };
    // The edit replaces the author's own path, so the other spelling has to reach the file that path
    // reaches, and it has to be writable between the quotation marks that enclose it.
    let Some(other) = named_file(source, &spelling) else {
        return Vec::new();
    };
    if other != target || !no_escape_needed(&spelling) {
        return Vec::new();
    }
    vec![(
        format!("replace `{written}` with `{spelling}`"),
        TextEdit {
            range,
            new_text: spelling,
        },
    )]
}

/// The text one declared path is written as.
fn written(source: &Source, cursor: usize) -> Option<String> {
    let node = tola_typst_syntax::syntax::expression(source, cursor)?;
    Some(node.cast::<ast::Str>()?.get().to_string())
}

/// The file `spelling` names from `source`, or `None` when it names no file: the site root is a
/// directory, and Typst refuses a path that escapes the root.
fn named_file(source: &Source, spelling: &str) -> Option<RootedPath> {
    let resolved = PathOrStr::Str(spelling.into()).resolve(source.id()).ok()?;
    (!resolved.vpath().is_root()).then_some(resolved)
}

/// The spelling of `target` seen from the directory of the file at `writer`, or `None` when the two
/// name one directory, which no path spells.
fn relative_spelling(target: &VirtualPath, writer: &VirtualPath) -> Option<String> {
    let spelling = target.relative_from(&writer.parent()?);
    if spelling.is_empty() {
        return None;
    }
    Some(spelling.to_string())
}

/// Whether `spelling` stands between the quotation marks of a Typst string as it is written: one
/// holding a quote, a backslash, or a line break would end the string there instead.
fn no_escape_needed(spelling: &str) -> bool {
    !spelling.contains(['"', '\\']) && !spelling.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position;

    /// The actions the path at the first quotation mark of `text` offers, with the source they edit.
    fn offered(writer: &str, text: &str) -> (Source, Vec<(String, TextEdit)>) {
        let id = RootedPath::new(
            VirtualRoot::Project,
            VirtualPath::new(writer).expect("a site path"),
        )
        .intern();
        let source = Source::new(id, text.to_owned());
        let cursor = source.text().find('"').expect("a path") + 2;
        let actions = rewrites(&source, cursor, written_range(&source, cursor));
        (source, actions)
    }

    /// The range the path at `cursor` occupies, as the links the reply collects report it.
    fn written_range(source: &Source, cursor: usize) -> Option<lsp_types::Range> {
        let at = position::utf16_range(source.lines(), cursor..cursor)?.start;
        let root = std::env::current_dir().expect("the tests run in the crate's directory");
        crate::links::paths(source, &root)?
            .into_iter()
            .find(|link| link.range.start <= at && at <= link.range.end)
            .map(|link| link.range)
    }

    /// The text of `source` one edit replaces.
    fn replaced(source: &Source, edit: &TextEdit) -> String {
        let lines = source.lines();
        let start = position::byte_offset(lines, edit.range.start).expect("a path start");
        let end = position::byte_offset(lines, edit.range.end).expect("a path end");
        source.text()[start..end].to_owned()
    }

    /// A path naming one target offers the other spelling of it: a relative or root import, a
    /// site-root file, a reader call's argument, and an asset argument.
    #[test]
    fn paths_offer_their_other_spelling() {
        let cases: [(&str, &str, &str, Option<&str>); 5] = [
            (
                "content/post.typ",
                "#import \"../templates/page.typ\": page\n",
                "/templates/page.typ",
                Some("../templates/page.typ"),
            ),
            (
                "content/post.typ",
                "#import \"/templates/page.typ\": page\n",
                "../templates/page.typ",
                Some("/templates/page.typ"),
            ),
            (
                "site.typ",
                "#import \"/templates/page.typ\": page\n",
                "templates/page.typ",
                None,
            ),
            (
                "site.typ",
                "#let data = read(\"data/site.json\")\n",
                "/data/site.json",
                Some("data/site.json"),
            ),
            // An asset argument names an output from the site root, so it converts without a file
            // answering either spelling.
            (
                "site.typ",
                "#asset(\"/downloads/file.pdf\", bytes)\n",
                "downloads/file.pdf",
                Some("/downloads/file.pdf"),
            ),
        ];
        for (writer, text, expected, written) in cases {
            let (source, actions) = offered(writer, text);
            let [(_, edit)] = actions.as_slice() else {
                panic!("one action for the other spelling, got {actions:?}");
            };
            assert_eq!(edit.new_text, expected, "{text}");
            if let Some(written) = written {
                assert_eq!(replaced(&source, edit), written, "{text}");
            }
        }
    }

    /// A path that escapes the site, the site root itself, and the writing file's own directory
    /// each name one thing rather than two.
    #[test]
    fn unambiguous_paths_offer_no_action() {
        for text in [
            "#import \"../../shared.typ\": shared\n",
            "#include \"/\"\n",
            "#import \"/content\": content\n",
            "#asset(\"/\", bytes)\n",
        ] {
            let (_, actions) = offered("content/post.typ", text);
            assert!(actions.is_empty(), "{text} offered {actions:?}");
        }
    }
}
