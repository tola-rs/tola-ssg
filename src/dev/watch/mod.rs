//! Filesystem changes, input coverage, and event boundaries used by development builds.

mod diagnostic;
mod fs;
mod requirements;

pub(crate) use diagnostic::with_diagnostics;
pub(crate) use fs::{
    EVENT_DETAIL_CAPACITY, FileChangeBatch, FileChangeSource, ProducerKind, RebuildScope,
    WatchError, WatchRequirements,
};
pub(crate) use requirements::{
    append_observed_paths, append_typst_inputs, watch_requirements_from_observation,
};
