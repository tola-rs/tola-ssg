//! Priority levels for compilation and task ordering.

/// Priority level for task ordering
///
/// Higher value = higher priority (processed first in BinaryHeap)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    /// Background pre-compilation - lowest priority
    Background = 0,
    /// File affected by dependency changes
    Affected = 1,
    /// File directly modified by user (hot-reload)
    Direct = 2,
    /// Page currently being viewed (on-demand) - highest priority
    Active = 3,
}
