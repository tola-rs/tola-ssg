//! Per-invocation permissions across input layers.

use crate::InputScope;

/// Input read, write, and fetch permissions computed from CLI flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputMode {
    scope: InputScope,
    locked: bool,
}

impl InputMode {
    /// Build input permissions (`--pure` implies `--locked`).
    pub const fn new(scope: InputScope, locked: bool) -> Self {
        Self {
            scope,
            locked: locked || matches!(scope, InputScope::Pure),
        }
    }

    pub const fn scope(self) -> InputScope {
        self.scope
    }

    pub const fn locked(self) -> bool {
        self.locked
    }

    pub const fn may_write_lock(self) -> bool {
        !self.locked
    }

    pub const fn may_use_network(self) -> bool {
        matches!(self.scope, InputScope::Online)
    }

    pub const fn may_read_cache(self) -> bool {
        !matches!(self.scope, InputScope::Pure)
    }

    pub const fn may_read_host_inputs(self) -> bool {
        !matches!(self.scope, InputScope::Pure)
    }

    pub const fn requires_lock(self) -> bool {
        self.locked
    }
}
