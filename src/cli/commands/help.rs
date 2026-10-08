use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;

use crate::cancellation::Cancellation;
use crate::cli::HelpArgs;
use crate::cli::log::{LogFile, destination};
use crate::cli::output::CommandOutput;
use crate::demos::preview::{DemoPreview, PreviewStatus};
use crate::help::model::{HelpDocument, LinkTarget, PageId};
use crate::help::pages::{self, CrossRefs};
use crate::help::view::{
    Clipboard, DestinationSource, PreviewPhase, PreviewState, ReaderAction, View,
};
use crate::i18n::HelpLanguage;
use crate::terminal::session::Terminal as _;
use crate::terminal::session::{self, ProcessTerminal, Shown, TerminalRefused};

pub(in crate::cli) fn run(
    args: &HelpArgs,
    language: HelpLanguage,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    let target = pages::request(&args.targets)?;
    let id = target.page().expect("a help request locates a page");
    let requested = pages::page_of(id, language, CrossRefs::Emit)?;
    let selected = id.demo().map(pages::demo).transpose()?;
    if (args.preview || args.export.is_some()) && selected.is_none() {
        return Err(operation_target());
    }
    if let Some(directory) = &args.export {
        destination::check_directory(output.log().map(LogFile::path), directory)?;
    }
    destination::start(output, &cancellation.token())?;
    if args.interactive && interactive_available(output) {
        return run_interactive(args, language, target, requested, output, cancellation);
    }
    if let Some(destination) = &args.export {
        let demo = selected.expect("demo operations require a selected demo");
        let exported = crate::demos::export::write(demo, destination, &cancellation.token())?;
        output.secondary(format!("Exported to {}", exported.display()))?;
        if args.edit {
            crate::editor::launch::edit(&exported, args.editor.as_deref(), &cancellation.token())?;
        }
        return Ok(());
    }
    if args.preview {
        return preview_plain(
            selected.expect("preview requires a selected demo"),
            output,
            cancellation,
        );
    }
    write_page(args, language, output, args.interactive)
}

fn interactive_available(output: &CommandOutput) -> bool {
    let sink = output.terminal().sink();
    let terminal = ProcessTerminal::new(&sink);
    terminal.is_interactive()
        && terminal.can_draw()
        && terminal
            .size()
            .is_ok_and(|(columns, rows)| columns > 0 && rows > 0)
}

fn write_page(
    args: &HelpArgs,
    language: HelpLanguage,
    output: &CommandOutput,
    direct: bool,
) -> Result<()> {
    let target = pages::request(&args.targets)?;
    if let Some(PageId::DemoFile { id, path }) = target.page() {
        let file = pages::source_file(id, path)?;
        if std::str::from_utf8(file.bytes).is_ok() {
            return output.write_stdout(file.bytes);
        }
    }
    let rendered = pages::render(
        &args.targets,
        language,
        output.terminal().stdout_columns(),
        output.terminal().stdout_uses_color(),
    )?;
    if direct {
        output.write_stdout(rendered)
    } else {
        output.write_documentation(rendered)
    }
}

fn run_interactive(
    args: &HelpArgs,
    language: HelpLanguage,
    target: LinkTarget,
    requested: crate::help::model::HelpPage,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    let preview = Rc::new(RefCell::new(DemoSession::default()));
    let completed = (|| -> Result<()> {
        let load = |id: &PageId| {
            let page = pages::page_of(id, language, CrossRefs::Emit)?;
            Ok(HelpDocument::parse(page))
        };
        let poll_preview = Rc::clone(&preview);
        let destinations = ExportDestinations;
        let poll = || poll_preview.borrow_mut().display(language);
        let clipboard = ReaderClipboard {
            sink: output.terminal().sink(),
            cancellation: cancellation.token(),
        };
        let requested_id = requested.id.clone();
        let mut view = View::new(
            HelpDocument::parse(requested),
            &load,
            output.terminal().columns().unwrap_or(80),
            output.terminal().palette(),
        );
        view.set_language(language);
        view.set_mouse(!args.no_mouse);
        view.set_preview(&poll);
        view.set_destinations(&destinations);
        view.set_clipboard(&clipboard);
        if let LinkTarget::PageAnchor(_, anchor) = &target {
            view.navigate(&LinkTarget::PageAnchor(requested_id, anchor.clone()));
        }
        if args.preview {
            let demo = target
                .page()
                .and_then(PageId::demo)
                .expect("a preview request has a demo");
            if let Err(error) = preview.borrow_mut().start(demo) {
                view.set_notice(friendly_error(&error, "could not preview this demo"));
            }
        }
        if let Some(destination) = &args.export {
            let demo = target
                .page()
                .and_then(PageId::demo)
                .expect("an export request has a demo");
            match export_demo(
                demo,
                destination,
                args.edit,
                args.editor.as_deref(),
                output,
                cancellation,
            ) {
                Ok(notice) => view.set_notice(notice),
                Err(error) if error.is::<tola_build::cancellation::BuildCancelled>() => {
                    return Err(error);
                }
                Err(error) => view.set_notice(friendly_error(&error, "could not export this demo")),
            }
        }
        let token = cancellation.token();
        let sink = output.terminal().sink();
        let cancelled = || token.is_cancelled();
        loop {
            let shown =
                match session::show(&sink, output.terminal().palette(), &cancelled, &mut view) {
                    Err(error)
                        if matches!(
                            error.downcast_ref::<TerminalRefused>(),
                            Some(TerminalRefused::NotTerminal)
                        ) =>
                    {
                        Shown::Plain
                    }
                    shown => shown?,
                };
            if shown == Shown::Plain {
                if let Some(notice) = view.notice() {
                    output.secondary(notice)?;
                }
                if args.preview {
                    let id = target
                        .page()
                        .and_then(PageId::demo)
                        .expect("a preview request has a demo");
                    let mut preview = preview.borrow_mut();
                    let current = preview.current.as_mut().ok_or_else(operation_target)?;
                    output.status(format!("Preparing demo {id}…"))?;
                    return wait_preview(&mut current.preview, output, cancellation);
                }
                return write_page(args, language, output, true);
            }
            let Some(action) = view.take_action() else {
                return Ok(());
            };
            // External tools and filesystem writes run after the session restored the terminal.
            let acted = match action {
                ReaderAction::Preview { demo } => preview.borrow_mut().start(&demo).map(|()| {
                    format!(
                        "Previewing {}",
                        pages::demo(&demo)
                            .expect("the selected demo exists")
                            .title(language)
                    )
                }),
                ReaderAction::StopPreview => preview
                    .borrow_mut()
                    .stop()
                    .map(|()| "Preview stopped".into()),
                ReaderAction::Export {
                    demo,
                    destination,
                    edit,
                } => export_demo(
                    &demo,
                    &destination,
                    edit,
                    args.editor.as_deref(),
                    output,
                    cancellation,
                ),
                ReaderAction::OpenBrowser { url } => {
                    crate::editor::launch::open_url(&url, &cancellation.token())
                        .map(|()| format!("Opened {url}"))
                }
            };
            match acted {
                Ok(notice) => view.set_notice(notice),
                Err(error) if error.is::<tola_build::cancellation::BuildCancelled>() => {
                    return Err(error);
                }
                Err(error) => {
                    output.record_failure(&error);
                    view.set_notice(friendly_error(
                        &error,
                        "could not complete this demo action",
                    ));
                }
            }
        }
    })();
    let stopped = preview.borrow_mut().stop();
    finish_preview(completed, stopped, output)
}

fn export_demo(
    id: &str,
    destination: &Path,
    edit: bool,
    editor: Option<&str>,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<String> {
    destination::check_directory(output.log().map(LogFile::path), destination)?;
    let demo = pages::demo(id)?;
    let exported = crate::demos::export::write(demo, destination, &cancellation.token())?;
    if edit
        && let Err(error) = crate::editor::launch::edit(&exported, editor, &cancellation.token())
    {
        if error.is::<tola_build::cancellation::BuildCancelled>() {
            return Err(error);
        }
        output.record_failure(&error);
        tracing::debug!(?error, "could not open the exported demo in the editor");
        return Ok(format!(
            "Exported to {} · {}",
            exported.display(),
            friendly_error(&error, "could not open the editor")
        ));
    }
    Ok(format!(
        "Exported to {} · press v to open",
        exported.display()
    ))
}

fn preview_plain(
    demo: &'static crate::demos::Demo,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    let mut preview = DemoPreview::start(demo)?;
    let completed = (|| {
        output.status(format!("Preparing demo {}…", demo.id))?;
        wait_preview(&mut preview, output, cancellation)
    })();
    let stopped = preview.stop();
    finish_preview(completed, stopped, output)
}

fn wait_preview(
    preview: &mut DemoPreview,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    let token = cancellation.token();
    let mut announced = false;
    loop {
        token.ensure_active()?;
        match preview.status() {
            PreviewStatus::Preparing => {}
            PreviewStatus::Ready(ready) => {
                if !announced {
                    output.serving(&ready.url)?;
                    output.secondary("Press Ctrl+C to stop the demo preview")?;
                    announced = true;
                }
            }
            PreviewStatus::Failed(error) => {
                return Err(anyhow::Error::new(SharedPreviewFailure(error)));
            }
            PreviewStatus::Stopped => return Ok(()),
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn finish_preview(
    completed: Result<()>,
    stopped: Result<()>,
    output: &CommandOutput,
) -> Result<()> {
    if let Err(error) = stopped {
        output.record_failure(&error);
        let mut diagnostics =
            crate::cli::output::attached_or_fallback(&error, crate::codes::demo::PREVIEW);
        for diagnostic in &mut diagnostics {
            diagnostic.severity = tola_build::diagnostic::Severity::Warning;
        }
        output.diagnostics(&diagnostics)?;
    }
    completed
}

#[derive(Debug)]
struct SharedPreviewFailure(Arc<anyhow::Error>);

impl std::fmt::Display for SharedPreviewFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self.0.as_ref(), formatter)
    }
}

impl std::error::Error for SharedPreviewFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref().as_ref())
    }
}

#[derive(Default)]
struct DemoSession {
    current: Option<ActivePreview>,
}

struct ActivePreview {
    demo: &'static crate::demos::Demo,
    preview: DemoPreview,
}

impl DemoSession {
    fn start(&mut self, id: &str) -> Result<()> {
        let demo = pages::demo(id)?;
        if let Some(current) = &mut self.current
            && current.demo.id == id
            && matches!(
                current.preview.status(),
                PreviewStatus::Preparing | PreviewStatus::Ready(_)
            )
        {
            return Ok(());
        }
        self.stop()?;
        self.current = Some(ActivePreview {
            demo,
            preview: DemoPreview::start(demo)?,
        });
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        if let Some(current) = &mut self.current {
            current.preview.stop()?;
        }
        Ok(())
    }

    fn display(&mut self, language: HelpLanguage) -> Option<PreviewState> {
        let current = self.current.as_mut()?;
        let phase = match current.preview.status() {
            PreviewStatus::Preparing => PreviewPhase::Preparing,
            PreviewStatus::Ready(ready) => PreviewPhase::Ready {
                url: ready.url.clone(),
            },
            PreviewStatus::Failed(error) => PreviewPhase::Failed {
                message: friendly_error(&error, "could not preview this demo"),
            },
            PreviewStatus::Stopped => PreviewPhase::Stopped,
        };
        Some(PreviewState {
            demo: current.demo.id.into(),
            title: current.demo.title(language).into(),
            phase,
        })
    }
}

/// What the export prompt reads: the directories under a path, and the absolute path a
/// typed destination names.
struct ExportDestinations;

impl DestinationSource for ExportDestinations {
    fn directories(&self, directory: &Path, prefix: &str) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return Vec::new();
        };
        let mut names = entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
            .filter(|name| name.starts_with(prefix))
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    fn resolve(&self, typed: &Path) -> PathBuf {
        std::path::absolute(typed).unwrap_or_else(|_| typed.to_path_buf())
    }

    fn base(&self) -> PathBuf {
        std::env::current_dir().unwrap_or_default()
    }
}

/// The reader's clipboard: the terminal's own escape, and this system's pasteboard when it
/// has one.
///
/// A terminal only reads the escape when it allows programs to write the clipboard, so the
/// system's own pasteboard decides what the reader can trust.
struct ReaderClipboard {
    sink: crate::terminal::OutputSink,
    cancellation: tola_build::cancellation::BuildCancellation,
}

impl Clipboard for ReaderClipboard {
    fn copy(&self, text: &str) -> bool {
        let escape = crate::terminal::clipboard::escape(text);
        let _ = self.sink.write_stderr_locked(escape.as_bytes());
        let Some(program) = crate::sys::clipboard_command() else {
            return true;
        };
        let mut command = std::process::Command::new(program);
        crate::sys::run_command_with_input(&mut command, text.as_bytes(), &self.cancellation)
            .is_ok_and(|status| status.success())
    }
}

fn friendly_error(error: &anyhow::Error, fallback: &str) -> String {
    match tola_build::diagnostic::attached(error).and_then(|diagnostics| diagnostics.first()) {
        Some(diagnostic) => match diagnostic.distinct_help().next() {
            Some(help) => format!("{} · {}", diagnostic.display_message(), help.message),
            None => diagnostic.display_message().into(),
        },
        None => fallback.into(),
    }
}

fn operation_target() -> anyhow::Error {
    let message = "choose a demo before previewing or exporting it";
    let diagnostic = tola_build::diagnostic::Diagnostic::new(
        crate::codes::help::TARGET,
        tola_build::diagnostic::Severity::Error,
        message,
    )
    .with_help("Use `tola help demo` to choose one");
    tola_build::diagnostic::DiagnosticError::new(message, vec![diagnostic]).into()
}
