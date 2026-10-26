use std::io::Write;

use anyhow::Result;
use serde_json::{Value, json};
use tola_diagnostic::Diagnostic;

#[derive(Clone, Copy, Default)]
pub(crate) struct Events(pub(crate) bool);

impl Events {
    fn send(self, event: impl FnOnce() -> Value) -> Result<()> {
        if self.0 {
            let mut event = event();
            event["formatVersion"] = json!(1);
            let mut stdout = std::io::stdout().lock();
            serde_json::to_writer(&mut stdout, &event)?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
        }
        Ok(())
    }

    pub(crate) fn server_ready(self, url: &str, revision: Option<&str>) -> Result<()> {
        self.send(|| json!({"event": "server-ready", "url": url, "revision": revision}))
    }

    pub(crate) fn build_started(self, build: u64) -> Result<()> {
        self.send(|| json!({"event": "build-started", "build": build}))
    }

    pub(crate) fn build_finished(
        self,
        build: u64,
        status: &str,
        published: bool,
        revision: Option<&str>,
        diagnostics: &[Diagnostic],
    ) -> Result<()> {
        self.send(|| {
            json!({
                "event": "build-finished", "build": build, "status": status,
                "published": published, "revision": revision, "diagnostics": diagnostics,
            })
        })
    }

    pub(crate) fn hook_finished(
        self,
        hook: Option<&str>,
        revision: &str,
        status: &str,
        diagnostics: &[Diagnostic],
    ) -> Result<()> {
        self.send(|| {
            json!({
                "event": "hook-finished", "stage": "after-publish", "hook": hook,
                "revision": revision, "status": status, "diagnostics": diagnostics,
            })
        })
    }

    pub(crate) fn stopped(self, reason: &str, diagnostics: &[Diagnostic]) -> Result<()> {
        self.send(|| json!({"event": "stopped", "reason": reason, "diagnostics": diagnostics}))
    }
}
