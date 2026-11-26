use super::*;
use tola_build::config::FieldPath;

struct Completed {
    label: String,
    key: String,
    range: lsp_types::Range,
}

fn completed_in(host: HostSections, text: &str) -> Vec<Completed> {
    let offset = text.find('|').expect("a cursor marker");
    let lines = Lines::new(text.replace('|', ""));
    completion(&lines, offset, host)
        .into_iter()
        .map(|item| {
            let CompletionTextEdit::Edit(edit) = item.text_edit.unwrap() else {
                panic!("expected a text edit");
            };
            Completed {
                label: item.label,
                key: edit.new_text,
                range: edit.range,
            }
        })
        .collect()
}

fn completed(text: &str) -> Vec<Completed> {
    completed_in(&[], text)
}

fn labels(text: &str) -> Vec<String> {
    completed(text).into_iter().map(|i| i.label).collect()
}

fn labels_in(host: HostSections, text: &str) -> Vec<String> {
    completed_in(host, text)
        .into_iter()
        .map(|i| i.label)
        .collect()
}

fn hover_in(host: HostSections, text: &str) -> Option<String> {
    let offset = text.find('|').expect("a cursor marker");
    let unmarked = text.replace('|', "");
    let lines = Lines::new(unmarked);
    let pos = position::utf16_range(&lines, offset..offset)?.start;
    let hover = document_hover(lines.text(), pos, host)?;
    match hover.contents {
        lsp_types::HoverContents::Markup(markup) => Some(markup.value),
        contents => panic!("markdown, got {contents:?}"),
    }
}

fn hover(text: &str) -> Option<String> {
    hover_in(&[], text)
}

fn documented_path(hover: &str) -> (&str, &str) {
    let (head, body) = hover
        .split_once("\n\n")
        .unwrap_or_else(|| panic!("a path line and its documentation: {hover:?}"));
    let path = head
        .strip_prefix('`')
        .and_then(|head| head.strip_suffix('`'))
        .unwrap_or_else(|| panic!("the path as code: {hover:?}"));
    assert!(!body.trim().is_empty(), "documentation: {hover:?}");
    (path, body)
}

const HOST: HostSections = &[&[
    (
        FieldPath::new("diagnostics.max_errors"),
        Some("Host-declared errors limit."),
    ),
    (FieldPath::new("build.entry"), Some("Host-declared entry.")),
]];

#[test]
fn host_field_hover_documentation() {
    let answer = hover_in(HOST, "[diagnostics]\nmax_erro|rs = 3\n").expect("host hover");
    let (path, docs) = documented_path(&answer);
    assert_eq!(path, "diagnostics.max_errors");
    assert_eq!(docs, "Host-declared errors limit.");
}

#[test]
fn core_documentation_precedence() {
    let answer = hover_in(HOST, "[build]\nent|ry = \"site.typ\"\n").expect("a hover");
    let (path, docs) = documented_path(&answer);
    assert_eq!(path, "build.entry");
    assert!(!docs.contains("Host-declared"), "core wins: {answer}");
}

#[test]
fn host_keys_complete_in_section() {
    let offered = labels_in(HOST, "[diagnostics]\n|");
    assert!(offered.contains(&"max_errors".to_owned()), "{offered:?}");
}

#[test]
fn boolean_value_completion() {
    assert_eq!(labels("[build.minify]\njavascript = tr|\n"), ["true"]);
}

#[test]
fn value_hover_resolves_field() {
    assert_eq!(
        documented_path(&hover("[site]\nlanguage = \"z|h\"\n").expect("a hover")).0,
        "site.language"
    );
}

fn fix(text: &str) -> Option<TextEdit> {
    let offset = text.find('|').expect("a cursor marker");
    let unmarked = text.replace('|', "");
    let lines = Lines::new(unmarked.clone());
    let pos = position::utf16_range(&lines, offset..offset)?.start;
    replacement(&unmarked, pos, &[])
}

#[test]
fn misspelled_key_fix() {
    let edit = fix("[build]\nentr| = \"content\"\n").expect("a fix");
    assert_eq!(edit.new_text, "entry");
    assert_eq!(
        (
            edit.range.start.line,
            edit.range.start.character,
            edit.range.end.character
        ),
        (1, 0, 4),
    );
}

#[test]
fn dotted_key_fix_avoids_collision() {
    let edit = fix("site.tite|l = \"Site\"\n").expect("a dotted key has a fix");
    assert_eq!(edit.new_text, "title");
    assert_eq!(edit.range.start.character, 5);
    assert!(fix("[site]\ntitle = \"Keep\"\ntite|l = \"Typo\"\n").is_none());
}

#[test]
fn key_fix_rejects_valid_and_unrelated() {
    assert!(fix("[build]\nent|ry = \"content\"\n").is_none());
    assert!(fix("[build]\nentry = \"cont|ent\"\n").is_none());
    assert!(fix("[bu|ild]\n").is_none());
    assert_eq!(
        fix("[bul|d]\n").map(|e| e.new_text),
        Some("build".to_owned())
    );
    assert!(fix("[site]\nzzzzzz|zz = 1\n").is_none());
}

#[test]
fn table_scoped_key_completion() {
    let offered = labels("[build]\n|");
    assert!(offered.contains(&"entry".to_owned()), "{offered:?}");
    assert!(offered.contains(&"content-dir".to_owned()), "{offered:?}");
    assert!(!offered.contains(&"title".to_owned()), "{offered:?}");
    assert_eq!(labels("[build]\nent|"), ["entry"]);
}

#[test]
fn nested_section_completion() {
    let offered = labels("[hooks]\n|");
    assert!(offered.contains(&"before-build".to_owned()), "{offered:?}");
    assert!(!offered.contains(&"entry".to_owned()), "{offered:?}");
    assert!(!offered.contains(&"site".to_owned()), "{offered:?}");
    assert!(offered.iter().all(|k| !k.contains('.')), "{offered:?}");

    let top = labels("|");
    for section in ["site", "build"] {
        assert!(top.iter().any(|k| k == section), "{top:?}");
    }
    assert!(top.iter().all(|k| !k.contains('.')), "{top:?}");
    assert!(
        !top.iter().any(|k| matches!(k.as_str(), "title" | "entry")),
        "{top:?}"
    );
}

#[test]
fn written_keys_excluded() {
    let offered = labels("[site]\ntitle = \"Title\"\n|");
    assert!(!offered.contains(&"title".to_owned()), "{offered:?}");
    assert!(offered.contains(&"description".to_owned()), "{offered:?}");
}

#[test]
fn non_key_positions_no_key_completion() {
    for text in [
        "[build]\nentry = |\n",
        "version = |\n",
        "[bui|]\n",
        "# |\n",
        "[build]\nentry = \"|\"\n",
    ] {
        assert!(labels(text).is_empty(), "{text:?}");
    }
}

#[test]
fn config_hover_paths() {
    let cases: [(&str, &str); 11] = [
        ("[build]\nentr|y = \"content\"\n", "build.entry"),
        ("[hoo|ks]\n", "hooks"),
        ("[site]\nti|tle = \"Title\"\n", "site.title"),
        ("[build]\nminify = { ht|ml = true }\n", "build.minify.html"),
        ("[build]\nminify = { html = tr|ue }\n", "build.minify.html"),
        ("site.ti|tle = \"T\"\n", "site.title"),
        (
            "[build.references]\nnaviga|tion = \"warn\"\n",
            "build.references.navigation",
        ),
        (
            "[[hooks.before-build]]\nna|me = \"icons\"\n",
            "hooks.before-build.name",
        ),
        ("[si|te]\n", "site"),
        ("[[hooks.before-|build]]\n", "hooks.before-build"),
        (
            "[[hooks.before-build]]\nname = \"icons\"\nargv = [\n  \"node\",\n  \"|scripts/icons.mjs\",\n]\n",
            "hooks.before-build.argv",
        ),
    ];
    for (src, path) in cases {
        let answer = hover(src).unwrap_or_else(|| panic!("a hover for {src:?}"));
        assert_eq!(documented_path(&answer).0, path, "{src:?}");
    }
}

#[test]
fn array_element_hover_field() {
    assert_eq!(
        documented_path(
            &hover("[[hooks.before-build]]\nargv = [\n  [\"s|ite\"],\n]\n").expect("a hover")
        )
        .0,
        "hooks.before-build.argv"
    );
}

#[test]
fn undeclared_config_no_hover() {
    for src in [
        "|nope = 1\n",
        "[build]\nnope| = 1\n",
        "[bui|]\n",
        "[build]\n|",
        "# |note\n",
    ] {
        assert_eq!(hover(src), None, "{src:?}");
    }
}

#[test]
fn completion_edit_range_covers_prefix() {
    let completed = completed("[build]\nent|");
    let [item] = &completed[..] else {
        panic!("expected one completion");
    };
    assert_eq!(item.key, "entry");
    assert_eq!(item.range.start.line, 1);
    assert_eq!(item.range.start.character, 0);
    assert_eq!(item.range.end.character, 3);
}

#[test]
fn hook_stage_fields_and_dev_values() {
    for (stage, own, foreign, dev) in [
        (
            "before-build",
            "generates",
            "outputs",
            vec!["\"run\"", "\"skip\"", "\"reuse\""],
        ),
        (
            "generate-outputs",
            "outputs",
            "generates",
            vec!["\"run\"", "\"skip\"", "\"reuse\""],
        ),
        (
            "after-publish",
            "name",
            "generates",
            vec!["\"run\"", "\"skip\""],
        ),
    ] {
        let offered = labels(&format!("[[hooks.{stage}]]\n|"));
        assert!(offered.contains(&own.to_owned()), "{stage}: {offered:?}");
        assert!(
            !offered.contains(&foreign.to_owned()),
            "{stage}: {offered:?}"
        );
        assert_eq!(labels(&format!("[[hooks.{stage}]]\ndev = |")), dev);
    }
}

#[test]
fn interleaved_hook_entry_completion() {
    for (src, expected) in [
        (
            r#"[build]
entry = "site.typ"
[[hooks.before-build]]
name = "icons"
[site]
title = "Example"
[[hooks.after-publish]]
name = "deploy"
[[hooks.before-build]]
gen|
"#,
            ["generates"],
        ),
        (
            r#"[[hooks.before-build]]
name = "icons"
argv = ["icons"]
[[hooks.before-build]]
na|
"#,
            ["name"],
        ),
    ] {
        assert_eq!(labels(src), expected);
    }
}
