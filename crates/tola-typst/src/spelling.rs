//! The declared name a written name nearly spells.

use std::cmp::Reverse;

/// The declared name `written` most likely meant, if one is within the typo distance.
///
/// A declared name suppresses the suggestion. The distance is optimal string alignment, so an
/// adjacent transposition costs one edit, and the allowed edits grow with the name's length.
///
/// A case-only correction wins first. Ties prefer an adjacent transposition, then the longer
/// shared prefix. Equally close distinct names leave the choice to the author.
pub fn closest<'a>(written: &str, declared: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let allowed = maximum_edits(written.chars().count());
    let mut best = None;
    let mut ambiguous = false;
    for candidate in declared {
        if candidate == written {
            return None;
        }
        let rank = if candidate.eq_ignore_ascii_case(written) {
            (0, false, Reverse(0))
        } else if let Some(distance) = osa_distance(written, candidate, allowed) {
            (
                distance,
                !is_adjacent_transposition(written, candidate),
                Reverse(common_prefix_length(written, candidate)),
            )
        } else {
            continue;
        };
        match best {
            Some((_, previous)) if rank > previous => {}
            Some((name, previous)) if rank == previous => ambiguous |= name != candidate,
            _ => {
                best = Some((candidate, rank));
                ambiguous = false;
            }
        }
    }
    best.filter(|_| !ambiguous).map(|(name, _)| name)
}

/// The edit distance a written name of `length` characters may have.
fn maximum_edits(length: usize) -> usize {
    match length {
        0 | 1 => 0,
        2..=5 => 1,
        _ => 2,
    }
}

/// The optimal string alignment distance between `left` and `right`, if it is at most `maximum`.
///
/// Adjacent transposition costs one edit, which plain Levenshtein distance would charge two.
fn osa_distance(left: &str, right: &str, maximum: usize) -> Option<usize> {
    let left_length = left.chars().count();
    let right_length = right.chars().count();
    if left_length.abs_diff(right_length) > maximum {
        return None;
    }
    if maximum == 0 {
        return (left == right).then_some(0);
    }
    // Row `row` is the distance from `left`'s first `row` characters to every prefix of `right`.
    // A transposition reads two rows back, so the three rows rotate in place.
    let mut previous_previous = vec![0; right_length + 1];
    let mut previous: Vec<usize> = (0..=right_length).collect();
    let mut current = vec![0; right_length + 1];
    let mut previous_left = None;
    for (row, left_character) in left.chars().enumerate() {
        current[0] = row + 1;
        let mut previous_right = None;
        for (column, right_character) in right.chars().enumerate() {
            let mut distance = (previous[column] + usize::from(left_character != right_character))
                .min(previous[column + 1] + 1)
                .min(current[column] + 1);
            if let (Some(previous_left), Some(previous_right)) = (previous_left, previous_right)
                && left_character == previous_right
                && previous_left == right_character
            {
                distance = distance.min(previous_previous[column - 1] + 1);
            }
            current[column + 1] = distance;
            previous_right = Some(right_character);
        }
        std::mem::swap(&mut previous_previous, &mut current);
        std::mem::swap(&mut previous, &mut previous_previous);
        previous_left = Some(left_character);
    }
    (previous[right_length] <= maximum).then_some(previous[right_length])
}

/// Whether `candidate` is `written` with one adjacent pair swapped.
///
/// A swap and a substitution both cost one edit under OSA; preferring the swap keeps `teh` from
/// correcting to `tea`.
fn is_adjacent_transposition(written: &str, candidate: &str) -> bool {
    let mut written = written.chars();
    let mut candidate = candidate.chars();
    let mut position = 0;
    let mut first = None;
    let mut second = None;
    loop {
        match (written.next(), candidate.next()) {
            (None, None) => break,
            (Some(left), Some(right)) if left == right => {}
            (Some(left), Some(right)) => {
                let difference = (position, left, right);
                if first.is_none() {
                    first = Some(difference);
                } else if second.is_none() {
                    second = Some(difference);
                } else {
                    return false;
                }
            }
            _ => return false,
        }
        position += 1;
    }
    matches!(
        (first, second),
        (
            Some((first_at, first_left, first_right)),
            Some((second_at, second_left, second_right))
        ) if second_at == first_at + 1
            && first_left == second_right
            && first_right == second_left
    )
}

/// The number of characters `left` and `right` share at their start.
fn common_prefix_length(left: &str, right: &str) -> usize {
    left.chars()
        .zip(right.chars())
        .take_while(|(left, right)| left == right)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn closest_of<'a>(written: &str, candidates: &'a [&'a str]) -> Option<&'a str> {
        closest(written, candidates.iter().copied())
    }

    #[test]
    fn near_miss_names_its_closest_candidate() {
        let candidates = ["site", "document", "address"];
        assert_eq!(closest_of("sitee", &candidates), Some("site"));
        assert_eq!(closest_of("documnt", &candidates), Some("document"));
        assert_eq!(closest_of("adress", &candidates), Some("address"));
        assert_eq!(closest_of("nonsense", &candidates), None);
        assert_eq!(
            closest_of("site", &candidates),
            None,
            "an exact name needs no fix"
        );

        // Two candidates inside the typo distance still choose the nearer one.
        let close = ["asset", "assert"];
        assert_eq!(closest_of("asert", &close), Some("assert"));
        assert_eq!(closest_of("nothing", &[]), None);
    }

    #[test]
    fn transposition_outranks_substitution() {
        assert_eq!(closest_of("teh", &["tea", "the"]), Some("the"));
        assert_eq!(
            closest_of("teh", &["the", "tea"]),
            Some("the"),
            "order does not decide"
        );
        assert_eq!(
            closest_of("recieve", &["relieve", "receive"]),
            Some("receive")
        );
    }

    #[test]
    fn short_names_are_not_corrected() {
        for (written, candidate) in [("x", "y"), ("ab", "cd"), ("", "a")] {
            assert_eq!(
                closest_of(written, &[candidate]),
                None,
                "{written:?} was corrected to {candidate:?}"
            );
        }
    }

    #[test]
    fn suggestions_require_one_best_spelling() {
        assert_eq!(closest_of("Site", &["site", "SITE"]), None);
        assert_eq!(closest_of("Title", &["title", "title"]), Some("title"));
        assert_eq!(closest_of("siet", &["site", "siet"]), None);
        assert_eq!(closest_of("cat", &["cab", "car"]), None);
    }
}
