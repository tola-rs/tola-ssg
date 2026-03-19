use std::path::PathBuf;

/// What happened to a file
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ChangeKind {
    Created,
    Modified,
    Removed,
}

impl ChangeKind {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Modified => "modified",
            Self::Removed => "removed",
        }
    }
}

/// Debounced file events, categorized by type
pub(super) struct DebouncedEvents(pub(super) Vec<(PathBuf, ChangeKind)>);

impl DebouncedEvents {
    pub(super) fn split(self) -> (Vec<PathBuf>, Vec<PathBuf>, Vec<PathBuf>) {
        let mut created = Vec::new();
        let mut modified = Vec::new();
        let mut removed = Vec::new();

        for (path, kind) in self.0 {
            match kind {
                ChangeKind::Created => created.push(path),
                ChangeKind::Modified => modified.push(path),
                ChangeKind::Removed => removed.push(path),
            }
        }

        (created, modified, removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(name: &str) -> PathBuf {
        PathBuf::from(format!("/tmp/{name}"))
    }

    #[test]
    fn debounced_events_split_groups_by_change_kind() {
        let events = DebouncedEvents(vec![
            (path("a.typ"), ChangeKind::Created),
            (path("b.typ"), ChangeKind::Modified),
            (path("c.typ"), ChangeKind::Removed),
            (path("d.typ"), ChangeKind::Created),
        ]);

        let (created, modified, removed) = events.split();

        assert_eq!(created.len(), 2);
        assert_eq!(modified.len(), 1);
        assert_eq!(removed.len(), 1);
    }
}
