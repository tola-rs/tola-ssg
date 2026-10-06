//! The `Related:` line a bundled package's documentation block may carry.
//!
//! The line is Tola's own extension of the shared `///` grammar: `tola-typst-syntax` parses
//! doc blocks without knowing it, `tola-packages` reads it out here, and `tola help` renders
//! the targets it names.

use tola_typst_syntax::docs::documentation_lines;

/// One target an export's `Related:` line names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelatedTarget {
    /// Another export of the same package, as the line spells it.
    Export(String),
    /// A bundled package, named without its `@tola/` namespace.
    Package(String),
}

/// The line one documentation block spells its related targets on. A block that writes it
/// anywhere outside a fenced example has that line read as targets instead of prose.
const RELATED_PREFIX: &str = "Related:";

/// The targets one documentation block's `Related:` lines name, and the block without those
/// lines.
pub(crate) struct RelatedTargets {
    /// The targets, in the order the block writes them.
    pub targets: Vec<RelatedTarget>,
    /// The block with its `Related:` lines removed, so the shared parser reads prose only.
    pub prose: String,
}

/// Read the `Related:` lines out of `block`.
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

/// The targets one `Related:` line spells: a bare name is an export of the same package, and
/// `@tola/<name>` a bundled package.
fn targets_of(line: &str) -> impl Iterator<Item = RelatedTarget> + '_ {
    line.split(|character: char| character == ',' || character.is_whitespace())
        .map(|token| token.trim_matches('`'))
        .filter(|token| !token.is_empty())
        .map(|token| match token.strip_prefix("@tola/") {
            Some(name) => RelatedTarget::Package(name.to_owned()),
            None => RelatedTarget::Export(token.to_owned()),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn related_line_reads_as_targets() {
        let read = read(
            "Summary.\n\nRelated: `first`, second, @tola/address\n\n- value (int): Meaning.\n-> int",
        );
        assert_eq!(
            read.targets,
            vec![
                RelatedTarget::Export("first".to_owned()),
                RelatedTarget::Export("second".to_owned()),
                RelatedTarget::Package("address".to_owned()),
            ]
        );
        assert!(!read.prose.contains("Related:"));
    }

    #[test]
    fn fenced_related_line_stays_prose() {
        let read = read("Summary.\n\n```typst\nRelated: parse\n```\n\nRelated: parse");
        assert_eq!(
            read.targets,
            vec![RelatedTarget::Export("parse".to_owned())]
        );
        assert!(read.prose.contains("Related: parse"));
    }
}
