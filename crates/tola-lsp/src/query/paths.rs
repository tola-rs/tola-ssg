//! The two spellings a site path may be written in, and the conversion between them.

use lsp_types::{Range, TextEdit};
use tola_typst::typst::foundations::PathOrStr;
use tola_typst::typst::syntax::{RootedPath, Source, VirtualPath, VirtualRoot, ast};
use tola_typst_syntax::names::SourceNames;

use crate::links::Link;
use crate::position;

/// The action that rewrites the path the source declares at `cursor` into the spelling the author
/// has not written. A path with only one spelling offers no action.
///
/// `link` is the link `crate::links` found the cursor in, which owns where a path sits; the file
/// behind the path comes from Typst's own resolution here.
pub(crate) fn rewrites(
    source: &Source,
    cursor: usize,
    link: Option<&Link>,
    names: &SourceNames,
) -> Vec<(String, TextEdit)> {
    // `/` names the site root, so a document inside a package has no site spelling to convert to.
    if !matches!(source.id().root(), VirtualRoot::Project) {
        return Vec::new();
    }
    // An asset argument names an output from the root, so no file answers either spelling.
    if let Some(argument) = tola_typst_syntax::syntax::path_argument(source, cursor)
        && argument.base == tola_typst_syntax::syntax::PathBase::Site
    {
        return site_argument(source, &argument, names)
            .into_iter()
            .collect();
    }
    let Some(link) = link else {
        return Vec::new();
    };
    let Some(written) = written(source, cursor) else {
        return Vec::new();
    };
    let Some(spelling) = other_spelling(source, &written) else {
        return Vec::new();
    };
    rewrite_action(&written, spelling, link.range)
        .into_iter()
        .collect()
}

/// The action that rewrites an asset argument into the spelling of the same output, absent when the
/// argument is the site root itself or a name the source declares.
fn site_argument(
    source: &Source,
    argument: &tola_typst_syntax::syntax::PathArgument,
    names: &SourceNames,
) -> Option<(String, TextEdit)> {
    let callee = argument
        .callee
        .as_ref()
        .expect("an asset path belongs to a call");
    if names.declared_at(callee.start).is_some() {
        return None;
    }
    let written = source
        .text()
        .get(argument.range.clone())
        .unwrap_or_default();
    let spelling = match written.strip_prefix('/') {
        Some(rest) => rest.to_owned(),
        None => format!("/{written}"),
    };
    // `/` names the site root, which is no output the site publishes: a path with one spelling
    // offers no action.
    if spelling.is_empty() {
        return None;
    }
    let range = position::utf16_range(source.lines(), argument.range.clone())?;
    rewrite_action(written, spelling, range)
}

/// The spelling of the file `written` names that the author has not written, when both spellings
/// reach that file.
fn other_spelling(source: &Source, written: &str) -> Option<String> {
    let target = named_file(source, written)?;
    let spelling = if written.starts_with('/') {
        relative_spelling(target.vpath(), source.id().vpath())?
    } else {
        target.vpath().get_with_slash().to_owned()
    };
    // The edit replaces the author's own path, so the other spelling must reach the same file.
    (named_file(source, &spelling)? == target).then_some(spelling)
}

/// The action that rewrites `written` into `spelling`, absent when the spelling cannot stand
/// between the quotation marks that enclose the path.
fn rewrite_action(written: &str, spelling: String, range: Range) -> Option<(String, TextEdit)> {
    no_escape_needed(&spelling).then(|| {
        (
            format!("replace `{written}` with `{spelling}`"),
            TextEdit {
                range,
                new_text: spelling,
            },
        )
    })
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
        let names = SourceNames::new(source.clone());
        let links = crate::links::declared(&source);
        let actions = rewrites(
            &source,
            cursor,
            crate::links::holding(&links, cursor),
            &names,
        );
        (source, actions)
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
            "#let asset(path, body) = path\n#asset(\"/downloads/file.pdf\", bytes)\n",
        ] {
            let (_, actions) = offered("content/post.typ", text);
            assert!(actions.is_empty(), "{text} offered {actions:?}");
        }
    }
}
