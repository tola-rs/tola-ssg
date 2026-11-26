//! Group one diagnostic across the pages where it appeared.

use std::collections::{HashMap, hash_map::Entry};

use crate::{Diagnostic, DiagnosticCode};

pub fn collapse_repeated<K: std::hash::Hash + Eq>(
    diags: impl IntoIterator<Item = (K, Diagnostic, String)>,
) -> Vec<Diagnostic> {
    let mut pos: HashMap<K, usize> = HashMap::new();
    let mut groups: Vec<(Diagnostic, Vec<String>)> = Vec::new();
    for (key, diag, page) in diags {
        match pos.entry(key) {
            Entry::Occupied(entry) => {
                let pages = &mut groups[*entry.get()].1;
                if !pages.contains(&page) {
                    pages.push(page);
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(groups.len());
                groups.push((diag, vec![page]));
            }
        }
    }
    groups
        .into_iter()
        .map(|(diag, pages)| match pages_note(&pages) {
            Some(note) => diag.with_note(note),
            None => diag,
        })
        .collect()
}

#[derive(PartialEq, Eq, Hash)]
pub struct ReferenceIdentity {
    code: DiagnosticCode,
    path: Option<String>,
    line: Option<usize>,
    column: Option<usize>,
    message: String,
}

impl ReferenceIdentity {
    pub fn of(diag: &Diagnostic) -> Self {
        let loc = diag.location.as_ref();
        Self {
            code: diag.code,
            path: loc.map(|l| l.path.clone()),
            line: loc.and_then(|l| l.line),
            column: loc.and_then(|l| l.column),
            message: diag.message.clone(),
        }
    }
}

fn pages_note(pages: &[String]) -> Option<String> {
    match pages {
        [] | [_] => None,
        pages => Some(format!(
            "it appears on {}",
            bounded_listing(pages, ("page", "pages"))
        )),
    }
}

pub const LISTED_NAMES: usize = 3;

pub fn bounded_listing(names: &[String], unit: (&str, &str)) -> String {
    let mut quoted: Vec<_> = names
        .iter()
        .take(LISTED_NAMES)
        .map(|name| format!("`{name}`"))
        .collect();
    if names.len() > LISTED_NAMES {
        let rem = names.len() - LISTED_NAMES;
        let noun = if rem == 1 { unit.0 } else { unit.1 };
        quoted.push(format!("{rem} more {noun}"));
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
    fn distinct_pages() {
        let diag = Diagnostic::new(
            crate::codes::typst::COMPILE,
            Severity::Warning,
            "missing destination",
        );
        let grouped = collapse_repeated(
            ["index.html", "index.html", "about.html"]
                .map(|page| (0, diag.clone(), page.to_owned())),
        );

        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped[0].notes.len(), 1);
        for page in ["index.html", "about.html"] {
            assert_eq!(grouped[0].notes[0].matches(page).count(), 1);
        }
    }
}
