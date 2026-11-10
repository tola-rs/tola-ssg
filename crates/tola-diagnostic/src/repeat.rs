//! Group one diagnostic across the pages where it appeared.

use std::collections::{HashMap, hash_map::Entry};

use crate::{Diagnostic, DiagnosticCode};

/// Collapse the records that one source mistake produced, naming the pages each reached.
pub fn collapse_repeated<K: std::hash::Hash + Eq>(
    diagnostics: impl IntoIterator<Item = (K, Diagnostic, String)>,
) -> Vec<Diagnostic> {
    let mut positions: HashMap<K, usize> = HashMap::new();
    let mut groups: Vec<(Diagnostic, Vec<String>)> = Vec::new();
    for (key, diagnostic, page) in diagnostics {
        match positions.entry(key) {
            Entry::Occupied(position) => {
                let pages = &mut groups[*position.get()].1;
                if !pages.contains(&page) {
                    pages.push(page);
                }
            }
            Entry::Vacant(position) => {
                position.insert(groups.len());
                groups.push((diagnostic, vec![page]));
            }
        }
    }
    groups
        .into_iter()
        .map(|(diagnostic, pages)| match pages_note(&pages) {
            Some(note) => diagnostic.with_note(note),
            None => diagnostic,
        })
        .collect()
}

/// The identity of the source one record names, shared across pages.
#[derive(PartialEq, Eq, Hash)]
pub struct ReferenceIdentity {
    code: DiagnosticCode,
    path: Option<String>,
    line: Option<usize>,
    column: Option<usize>,
    message: String,
}

impl ReferenceIdentity {
    pub fn of(diagnostic: &Diagnostic) -> Self {
        let location = diagnostic.location.as_ref();
        Self {
            code: diagnostic.code,
            path: location.map(|location| location.path.clone()),
            line: location.and_then(|location| location.line),
            column: location.and_then(|location| location.column),
            message: diagnostic.message.clone(),
        }
    }
}

/// One sentence naming the pages a repeated record reached, counting beyond the first three.
fn pages_note(pages: &[String]) -> Option<String> {
    match pages {
        [] | [_] => None,
        pages => Some(format!(
            "it appears on {}",
            bounded_listing(pages, ("page", "pages"))
        )),
    }
}

/// How many names a listing names before it counts the rest.
pub const LISTED_NAMES: usize = 3;

/// Join names the way a sentence reads, naming [`LISTED_NAMES`] and counting the rest.
pub fn bounded_listing(names: &[String], unit: (&str, &str)) -> String {
    let mut quoted: Vec<_> = names
        .iter()
        .take(LISTED_NAMES)
        .map(|name| format!("`{name}`"))
        .collect();
    if names.len() > LISTED_NAMES {
        let remaining = names.len() - LISTED_NAMES;
        let noun = if remaining == 1 { unit.0 } else { unit.1 };
        quoted.push(format!("{remaining} more {noun}"));
    }
    let Some(last) = quoted.pop() else {
        return String::new();
    };
    if quoted.is_empty() {
        last
    } else {
        format!("{} and {last}", quoted.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Severity;

    #[test]
    fn lists_distinct_pages() {
        let diagnostic = Diagnostic::new(
            crate::codes::typst::COMPILE,
            Severity::Warning,
            "missing destination",
        );
        let grouped = collapse_repeated(
            ["index.html", "index.html", "about.html"]
                .map(|page| (0, diagnostic.clone(), page.to_owned())),
        );

        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped[0].notes.len(), 1);
        for page in ["index.html", "about.html"] {
            assert_eq!(grouped[0].notes[0].matches(page).count(), 1);
        }
    }
}
