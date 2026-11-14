//! Count interrupted attempts without turning an ordinary save into a warning.

use std::time::{Duration, Instant};

use super::round::BuildProgress;

const REPEATED_AFTER: Duration = Duration::from_secs(2);
const REPORT_INTERVAL: Duration = Duration::from_secs(5);

pub(super) struct Restarts {
    started: Instant,
    count: u64,
    warned: bool,
    reported: Option<Instant>,
    updated: Option<Instant>,
    elapsed: Duration,
}

impl Default for Restarts {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            count: 0,
            warned: false,
            reported: None,
            updated: None,
            elapsed: Duration::ZERO,
        }
    }
}

impl Restarts {
    /// Count every restart, update screens once a second, and print less often to redirected output.
    pub(super) fn record(&mut self, now: Instant) -> Option<bool> {
        self.count += 1;
        self.elapsed = now.duration_since(self.started);
        let warning = self.count >= 3 && self.elapsed >= REPEATED_AFTER;
        let print = (warning && !self.warned)
            || self
                .reported
                .is_none_or(|last| now.duration_since(last) >= REPORT_INTERVAL);
        self.warned |= warning;
        if print {
            self.reported = Some(now);
        }
        if print
            || self
                .updated
                .is_none_or(|last| now.duration_since(last) >= Duration::from_secs(1))
        {
            self.updated = Some(now);
            Some(print)
        } else {
            None
        }
    }

    pub(super) fn progress(
        &self,
        reason: &str,
        paths: &[String],
        watches: &[String],
        has_preview: bool,
    ) -> BuildProgress {
        let mut lines = Vec::new();
        if self.warned {
            lines.push(format!(
                "{} restarts in {:.1} s; no new build has finished",
                self.count,
                self.elapsed.as_secs_f64(),
            ));
            lines.push(reason.to_owned());
        }
        lines.extend(paths.iter().take(4).map(|path| format!("Changed: {path}")));
        if paths.len() > 4 {
            lines.push(format!("… and {} more paths", paths.len() - 4));
        }
        lines.extend(
            watches
                .iter()
                .take(4)
                .map(|watch| format!("Watched by: {watch}")),
        );
        if self.warned {
            if !paths.is_empty() {
                lines.push(if watches.is_empty() {
                    "Check whether a tool is repeatedly rewriting these inputs.".to_owned()
                } else {
                    "Check these hooks' `rerun-on` inputs. Keep temporary files in `hook.tempDir`, and persistent backups in `hook.cacheDir`.".to_owned()
                });
            }
            lines.push(if has_preview {
                "Preview still shows the last successful build. Retrying; Ctrl+C to stop."
                    .to_owned()
            } else {
                "No preview has been built yet. Retrying; Ctrl+C to stop.".to_owned()
            });
        }
        BuildProgress {
            title: if self.warned {
                "Build keeps restarting".to_owned()
            } else {
                format!("{reason}; restarting build")
            },
            detail: lines.join("\n"),
            warning: self.warned,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_interruptions_warn_without_printing_every_attempt() {
        let mut restarts = Restarts::default();
        let start = restarts.started;
        assert_eq!(restarts.record(start), Some(true));
        assert!(!restarts.warned);
        assert_eq!(restarts.record(start + Duration::from_millis(100)), None);
        assert_eq!(restarts.record(start + REPEATED_AFTER), Some(true));
        assert!(restarts.warned);
        for offset in 1..164 {
            assert_eq!(
                restarts.record(start + REPEATED_AFTER + Duration::from_millis(offset)),
                None
            );
        }
        assert_eq!(
            restarts.record(start + REPEATED_AFTER + Duration::from_secs(1)),
            Some(false)
        );
        assert_eq!(
            restarts.record(start + REPEATED_AFTER + REPORT_INTERVAL),
            Some(true)
        );
        assert!(!Restarts::default().warned);
    }
}
