use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::event::{accepts_event, normalized_event_paths};

pub(super) const QUIET_PERIOD_MS: u64 = 50;
/// Continuous edits must eventually close a batch even without a quiet interval.
pub(super) const MAX_BATCH_WAIT_MS: u64 = 250;

struct DebounceWindow {
    opened: Instant,
    last_event: Instant,
}

/// Collect paths to re-evaluate; filesystem discovery decides their current meaning.
pub(super) struct Debouncer {
    paths: BTreeSet<PathBuf>,
    window: Option<DebounceWindow>,
}

impl Debouncer {
    pub(super) fn new() -> Self {
        Self {
            paths: BTreeSet::new(),
            window: None,
        }
    }

    pub(super) fn add_event(&mut self, event: &notify::Event, now: Instant) {
        if !accepts_event(event.kind) {
            return;
        }
        // The raw firehose is one event per edit; the closed window below is what a reader wants.
        tracing::trace!(target: "tola::watch", event = ?event.kind, paths = ?event.paths, "raw notify");
        for path in normalized_event_paths(event) {
            if self.paths.len() < super::EVENT_DETAIL_CAPACITY {
                self.paths.insert(path);
            }
            self.window
                .get_or_insert(DebounceWindow {
                    opened: now,
                    last_event: now,
                })
                .last_event = now;
        }
    }

    pub(super) fn take_if_ready(
        &mut self,
        now: Instant,
        mut accepts: impl FnMut(&Path) -> bool,
    ) -> Option<Vec<PathBuf>> {
        if !self.remaining(now)?.is_zero() {
            return None;
        }
        let window = self.window.take()?;
        let sample = self
            .paths
            .iter()
            .take(crate::cli::log::LOGGED_PATH_SAMPLE)
            .cloned()
            .collect::<Vec<_>>();
        tracing::debug!(target: "tola::watch",
            debounce_ms = now.saturating_duration_since(window.opened).as_secs_f64() * 1000.0,
            queued_paths = self.paths.len(),
            sample_paths = ?sample,
            "closed filesystem debounce window");
        Some(
            std::mem::take(&mut self.paths)
                .into_iter()
                .filter(|path| accepts(path))
                .collect(),
        )
    }

    pub(super) fn remaining(&self, now: Instant) -> Option<Duration> {
        let window = self.window.as_ref()?;
        let quiet = Duration::from_millis(QUIET_PERIOD_MS)
            .saturating_sub(now.saturating_duration_since(window.last_event));
        let max_wait = Duration::from_millis(MAX_BATCH_WAIT_MS)
            .saturating_sub(now.saturating_duration_since(window.opened));
        Some(quiet.min(max_wait))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::EventKind;
    use notify::event::{CreateKind, DataChange, ModifyKind, RemoveKind, RenameMode};

    fn event(kind: EventKind, paths: &[&str]) -> notify::Event {
        notify::Event {
            kind,
            paths: paths.iter().map(PathBuf::from).collect(),
            attrs: Default::default(),
        }
    }

    fn modify(paths: &[&str]) -> notify::Event {
        event(EventKind::Modify(ModifyKind::Data(DataChange::Any)), paths)
    }

    fn paths(paths: &[&str]) -> Vec<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    fn settled(debouncer: &mut Debouncer, last_event: Instant) -> Vec<PathBuf> {
        debouncer
            .take_if_ready(last_event + Duration::from_millis(QUIET_PERIOD_MS), |_| {
                true
            })
            .expect("accepted events must produce a batch after the quiet interval")
    }

    #[test]
    fn duplicate_paths_collapse_in_order() {
        let now = Instant::now();
        let mut debouncer = Debouncer::new();
        debouncer.add_event(&modify(&["/tmp/z.typ", "/tmp/z.typ"]), now);
        debouncer.add_event(
            &event(EventKind::Remove(RemoveKind::File), &["/tmp/a.typ"]),
            now,
        );
        debouncer.add_event(
            &event(EventKind::Create(CreateKind::File), &["/tmp/m.typ"]),
            now,
        );

        assert_eq!(
            settled(&mut debouncer, now),
            paths(&["/tmp/a.typ", "/tmp/m.typ", "/tmp/z.typ"])
        );
        assert!(
            debouncer
                .take_if_ready(now + Duration::from_secs(1), |_| true)
                .is_none()
        );
    }

    #[test]
    fn quiet_deadline_gates_the_batch() {
        let now = Instant::now();
        let mut debouncer = Debouncer::new();
        debouncer.add_event(&modify(&["/tmp/a.typ"]), now);

        assert!(
            debouncer
                .take_if_ready(now + Duration::from_millis(QUIET_PERIOD_MS - 1), |_| true)
                .is_none()
        );
        assert_eq!(settled(&mut debouncer, now), paths(&["/tmp/a.typ"]));
    }

    #[test]
    fn repeated_path_extends_quiet_window() {
        let now = Instant::now();
        let later = now + Duration::from_millis(30);
        let mut debouncer = Debouncer::new();
        debouncer.add_event(&modify(&["/tmp/a.typ"]), now);
        debouncer.add_event(&modify(&["/tmp/a.typ"]), later);

        assert!(
            debouncer
                .take_if_ready(now + Duration::from_millis(QUIET_PERIOD_MS), |_| true)
                .is_none()
        );
        assert_eq!(settled(&mut debouncer, later), paths(&["/tmp/a.typ"]));
    }

    #[test]
    fn continuous_edits_close_at_deadline() {
        let now = Instant::now();
        let mut debouncer = Debouncer::new();
        for elapsed in (0..MAX_BATCH_WAIT_MS).step_by(40) {
            debouncer.add_event(
                &modify(&["/tmp/a.typ"]),
                now + Duration::from_millis(elapsed),
            );
        }
        let before_deadline = now + Duration::from_millis(MAX_BATCH_WAIT_MS - 1);
        assert!(debouncer.take_if_ready(before_deadline, |_| true).is_none());
        assert_eq!(
            debouncer.take_if_ready(now + Duration::from_millis(MAX_BATCH_WAIT_MS), |_| true),
            Some(paths(&["/tmp/a.typ"]))
        );
    }

    #[test]
    fn pathless_events_keep_the_window() {
        let now = Instant::now();
        let mut debouncer = Debouncer::new();
        debouncer.add_event(&modify(&["/tmp/a.typ"]), now);
        let later = now + Duration::from_millis(40);
        debouncer.add_event(
            &event(
                EventKind::Modify(ModifyKind::Metadata(
                    notify::event::MetadataKind::AccessTime,
                )),
                &["/tmp/a.typ"],
            ),
            later,
        );
        debouncer.add_event(&modify(&[]), later);

        assert_eq!(settled(&mut debouncer, now), paths(&["/tmp/a.typ"]));
    }

    #[test]
    fn rename_events_keep_every_endpoint() {
        let now = Instant::now();
        let mut debouncer = Debouncer::new();
        let paired = event(
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            &["/tmp/b.typ", "/tmp/a.typ"],
        );
        debouncer.add_event(&paired, now);
        debouncer.add_event(&paired, now);
        assert_eq!(
            settled(&mut debouncer, now),
            paths(&["/tmp/a.typ", "/tmp/b.typ"])
        );

        let mut debouncer = Debouncer::new();
        for (mode, path) in [
            (RenameMode::From, "/tmp/a.typ"),
            (RenameMode::From, "/tmp/b.typ"),
            (RenameMode::To, "/tmp/b2.typ"),
            (RenameMode::To, "/tmp/a2.typ"),
        ] {
            debouncer.add_event(
                &event(EventKind::Modify(ModifyKind::Name(mode)), &[path]),
                now,
            );
        }
        assert_eq!(
            settled(&mut debouncer, now),
            paths(&["/tmp/a.typ", "/tmp/a2.typ", "/tmp/b.typ", "/tmp/b2.typ"])
        );
    }

    #[test]
    fn separate_windows_close_independently() {
        let now = Instant::now();
        let mut debouncer = Debouncer::new();
        debouncer.add_event(
            &event(
                EventKind::Modify(ModifyKind::Name(RenameMode::From)),
                &["/tmp/a.typ"],
            ),
            now,
        );
        assert_eq!(settled(&mut debouncer, now), paths(&["/tmp/a.typ"]));
        let later = now + Duration::from_secs(1);
        debouncer.add_event(
            &event(
                EventKind::Modify(ModifyKind::Name(RenameMode::To)),
                &["/tmp/b.typ"],
            ),
            later,
        );
        assert_eq!(settled(&mut debouncer, later), paths(&["/tmp/b.typ"]));
    }

    #[test]
    fn structural_event_keeps_ancestor() {
        for kind in [
            EventKind::Create(CreateKind::Folder),
            EventKind::Remove(RemoveKind::Folder),
            EventKind::Modify(ModifyKind::Name(RenameMode::Any)),
        ] {
            let now = Instant::now();
            let mut debouncer = Debouncer::new();
            debouncer.add_event(&modify(&["/site/content"]), now);
            debouncer.add_event(&event(kind, &["/site/content"]), now);
            debouncer.add_event(&modify(&["/site/content", "/site/content/post.typ"]), now);
            assert_eq!(
                settled(&mut debouncer, now),
                paths(&["/site/content", "/site/content/post.typ"])
            );
        }
    }

    #[test]
    fn removed_path_still_needs_recheck() {
        let now = Instant::now();
        let mut debouncer = Debouncer::new();
        debouncer.add_event(
            &event(
                EventKind::Create(CreateKind::File),
                &["/site/content/post.typ"],
            ),
            now,
        );
        debouncer.add_event(
            &event(
                EventKind::Remove(RemoveKind::File),
                &["/site/content/post.typ"],
            ),
            now,
        );
        assert_eq!(
            settled(&mut debouncer, now),
            paths(&["/site/content/post.typ"])
        );
    }

    #[test]
    fn rejected_child_keeps_accepted_parent() {
        let now = Instant::now();
        let mut debouncer = Debouncer::new();
        debouncer.add_event(
            &modify(&["/site/content", "/site/content/ignored.typ"]),
            now,
        );
        let accepted = debouncer
            .take_if_ready(now + Duration::from_millis(QUIET_PERIOD_MS), |path| {
                path != Path::new("/site/content/ignored.typ")
            });
        assert_eq!(accepted, Some(paths(&["/site/content"])));
    }
}
