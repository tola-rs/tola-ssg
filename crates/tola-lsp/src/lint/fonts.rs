//! Font families the checked environment does not include, as the source's `text(...)` calls name
//! them.

use tola_typst::TypstWorld;
use tola_typst::typst::World;
use tola_typst::typst::syntax::LinkedNode;
use tola_typst::typst::syntax::ast::{self, AstNode};
use tola_typst_syntax::names::{OccurrenceKind, SourceNames};

use crate::codes;

use super::Hint;
use super::imports::could_supply_unknown;

/// Warn about each font family a `text(...)` call names that this environment does not include.
pub(super) fn warn_missing_families(
    names: &SourceNames,
    world: Option<&TypstWorld>,
    hints: &mut Vec<Hint>,
) {
    // A font name is a runtime lookup this environment either satisfies or does not, so the book
    // of the world the compiler read is the one authority: a name it does not include falls back.
    let Some(world) = world else {
        return;
    };
    let mut installed: Option<Vec<&str>> = None;
    for (name, span) in named_families(names) {
        if world.book().contains_family(&name.to_lowercase()) {
            continue;
        }
        let installed = installed
            .get_or_insert_with(|| world.book().families().map(|(family, _)| family).collect());
        hints.push(Hint {
            code: codes::editor::UNKNOWN_FONT,
            message: format!("the font `{name}` is not available"),
            note: Some("the text falls back to another font".to_owned()),
            help: help(&name, installed),
            span,
            cause: None,
        });
    }
}

/// The font families one source's `text(...)` calls name, with each literal's own range.
///
/// Both spellings count: a call, and a set rule whose target is the same call. A `text` this
/// source binds itself, or an import this index cannot enumerate may bind, is not the builtin this
/// pass reads. Only string literals are read — a family name an expression computes is a value
/// this pass cannot decide.
fn named_families(names: &SourceNames) -> Vec<(String, std::ops::Range<usize>)> {
    let mut fonts = Vec::new();
    let mut stack = vec![LinkedNode::new(names.source().root())];
    while let Some(node) = stack.pop() {
        if let Some(call) = node.cast::<ast::FuncCall>()
            && let Some(callee) = node.find(call.callee().span())
            && binds_builtin_text(names, &callee)
        {
            arguments(&node, call.args(), &mut fonts);
        }
        if let Some(set) = node.cast::<ast::SetRule>()
            && let Some(target) = node.find(set.target().span())
            && binds_builtin_text(names, &target)
        {
            arguments(&node, set.args(), &mut fonts);
        }
        for child in node.children() {
            stack.push(child);
        }
    }
    fonts.sort_by_key(|(_, span)| span.start);
    fonts
}

/// Whether the node is a `text` spelling this source cannot bind elsewhere.
fn binds_builtin_text(names: &SourceNames, node: &LinkedNode<'_>) -> bool {
    let Some(expr) = node.cast::<ast::Expr>() else {
        return false;
    };
    if !matches!(expr, ast::Expr::Ident(ident) if ident.get().as_str() == "text") {
        return false;
    }
    let range = node.range();
    let Some(occurrence) = names.occurrence(range.start) else {
        return false;
    };
    if occurrence.range != range || !matches!(occurrence.kind, OccurrenceKind::Name) {
        return false;
    }
    names
        .resolve("text", occurrence.scope, range.start)
        .is_none()
        && !could_supply_unknown(names, occurrence.scope, range.start)
}

/// The font literals one argument list names under `font:`.
fn arguments(
    node: &LinkedNode<'_>,
    args: ast::Args<'_>,
    names: &mut Vec<(String, std::ops::Range<usize>)>,
) {
    for argument in args.items() {
        let ast::Arg::Named(named) = argument else {
            continue;
        };
        if named.name().get().as_str() != "font" {
            continue;
        }
        let Some(argument) = node.find(named.expr().span()) else {
            continue;
        };
        family_literals(&argument, names);
    }
}

/// The string literals one `font:` argument names, with each literal's own range.
///
/// The argument is a family name, an array of them, or a font descriptor whose `name` field
/// has the family; anything else is a value this pass cannot decide.
fn family_literals(node: &LinkedNode<'_>, names: &mut Vec<(String, std::ops::Range<usize>)>) {
    if let Some(text) = node.cast::<ast::Str>() {
        names.push((text.get().to_string(), node.range()));
        return;
    }
    if let Some(array) = node.cast::<ast::Array>() {
        for item in array.items() {
            if let Some(child) = node.find(item.span()) {
                family_literals(&child, names);
            }
        }
        return;
    }
    if let Some(dict) = node.cast::<ast::Dict>() {
        for item in dict.items() {
            if let ast::DictItem::Named(field) = item
                && field.name().get().as_str() == "name"
                && let Some(child) = node.find(field.expr().span())
            {
                family_literals(&child, names);
            }
        }
    }
}

/// What an author can do about a font this environment does not include.
fn help(name: &str, families: &[&str]) -> Option<String> {
    if let Some((_, english)) = LOCALIZED_FONTS
        .iter()
        .find(|(localized, _)| *localized == name)
        && let Some(installed) = english.iter().find(|english| families.contains(english))
    {
        return Some(format!("this font is installed as `{installed}`"));
    }
    if let Some(closest) = crate::nearest::closest(name, families.iter().copied()) {
        return Some(format!("did you mean `{closest}`?"));
    }
    if name.contains(',') {
        return Some(
            "write each font as its own string, as in `(\"Times New Roman\", \"Arial\")`"
                .to_owned(),
        );
    }
    if !name.is_ascii() {
        return Some("use the font's English (PostScript) name".to_owned());
    }
    Some("check the name, or add the font through `typst.fonts.paths`".to_owned())
}

/// The English names a localized font name is usually installed under.
///
/// The table tinymist's lint has (Apache-2.0; see `licenses/README.md`), so an author
/// writing a CJK name reaches the name a font book actually holds.
const LOCALIZED_FONTS: &[(&str, &[&str])] = &[
    // Chinese Simplified
    ("宋体", &["SimSun"]),
    ("新宋体", &["NSimSun"]),
    ("黑体", &["SimHei"]),
    ("微软雅黑", &["Microsoft YaHei"]),
    ("楷体", &["KaiTi", "KaiTi_GB2312"]),
    ("仿宋", &["FangSong", "FangSong_GB2312"]),
    ("等线", &["DengXian"]),
    ("幼圆", &["YouYuan"]),
    ("华文宋体", &["STSong"]),
    ("华文楷体", &["STKaiti"]),
    ("华文仿宋", &["STFangsong"]),
    ("华文黑体", &["STHeiti"]),
    ("华文细黑", &["STXihei"]),
    // Chinese Traditional
    ("新細明體", &["PMingLiU"]),
    ("細明體", &["MingLiU"]),
    ("標楷體", &["DFKai-SB"]),
    ("微軟正黑體", &["Microsoft JhengHei"]),
    // Japanese
    ("ＭＳ 明朝", &["MS Mincho"]),
    ("ＭＳ Ｐ明朝", &["MS PMincho"]),
    ("ＭＳ ゴシック", &["MS Gothic"]),
    ("ＭＳ Ｐゴシック", &["MS PGothic"]),
    ("メイリオ", &["Meiryo"]),
    // Korean
    ("맑은 고딕", &["Malgun Gothic"]),
    ("바탕", &["Batang"]),
    ("돋움", &["Dotum"]),
    ("궁서", &["Gungsuh"]),
    ("굴림", &["Gulim"]),
];

#[cfg(test)]
mod tests {
    use super::*;
    use tola_typst::typst::syntax::Source;

    #[test]
    fn font_arguments_name_their_literals() {
        let names = SourceNames::new(Source::detached(
            "#text(font: \"Inter\")[a]\n#set text(font: (\"A\", \"B\"))\n#text(font: (name: \"C\", weight: \"bold\"))[b]\n",
        ));
        let fonts = named_families(&names);
        assert_eq!(
            fonts
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            ["Inter", "A", "B", "C"]
        );
    }
    #[test]
    fn font_help_names_the_closest_available_family() {
        assert_eq!(
            help("Inetr", &["Inter", "Arial"]).as_deref(),
            Some("did you mean `Inter`?")
        );
        assert_eq!(
            help("宋体", &["SimSun"]).as_deref(),
            Some("this font is installed as `SimSun`")
        );
    }

    #[test]
    fn shadowed_text_call_names_no_fonts() {
        let shadowed = SourceNames::new(Source::detached(
            "#let text(font) = [local]\n#text(font: \"Absent\")\n".to_owned(),
        ));
        assert!(named_families(&shadowed).is_empty());
        let builtin = SourceNames::new(Source::detached("#text(font: \"Absent\")\n".to_owned()));
        assert_eq!(named_families(&builtin).len(), 1);
    }
}
