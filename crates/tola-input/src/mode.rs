//! What one attempt may do to each input layer.
//!
//! The mode is computed once from the command's flags and read by every layer that resolves,
//! writes, or reads an input.

use crate::InputScope;

/// What one attempt may read, write, and fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputMode {
    scope: InputScope,
    locked: bool,
}

impl InputMode {
    /// Build the mode for one attempt. `--pure` implies `--locked`.
    pub const fn new(scope: InputScope, locked: bool) -> Self {
        Self {
            scope,
            locked: locked || matches!(scope, InputScope::Pure),
        }
    }

    /// The input scope the command selected.
    pub const fn scope(self) -> InputScope {
        self.scope
    }

    /// Whether this attempt may not write the lock, even when a declaration changed.
    pub const fn locked(self) -> bool {
        self.locked
    }

    /// Whether resolving may write the lock.
    pub const fn may_write_lock(self) -> bool {
        !self.locked
    }

    /// Whether resolving may use the network.
    pub const fn may_use_network(self) -> bool {
        matches!(self.scope, InputScope::Online)
    }

    /// Whether a build may read a cache outside `vendor/`.
    pub const fn may_read_cache(self) -> bool {
        !matches!(self.scope, InputScope::Pure)
    }

    /// Whether a build may read a host package root or a system font.
    pub const fn may_read_host_inputs(self) -> bool {
        !matches!(self.scope, InputScope::Pure)
    }

    /// Whether this attempt needs an existing, complete lock before it can proceed.
    pub const fn requires_lock(self) -> bool {
        self.locked
    }
}
