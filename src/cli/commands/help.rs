use anyhow::Result;

use crate::cancellation::Cancellation;
use crate::cli::HelpArgs;
use crate::cli::log::destination;
use crate::cli::output::CommandOutput;
use crate::help::model::{HelpDocument, LinkTarget, PageId};
use crate::help::pages::{self, CrossRefs};
use crate::help::view::{Clipboard, View};
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
    destination::start(output, &cancellation.token())?;
    if args.interactive && interactive_available(output) {
        return run_interactive(args, language, target, requested, output, cancellation);
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
    let load = |id: &PageId| {
        let page = pages::page_of(id, language, CrossRefs::Emit)?;
        Ok(HelpDocument::parse(page))
    };
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
    view.set_clipboard(&clipboard);
    if let LinkTarget::PageAnchor(_, anchor) = &target {
        view.navigate(&LinkTarget::PageAnchor(requested_id, anchor.clone()));
    }
    let token = cancellation.token();
    let sink = output.terminal().sink();
    let cancelled = || token.is_cancelled();
    let shown = match session::show(&sink, output.terminal().palette(), &cancelled, &mut view) {
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
        return write_page(args, language, output, true);
    }
    Ok(())
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
