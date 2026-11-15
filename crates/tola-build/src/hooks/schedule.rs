//! Stable batches within each declaring site or module.

use std::collections::BTreeMap;

/// Batch declaration indices within each site or module instance, then by ascending `order`.
/// `None` names the site; module aliases sort independently of their order values. The site
/// runs first only for `before-build`. Equal values from different instances never share a batch.
///
/// Keep all declarations here and filter participation afterward so diagnostics retain their
/// original indices. Declaration order breaks ties within a batch; it is not a dependency
/// between members. Serial execution uses these same batches with a single permit.
pub fn hook_batches<T>(
    hooks: &[T],
    site_first: bool,
    owner: impl Fn(&T) -> Option<&str>,
    order: impl Fn(&T) -> u64,
) -> Vec<Vec<usize>> {
    let mut owners: BTreeMap<Option<&str>, Vec<usize>> = BTreeMap::new();
    for (index, hook) in hooks.iter().enumerate() {
        owners.entry(owner(hook)).or_default().push(index);
    }
    let site = owners.remove(&None);
    let groups = site
        .iter()
        .filter(|_| site_first)
        .chain(owners.values())
        .chain(site.iter().filter(|_| !site_first));
    let mut batches = Vec::new();
    for group in groups {
        let mut indices = group.clone();
        indices.sort_by_key(|index| order(&hooks[*index]));
        for index in indices {
            if batches.last().is_some_and(|batch: &Vec<usize>| {
                order(&hooks[batch[0]]) == order(&hooks[index])
                    && owner(&hooks[batch[0]]) == owner(&hooks[index])
            }) {
                batches.last_mut().unwrap().push(index);
            } else {
                batches.push(vec![index]);
            }
        }
    }
    batches
}

/// Calculate the shared session limit without changing the order batches.
/// Disabling parallelism retains the configured jobs value and limits execution to one hook.
pub fn hook_limit(
    par_hooks: bool,
    jobs: crate::config::section::build::HookJobs,
    auto_limit: usize,
) -> usize {
    if par_hooks {
        jobs.resolved(auto_limit)
    } else {
        1
    }
}
