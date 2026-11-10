//! Development watch paths derived from one prepared contribution.

use std::path::PathBuf;

use crate::hooks::RerunSelection;

/// Conservative inputs and generated paths of hooks that may run in development.
/// No options are evaluated and no command is run to construct this value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookWatchInputs {
    pub(crate) rerun_selections: Vec<RerunSelection>,
    pub(crate) hook_scripts: Vec<PathBuf>,
    pub(crate) generated_paths: Vec<PathBuf>,
}

impl HookWatchInputs {
    /// Extra inputs whose changes can rerun a module hook.
    pub fn rerun_selections(&self) -> &[RerunSelection] {
        &self.rerun_selections
    }
    /// Nu scripts whose changes can rerun a module hook.
    pub fn hook_scripts(&self) -> &[PathBuf] {
        &self.hook_scripts
    }
    /// Before-build output paths that may need event quarantine.
    pub fn generated_paths(&self) -> &[PathBuf] {
        &self.generated_paths
    }
}
