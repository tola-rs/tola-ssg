//! Development watch paths derived from one prepared contribution.

use std::path::PathBuf;

use crate::hooks::{HookStage, RerunSelection};

/// One hook's watched inputs and the declaration that selected them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RerunWatch {
    /// Name in the module's hook declaration.
    pub name: String,
    /// Module manifest containing the declaration.
    pub manifest: PathBuf,
    /// Stage containing the named hook.
    pub stage: HookStage,
    /// Compiled include and exclude patterns.
    pub selection: RerunSelection,
}

/// Conservative inputs and generated paths of hooks that may run in development.
/// No options are evaluated and no command is run to construct this value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookWatchInputs {
    pub(crate) rerun_watches: Vec<RerunWatch>,
    pub(crate) hook_scripts: Vec<PathBuf>,
    pub(crate) generated_paths: Vec<PathBuf>,
}

impl HookWatchInputs {
    /// Extra inputs whose changes can rerun a module hook.
    pub fn rerun_watches(&self) -> &[RerunWatch] {
        &self.rerun_watches
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
