//! Feature dependencies and selection changes.

use super::features::{Feature, FeatureSet, SlotId, canon, providers};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SelectionIssue {
    pub(super) feature: Feature,
    pub(super) slot: SlotId,
}

pub(super) fn selection_issues(set: &FeatureSet) -> Vec<SelectionIssue> {
    let canonical = canon(set);
    canonical
        .iter()
        .flat_map(|feature| {
            unfilled_slots(feature, &canonical)
                .into_iter()
                .map(move |slot| SelectionIssue { feature, slot })
        })
        .collect()
}

/// Interactive selections include their dependencies; explicit CLI selections are validated as given.
pub(super) fn complete(set: &FeatureSet) -> FeatureSet {
    let mut selected = canon(set);
    loop {
        let Some(slot) = selected
            .iter()
            .find_map(|feature| unfilled_slots(feature, &selected).first().copied())
        else {
            return selected;
        };
        let provider = providers(slot)
            .iter()
            .copied()
            .find(|provider| super::features::replacement(&selected, *provider).is_none())
            .expect("required slots have an available provider");
        selected.insert(provider);
        selected = canon(&selected);
    }
}

pub(super) fn select(set: &FeatureSet, feature: Feature) -> FeatureSet {
    let mut selected = set.clone();
    selected.insert(feature);
    complete(&selected)
}

pub(super) fn unselect(set: &FeatureSet, feature: Feature) -> FeatureSet {
    let mut selected = canon(set);
    selected.remove(feature);
    loop {
        let unsatisfied = selection_issues(&selected);
        if unsatisfied.is_empty() {
            return selected;
        }
        for issue in unsatisfied {
            selected.remove(issue.feature);
        }
    }
}

pub(super) fn unfilled_slots(feature: Feature, set: &FeatureSet) -> Vec<SlotId> {
    let required: &[SlotId] = match feature {
        Feature::TailwindCss | Feature::Pagefind => &[SlotId::Runner],
        Feature::StarterStylesheet
        | Feature::Canonical
        | Feature::Feed
        | Feature::Sitemap
        | Feature::OpenGraph
        | Feature::TwitterCard
        | Feature::DenoToolchain => &[],
    };
    required
        .iter()
        .copied()
        .filter(|slot| {
            !providers(*slot)
                .iter()
                .any(|provider| set.contains(*provider))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::commands::init::features::{self, PRESETS};

    fn set(features: &[Feature]) -> FeatureSet {
        FeatureSet::new(features.iter().copied())
    }

    fn subsets() -> impl Iterator<Item = FeatureSet> {
        let features = features::selectable_features().collect::<Vec<_>>();
        (0..(1u32 << features.len())).map(move |mask| {
            FeatureSet::new(
                features
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| mask & (1 << index) != 0)
                    .map(|(_, feature)| *feature),
            )
        })
    }

    #[test]
    fn presets_satisfy_dependencies() {
        for preset in PRESETS {
            assert!(
                selection_issues(&set(preset.features)).is_empty(),
                "{}",
                preset.name
            );
        }
    }

    #[test]
    fn tools_require_deno() {
        for selected in subsets() {
            let expected = [Feature::Pagefind, Feature::TailwindCss]
                .into_iter()
                .filter(|feature| {
                    selected.contains(*feature) && !selected.contains(Feature::DenoToolchain)
                })
                .collect::<Vec<_>>();
            let issues = selection_issues(&selected);
            assert_eq!(
                issues.iter().map(|issue| issue.feature).collect::<Vec<_>>(),
                expected,
                "{selected:?}"
            );
            assert!(issues.iter().all(|issue| issue.slot == SlotId::Runner));
        }
    }

    #[test]
    fn selections_resolve_dependencies() {
        for selected in subsets() {
            let completed = complete(&selected);
            assert_eq!(complete(&completed), completed);
            assert!(selection_issues(&completed).is_empty());
            assert_eq!(canon(&completed), completed);
            for feature in features::selectable_features() {
                let added = select(&selected, feature);
                assert_eq!(select(&added, feature), added);
                assert!(selection_issues(&added).is_empty());
                let removed = unselect(&selected, feature);
                assert!(!removed.contains(feature));
                assert!(selection_issues(&removed).is_empty());
            }
        }
    }

    #[test]
    fn tailwind_replaces_starter() {
        assert_eq!(
            select(&set(&[Feature::StarterStylesheet]), Feature::TailwindCss),
            set(&[Feature::TailwindCss, Feature::DenoToolchain])
        );
    }

    #[test]
    fn removing_deno_removes_tools() {
        assert_eq!(
            unselect(
                &set(&[
                    Feature::StarterStylesheet,
                    Feature::Pagefind,
                    Feature::DenoToolchain
                ]),
                Feature::DenoToolchain
            ),
            set(&[Feature::StarterStylesheet])
        );
        assert_eq!(
            unselect(
                &set(&[
                    Feature::TailwindCss,
                    Feature::Pagefind,
                    Feature::DenoToolchain
                ]),
                Feature::DenoToolchain
            ),
            FeatureSet::default()
        );
    }
}
