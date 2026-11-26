//! Parses `Related:` lines from bundled package `///` documentation blocks.

use tola_typst_syntax::docs::documentation_lines;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelatedTarget {
    Export(String),
    Package(String),
}

const RELATED_PREFIX: &str = "Related:";

pub(crate) struct RelatedTargets {
    pub targets: Vec<RelatedTarget>,
    pub prose: String,
}

pub(crate) fn read(block: &str) -> RelatedTargets {
    let mut targets = Vec::new();
    let mut lines = Vec::new();
    for line in documentation_lines(block) {
        let text = line.text.trim_end();
        if !line.is_fenced
            && let Some(spelled) = text.strip_prefix(RELATED_PREFIX)
        {
            targets.extend(targets_of(spelled));
            continue;
        }
        lines.push(line.text);
    }
    RelatedTargets {
        targets,
        prose: lines.join("\n"),
    }
}

fn targets_of(line: &str) -> impl Iterator<Item = RelatedTarget> + '_ {
    line.split(|c: char| c == ',' || c.is_whitespace())
        .map(|tok| tok.trim_matches('`'))
        .filter(|tok| !tok.is_empty())
        .map(|tok| match tok.strip_prefix("@tola/") {
            Some(name) => RelatedTarget::Package(name.to_owned()),
            None => RelatedTarget::Export(tok.to_owned()),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn related_targets_parsed() {
        let res = read(
            "Summary.\n\nRelated: `first`, second, @tola/address\n\n- value (int): Meaning.\n-> int",
        );
        assert_eq!(
            res.targets,
            vec![
                RelatedTarget::Export("first".to_owned()),
                RelatedTarget::Export("second".to_owned()),
                RelatedTarget::Package("address".to_owned()),
            ]
        );
        assert!(!res.prose.contains("Related:"));
    }

    #[test]
    fn related_fenced_ignored() {
        let res = read("Summary.\n\n```typst\nRelated: parse\n```\n\nRelated: parse");
        assert_eq!(res.targets, vec![RelatedTarget::Export("parse".to_owned())]);
        assert!(res.prose.contains("Related: parse"));
    }
}
