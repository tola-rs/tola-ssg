//! Task-focused authoring guidance, read without loading a site.

use std::ops::Range;

use anyhow::{Result, anyhow};
use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};
use tola_diagnostic::{Diagnostic, DiagnosticError, Severity};

use crate::cancellation::Cancellation;
use crate::cli::args::SkillArgs;
use crate::cli::log::{LogFile, destination};
use crate::cli::output::CommandOutput;
use crate::help::model::anchor;
use crate::terminal::display_path_as_given;
use crate::writes::FileWrites;

const SKILL: &str = include_str!("../../../TOLA-SKILL.md");

struct Section {
    title: String,
    path: Vec<String>,
    range: Range<usize>,
    own_end: usize,
}

pub(in crate::cli) fn run_builtin(
    args: &SkillArgs,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    run(SKILL, "tola skill", args, output, cancellation)
}

pub(in crate::cli) fn run(
    source: &str,
    command: &str,
    args: &SkillArgs,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    let cancellation = cancellation.token();
    cancellation.ensure_active()?;
    let markdown = requested(source, command, args)?;
    let Some(path) = &args.output else {
        destination::start(output, &cancellation)?;
        return output.write_stdout(markdown);
    };

    let destination = std::env::current_dir()?.join(path);
    let name = destination.file_name().ok_or_else(|| {
        anyhow!("choose an output filename, such as `tola skill --all -o guide.md`")
    })?;
    let mut writes = FileWrites::new(destination.parent().expect("a filename has a parent"))?;
    writes.create_file(name, markdown)?;
    destination::check_writes(output.log().map(LogFile::path), &writes, false)?;
    writes.check()?;
    destination::start(output, &cancellation)?;
    writes.apply(&cancellation)?;
    output.status(format!(
        "Wrote {} to {}",
        command,
        display_path_as_given(path)
    ))?;
    Ok(())
}

fn requested(source: &str, command: &str, args: &SkillArgs) -> Result<String> {
    let sections = sections(source);
    let path: Vec<String> = args
        .sections
        .iter()
        .map(|part| anchor(part).as_str().to_owned())
        .collect();
    let (start, end, own_end) = if path.is_empty() {
        (
            0,
            source.len(),
            sections
                .first()
                .map_or(source.len(), |section| section.range.start),
        )
    } else {
        let section = sections
            .iter()
            .find(|section| section.path == path)
            .ok_or_else(|| {
                let message = format!("skill section `{}` does not exist", args.sections.join(" "));
                let diagnostic = Diagnostic::new(
                    crate::codes::skill::SECTION,
                    Severity::Error,
                    message.clone(),
                )
                .with_help(format!("Run {command} --list and choose a listed section"));
                DiagnosticError::new(message, vec![diagnostic])
            })?;
        (section.range.start, section.range.end, section.own_end)
    };
    if args.all {
        return Ok(source[start..end].to_owned());
    }
    let mut markdown = if args.list {
        String::new()
    } else {
        source[start..own_end].trim_end().to_owned() + "\n"
    };
    let mut first = true;
    for section in &sections {
        let depth = section.path.len();
        if !section.path.starts_with(&path)
            || depth <= path.len()
            || (!args.list && depth != path.len() + 1)
        {
            continue;
        }
        if first && !args.list {
            markdown.push('\n');
        }
        first = false;
        let indent = "  ".repeat(depth - path.len() - 1);
        markdown.push_str(&format!(
            "{indent}- `{command} {}` — {}\n",
            section.path.join(" "),
            section.title,
        ));
    }
    Ok(markdown)
}

fn sections(markdown: &str) -> Vec<Section> {
    let mut headings = Vec::new();
    let mut events = Parser::new(markdown).into_offset_iter();
    let mut depth = 0;
    while let Some((event, range)) = events.next() {
        match event {
            Event::Start(Tag::Heading { level, .. }) if depth == 0 => {
                let mut title = String::new();
                for (event, _) in events.by_ref() {
                    match event {
                        Event::End(TagEnd::Heading(_)) => break,
                        Event::Text(text) | Event::Code(text) => title.push_str(&text),
                        Event::SoftBreak | Event::HardBreak => title.push(' '),
                        _ => {}
                    }
                }
                if level != HeadingLevel::H1 {
                    headings.push((level, range.start, title));
                }
            }
            Event::Start(_) => depth += 1,
            Event::End(_) => depth -= 1,
            _ => {}
        }
    }
    let mut parents: Vec<(HeadingLevel, String)> = Vec::new();
    headings
        .iter()
        .enumerate()
        .map(|(index, (level, start, title))| {
            while parents.last().is_some_and(|(parent, _)| parent >= level) {
                parents.pop();
            }
            parents.push((*level, anchor(title).as_str().to_owned()));
            let following = &headings[index + 1..];
            let end = following
                .iter()
                .find(|(next, _, _)| next <= level)
                .map_or(markdown.len(), |(_, start, _)| *start);
            Section {
                title: title.clone(),
                path: parents.iter().map(|(_, name)| name.clone()).collect(),
                range: *start..end,
                own_end: following.first().map_or(end, |(_, start, _)| *start),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;
    use std::collections::BTreeSet;

    #[test]
    fn section_boundaries_follow_headings() {
        let md = "# Guide\nIntro\n## Math\nCommon\n### Inline\n$x$\n```md\n## Not a section\n```\n> ## Quoted\n\n### Display\n$$\n## Links\nSee\n";
        let secs = sections(md);
        assert_eq!(
            secs.iter().map(|s| s.path.join(" ")).collect::<Vec<_>>(),
            ["math", "math inline", "math display", "links"]
        );
        assert_eq!(
            &md[secs[0].range.start..secs[0].own_end],
            "## Math\nCommon\n"
        );
        assert!(md[secs[0].range.clone()].contains("### Display"));
        assert!(!md[secs[0].range.clone()].contains("## Links"));
        assert!(md[secs[1].range.clone()].contains("## Not a section"));
    }

    #[test]
    fn heading_paths_scope_duplicates() {
        let md = "# Guide\nFirst\n-----\n### *Examples*\nOne\n## Second\n### Examples\nTwo\n";
        let secs = sections(md);
        assert_eq!(secs[1].path, ["first", "examples"]);
        assert_eq!(secs[3].path, ["second", "examples"]);
        assert!(md[secs[1].range.clone()].contains("One"));
        assert!(!md[secs[1].range.clone()].contains("Two"));
    }

    #[test]
    fn bundled_sections_addressable() {
        let mut paths = BTreeSet::new();
        for sec in sections(SKILL) {
            assert!(sec.path.iter().all(|part| !part.is_empty()));
            assert!(paths.insert(sec.path.clone()), "{:?}", sec.path);
            let mut argv = vec!["tola".to_owned(), "skill".to_owned()];
            argv.extend(sec.path);
            let crate::cli::Commands::Skill { args, .. } =
                crate::cli::Cli::try_parse_from(argv).unwrap().command
            else {
                panic!("expected skill request");
            };
            assert!(!requested(SKILL, "tola skill", &args).unwrap().is_empty());
        }
    }

    #[test]
    fn selection_limits_content() {
        let src = "# Guide\nCore\n## Math\nCommon\n### Rendering\nBrowser\n#### SVG\nArtwork\n## Links\nTargets\n";
        let args = SkillArgs {
            sections: vec!["math".into(), "rendering".into()],
            ..Default::default()
        };
        let overview = requested(src, "tola skill", &args).unwrap();
        assert!(overview.contains("Browser"));
        assert!(!overview.contains("Artwork"));
        let full = requested(
            src,
            "tola skill",
            &SkillArgs {
                all: true,
                ..args.clone()
            },
        )
        .unwrap();
        assert!(full.contains("Artwork"));
        assert!(!full.contains("Targets"));
        let idx = requested(src, "tola skill", &SkillArgs { list: true, ..args }).unwrap();
        assert!(idx.contains("rendering"));
        assert!(!idx.contains("Browser"));
        let root = requested(src, "tola skill", &SkillArgs::default()).unwrap();
        assert!(root.contains("Core"));
        assert!(root.contains("math"));
        assert!(!root.contains("Common"));
        assert!(
            requested(
                src,
                "tola skill",
                &SkillArgs {
                    sections: vec!["math".into(), "missing".into()],
                    ..Default::default()
                }
            )
            .is_err()
        );
    }
}
