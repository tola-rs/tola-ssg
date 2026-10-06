//! The declared name a written name nearly spells.
//!
//! A quick fix offers the closest declared name only when it is close enough to be a typo of it:
//! beyond that distance the author wrote something else, and rewriting it would be a guess.

/// The closest of `candidates` to `name`, if any is within the distance of a typo.
pub(crate) fn closest<'a>(
    name: &str,
    candidates: impl Iterator<Item = &'a str>,
) -> Option<&'a str> {
    candidates
        .filter(|candidate| *candidate != name)
        .map(|candidate| (distance(name, candidate), candidate))
        .filter(|(distance, _)| *distance <= MAXIMUM_DISTANCE)
        .min_by(|(left, left_name), (right, right_name)| {
            left.cmp(right).then_with(|| left_name.cmp(right_name))
        })
        .map(|(_, candidate)| candidate)
}

/// The edit distance beyond which two names are unrelated rather than misspelled.
const MAXIMUM_DISTANCE: usize = 2;

fn distance(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    if left.len().abs_diff(right.len()) > MAXIMUM_DISTANCE {
        return usize::MAX;
    }
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0; right.len() + 1];
    for (row, left) in left.iter().enumerate() {
        current[0] = row + 1;
        for (column, right) in right.iter().enumerate() {
            current[column + 1] = (previous[column] + usize::from(left != right))
                .min(previous[column + 1] + 1)
                .min(current[column] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn near_miss_names_its_closest_candidate() {
        let candidates = ["site", "document", "address"];
        let names = || candidates.into_iter();
        assert_eq!(closest("sitee", names()), Some("site"));
        assert_eq!(closest("documnt", names()), Some("document"));
        assert_eq!(closest("adress", names()), Some("address"));
        assert_eq!(closest("nonsense", names()), None);
        assert_eq!(closest("site", names()), None, "an exact name needs no fix");

        // Two candidates inside the typo distance still choose the nearer one.
        let close = ["asset", "assert"];
        let names = || close.into_iter();
        assert_eq!(closest("asert", names()), Some("assert"));
    }
}
