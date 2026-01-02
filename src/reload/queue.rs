//! Compilation Queue
//!
//! Priority-ordered queue for hot-reload compilation.

use std::path::PathBuf;

use rustc_hash::FxHashSet;

// Re-export Priority from core for convenience
pub use crate::core::Priority;

// =============================================================================
// Prioritized File
// =============================================================================

/// A file with its compilation priority.
#[derive(Debug, Clone)]
pub struct PrioritizedFile {
    pub path: PathBuf,
    pub priority: Priority,
}

impl PrioritizedFile {
    pub fn active(path: PathBuf) -> Self {
        Self {
            path,
            priority: Priority::Active,
        }
    }
}

// =============================================================================
// Compile Queue
// =============================================================================

/// A queue of files to compile, ordered by priority.
#[derive(Debug, Default)]
pub struct CompileQueue {
    pub(crate) files: Vec<PrioritizedFile>,
}

impl CompileQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add files with given priority, avoiding duplicates.
    pub fn add(&mut self, paths: impl IntoIterator<Item = PathBuf>, priority: Priority) {
        let existing: FxHashSet<_> = self.files.iter().map(|f| f.path.clone()).collect();
        let to_add: Vec<_> = paths
            .into_iter()
            .filter(|path| !existing.contains(path))
            .map(|path| PrioritizedFile { path, priority })
            .collect();
        self.files.extend(to_add);
    }

    /// Promote a file to active priority (highest) if it exists in the queue.
    ///
    /// Returns true if the file was found and promoted, false otherwise.
    pub fn set_active(&mut self, path: PathBuf) -> bool {
        let Some(pos) = self.files.iter().position(|f| f.path == path) else {
            return false;
        };
        self.files.remove(pos);
        self.files.insert(0, PrioritizedFile::active(path));
        true
    }

    /// Get files in priority order (stable sort preserves insertion order within same priority).
    /// Higher priority (Active > Direct > Affected > Background) comes first.
    pub fn into_ordered(mut self) -> Vec<PathBuf> {
        self.files.sort_by_key(|f| std::cmp::Reverse(f.priority));
        self.files.into_iter().map(|f| f.path).collect()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Get the number of files in the queue.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Get files that were directly modified (Priority::Direct or Priority::Active).
    ///
    /// These files should be compiled first with lowest latency.
    /// Use case: Serial compilation for immediate hot-reload feedback.
    pub fn direct_files(&self) -> impl Iterator<Item = &PathBuf> {
        self.files
            .iter()
            .filter(|f| matches!(f.priority, Priority::Active | Priority::Direct))
            .map(|f| &f.path)
    }

    /// Get files that were affected by dependency changes (Priority::Affected).
    ///
    /// These files can be compiled with lower priority or in parallel.
    /// Use case: Background compilation while user continues editing.
    pub fn affected_files(&self) -> impl Iterator<Item = &PathBuf> {
        self.files
            .iter()
            .filter(|f| f.priority == Priority::Affected)
            .map(|f| &f.path)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_priority_ordering() {
        // Higher value = higher priority (Active > Direct > Affected)
        assert!(Priority::Active > Priority::Direct);
        assert!(Priority::Direct > Priority::Affected);
    }

    #[test]
    fn test_compile_queue_ordering() {
        let mut queue = CompileQueue::new();

        // Add files first, then promote one to active
        queue.add([PathBuf::from("/affected.typ")], Priority::Affected);
        queue.add([PathBuf::from("/direct.typ")], Priority::Direct);
        queue.add([PathBuf::from("/active.typ")], Priority::Affected);
        assert!(queue.set_active(PathBuf::from("/active.typ"))); // Should succeed

        let ordered = queue.into_ordered();
        assert_eq!(ordered[0], PathBuf::from("/active.typ"));
        assert_eq!(ordered[1], PathBuf::from("/direct.typ"));
        assert_eq!(ordered[2], PathBuf::from("/affected.typ"));
    }

    #[test]
    fn test_compile_queue_dedup() {
        let mut queue = CompileQueue::new();

        queue.add([PathBuf::from("/a.typ")], Priority::Affected);
        queue.add([PathBuf::from("/a.typ")], Priority::Direct); // Should not add duplicate

        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn test_direct_and_affected_files() {
        let mut queue = CompileQueue::new();

        queue.add([PathBuf::from("/a.typ")], Priority::Direct);
        queue.add([PathBuf::from("/b.typ")], Priority::Affected);
        queue.add([PathBuf::from("/c.typ")], Priority::Direct);
        queue.add([PathBuf::from("/active.typ")], Priority::Affected);
        assert!(queue.set_active(PathBuf::from("/active.typ"))); // Promote existing file

        // Direct files include Active and Direct priorities
        let direct: Vec<_> = queue.direct_files().collect();
        assert_eq!(direct.len(), 3);
        assert!(direct.contains(&&PathBuf::from("/active.typ")));
        assert!(direct.contains(&&PathBuf::from("/a.typ")));
        assert!(direct.contains(&&PathBuf::from("/c.typ")));

        // Affected files only include Affected priority
        let affected: Vec<_> = queue.affected_files().collect();
        assert_eq!(affected.len(), 1);
        assert_eq!(affected[0], &PathBuf::from("/b.typ"));
    }

    #[test]
    fn test_set_active_not_in_queue() {
        let mut queue = CompileQueue::new();
        queue.add([PathBuf::from("/a.typ")], Priority::Affected);

        // set_active should return false if file not in queue
        assert!(!queue.set_active(PathBuf::from("/not_in_queue.typ")));
        assert_eq!(queue.len(), 1); // Queue unchanged
    }
}
