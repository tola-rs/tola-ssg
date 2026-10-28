//! The phrases diagnostics and hovers write for a list of names.

/// At most `limit` names, comma-joined, and how many were left out.
pub(crate) fn listed<'a>(
    mut names: impl Iterator<Item = &'a str>,
    limit: usize,
) -> (String, usize) {
    let listed = names.by_ref().take(limit).collect::<Vec<_>>().join(", ");
    (listed, names.count())
}

/// A list of names as one English phrase.
pub(crate) fn joined(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [only] => only.clone(),
        [first, second] => format!("{first} and {second}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

/// The names a sentence lists, each as code, joined the way a sentence reads.
pub(crate) fn quoted(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [only] => format!("`{only}`"),
        [head @ .., last] => format!(
            "{} and `{last}`",
            head.iter()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listed_reports_the_omitted_count() {
        for (names, expected) in [
            (vec![], ("", 0)),
            (vec!["a"], ("a", 0)),
            (vec!["a", "b", "c"], ("a, b, c", 0)),
            (vec!["a", "b", "c", "d", "e"], ("a, b, c", 2)),
        ] {
            assert_eq!(
                listed(names.iter().copied(), 3),
                (expected.0.to_owned(), expected.1)
            );
        }
    }
}
