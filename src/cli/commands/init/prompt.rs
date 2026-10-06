//! Interactive choice of the site directory, the scaffold's preset, and its editors.

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

use crate::cancellation::Cancellation;
use crate::cli::args::InitArgs;
use crate::cli::output::CommandOutput;
use crate::editor::Editor;
use crate::terminal::display_path_toward_home;
use crate::terminal::session::{self, Shown};

use super::custom;
use super::features::{self, FeatureSet};
use super::selection;

/// The marker a recommended preset's row has.
const RECOMMENDED_MARKER: &str = "(recommended)";

/// The preset a non-interactive invocation writes when no name is given.
const DEFAULT_PRESET: &str = features::DEFAULT_PRESET;

/// The preset the prompt recommends and starts on.
fn recommended_preset() -> &'static str {
    features::preset_choices()
        .find(|preset| preset.recommended)
        .expect("a preset is recommended")
        .name
}

/// Bail unless this terminal can ask the interactive questions.
pub(super) fn ensure_terminal(output: &CommandOutput) -> Result<()> {
    if !output.is_interactive() {
        bail!("`--interactive` needs a terminal; use `--preset` and `--editor` instead");
    }
    Ok(())
}

/// The site directory this invocation initializes: the named path, the interactive answer, or
/// the working directory when neither applies.
pub(super) fn site_directory(
    args: &InitArgs,
    cwd: &Path,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<PathBuf> {
    if let Some(path) = args.path.as_deref() {
        return Ok(cwd.join(path));
    }
    if !args.interactive {
        return Ok(cwd.to_path_buf());
    }
    let directory = output.read_line("Site directory", ".", &|| cancellation.is_requested())?;
    Ok(cwd.join(directory))
}

/// The scaffold one invocation resolves to.
pub(super) enum ScaffoldChoice {
    /// A named preset: the numbered question and the default without flags.
    Preset(String),
    /// A feature selection: the `--features` names, or the interactive screen's answer.
    Features(FeatureSet),
}

impl ScaffoldChoice {
    /// The features this choice scaffolds.
    pub(super) fn features(&self) -> FeatureSet {
        match self {
            ScaffoldChoice::Preset(name) => features::features(name),
            ScaffoldChoice::Features(set) => set.clone(),
        }
    }
}

/// The scaffold and editors this invocation uses, asking when `--interactive` asks for it.
pub(super) fn resolve_choices(
    args: &InitArgs,
    root: &Path,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<(ScaffoldChoice, Vec<Editor>)> {
    if !args.interactive {
        let choice = if args.features.is_empty() {
            ScaffoldChoice::Preset(
                args.preset
                    .clone()
                    .unwrap_or_else(|| DEFAULT_PRESET.to_owned()),
            )
        } else {
            ScaffoldChoice::Features(named_selection(args))
        };
        return Ok((choice, args.editor.clone()));
    }
    let choice = choose_interactively(args, root, output, cancellation)?;
    let editors = choose_editors(&args.editor, output, cancellation)?;
    Ok((choice, editors))
}

/// The scaffold the interactive screen answers, or the numbered questions a terminal that cannot
/// draw the screen answers instead.
fn choose_interactively(
    args: &InitArgs,
    root: &Path,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<ScaffoldChoice> {
    let initial = initial_selection(args);
    let mut view = custom::View::new(initial.clone(), root);
    let token = cancellation.token();
    let cancelled = || token.is_cancelled();
    let sink = output.terminal().sink();
    match session::show(&sink, output.terminal().palette(), &cancelled, &mut view)? {
        Shown::Interactive => Ok(ScaffoldChoice::Features(view.selection())),
        // A terminal that cannot draw the screen cannot edit a named selection either; the
        // numbered preset question answers when no features were named.
        Shown::Plain if !args.features.is_empty() => Ok(ScaffoldChoice::Features(initial)),
        Shown::Plain => Ok(ScaffoldChoice::Preset(choose_preset(
            args.preset.as_deref().unwrap_or(recommended_preset()),
            output,
            cancellation,
        )?)),
    }
}

fn initial_selection(args: &InitArgs) -> FeatureSet {
    let selected = if args.features.is_empty() {
        let name = args.preset.as_deref().unwrap_or(recommended_preset());
        features::features(name)
    } else {
        named_selection(args)
    };
    selection::complete(&selected)
}

/// The selection `--features` names, as given.
fn named_selection(args: &InitArgs) -> FeatureSet {
    FeatureSet::new(args.features.iter().map(|name| features::feature(name)))
}

/// Show the resolved scaffold and ask before anything is written.
pub(super) fn confirm_scaffold(
    root: &Path,
    selected: &FeatureSet,
    editors: &[Editor],
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<bool> {
    let editors = if editors.is_empty() {
        "none".to_owned()
    } else {
        editors
            .iter()
            .copied()
            .map(Editor::name)
            .collect::<Vec<_>>()
            .join(", ")
    };
    output.block(format!(
        "Directory: {}\nFeatures: {}\nEditors: {editors}",
        display_path_toward_home(root),
        features::selection_label(selected),
    ))?;
    output.confirm("Create this site?", true, &|| cancellation.is_requested())
}

/// The prompt row one preset shows: its name, terse composition, and the recommendation marker.
fn preset_label(preset: &features::Preset) -> String {
    let marker = if preset.recommended {
        format!(" {RECOMMENDED_MARKER}")
    } else {
        String::new()
    };
    format!(
        "{:<width$} ({}){marker}",
        preset.name,
        preset.composition,
        width = preset_column()
    )
}

/// The column the preset names are padded to: the longest the model offers.
fn preset_column() -> usize {
    features::preset_choices()
        .map(|preset| preset.name.len())
        .max()
        .unwrap_or(0)
}

/// The row the prompt starts on.
fn initial_row(initial: &str) -> usize {
    features::preset_choices()
        .position(|preset| preset.name == initial)
        .expect("every preset is offered")
}

fn choose_preset(
    initial: &str,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<String> {
    let presets = features::preset_choices().collect::<Vec<_>>();
    let choices = presets
        .iter()
        .map(|preset| preset_label(preset))
        .collect::<Vec<_>>();
    let choices = choices.iter().map(String::as_str).collect::<Vec<_>>();
    let selected = output.select_one(
        "Choose a scaffold preset",
        &choices,
        initial_row(initial),
        &|| cancellation.is_requested(),
    )?;
    Ok(presets[selected].name.to_owned())
}

/// The editors this invocation writes settings for, asking when none were named.
fn choose_editors(
    editors: &[Editor],
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<Vec<Editor>> {
    if !editors.is_empty() {
        return Ok(editors.to_vec());
    }
    let choices = crate::editor::selection_choices();
    let choices = choices.iter().map(String::as_str).collect::<Vec<_>>();
    let selected =
        output.select_many("Select editors", &choices, &|| cancellation.is_requested())?;
    let editors = <Editor as clap::ValueEnum>::value_variants();
    Ok(selected.into_iter().map(|index| editors[index]).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_interactive_default_is_minimal() {
        let (sink, _output) = crate::terminal::OutputSink::buffered();
        let output = CommandOutput::new(
            crate::terminal::Terminal::with_sink(sink, false, false),
            None,
        );
        let args = InitArgs {
            path: None,
            dry_run: false,
            force: false,
            preset: None,
            features: Vec::new(),
            interactive: false,
            editor: Vec::new(),
        };
        let (choice, editors) = resolve_choices(
            &args,
            Path::new("/tmp/site"),
            &output,
            &Cancellation::default(),
        )
        .unwrap();
        assert!(matches!(&choice, ScaffoldChoice::Preset(name) if name == "minimal"));
        assert_eq!(choice.features(), features::features("minimal"));
        assert!(editors.is_empty());
    }
    #[test]
    fn interactive_selection_completes_requirements() {
        let args = InitArgs {
            path: None,
            dry_run: false,
            force: false,
            preset: None,
            features: vec!["starter-stylesheet".to_owned(), "tailwind-css".to_owned()],
            interactive: true,
            editor: Vec::new(),
        };
        let selected = initial_selection(&args);
        assert!(selected.contains(features::Feature::TailwindCss));
        assert!(selected.contains(features::Feature::DenoToolchain));
        assert!(!selected.contains(features::Feature::StarterStylesheet));
    }
}
