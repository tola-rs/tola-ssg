use std::io::Write;

use anyhow::Result;
use serde::Serialize;
use tola_diagnostic::Diagnostic;

#[derive(Clone, Copy, Default)]
pub(crate) struct Events(pub(crate) bool);

/// One line of the `tola dev --json` stream.
#[derive(Serialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
enum DevEvent<'a> {
    ServerReady {
        #[serde(rename = "formatVersion")]
        format_version: u8,
        url: &'a str,
        revision: Option<&'a str>,
    },
    BuildStarted {
        #[serde(rename = "formatVersion")]
        format_version: u8,
        build: u64,
    },
    BuildFinished {
        #[serde(rename = "formatVersion")]
        format_version: u8,
        build: u64,
        status: &'a str,
        published: bool,
        revision: Option<&'a str>,
        diagnostics: &'a [Diagnostic],
    },
    HookFinished {
        #[serde(rename = "formatVersion")]
        format_version: u8,
        stage: &'static str,
        hook: Option<&'a str>,
        revision: &'a str,
        status: &'a str,
        diagnostics: &'a [Diagnostic],
    },
    Stopped {
        #[serde(rename = "formatVersion")]
        format_version: u8,
        reason: &'a str,
        diagnostics: &'a [Diagnostic],
    },
}

impl Events {
    fn send(self, event: DevEvent<'_>) -> Result<()> {
        if self.0 {
            let mut stdout = std::io::stdout().lock();
            serde_json::to_writer(&mut stdout, &event)?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
        }
        Ok(())
    }

    pub(crate) fn server_ready(self, url: &str, revision: Option<&str>) -> Result<()> {
        self.send(DevEvent::ServerReady {
            format_version: 1,
            url,
            revision,
        })
    }

    pub(crate) fn build_started(self, build: u64) -> Result<()> {
        self.send(DevEvent::BuildStarted {
            format_version: 1,
            build,
        })
    }

    pub(crate) fn build_finished(
        self,
        build: u64,
        status: &str,
        published: bool,
        revision: Option<&str>,
        diagnostics: &[Diagnostic],
    ) -> Result<()> {
        self.send(DevEvent::BuildFinished {
            format_version: 1,
            build,
            status,
            published,
            revision,
            diagnostics,
        })
    }

    pub(crate) fn hook_finished(
        self,
        hook: Option<&str>,
        revision: &str,
        status: &str,
        diagnostics: &[Diagnostic],
    ) -> Result<()> {
        self.send(DevEvent::HookFinished {
            format_version: 1,
            stage: "after-publish",
            hook,
            revision,
            status,
            diagnostics,
        })
    }

    pub(crate) fn stopped(self, reason: &str, diagnostics: &[Diagnostic]) -> Result<()> {
        self.send(DevEvent::Stopped {
            format_version: 1,
            reason,
            diagnostics,
        })
    }
}
