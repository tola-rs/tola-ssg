//! Finite commands around candidate construction and publication.
//!
//! The environment a hook command receives, and what a superseded build cancels, is written
//! for the site author in the section's and each stage's `HELP`, which `tola help "[build.hooks]"`
//! and its stage pages render.

use crate::config::{ConfigDiagnostics, FieldPath};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tola_config::Config;

/// External commands Tola runs around a build, grouped by stage. Each stage waits for its
/// commands to exit before it continues.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Config)]
#[serde(default, deny_unknown_fields)]
#[config(section = "build.hooks")]
pub struct HooksConfig {
    #[serde(rename = "before-build")]
    #[config(sub, collection = array_table)]
    pub before_build: Vec<BeforeBuildHookConfig>,
    #[serde(rename = "generate-outputs")]
    #[config(sub, collection = array_table)]
    pub generate_outputs: Vec<OutputCommandConfig>,
    #[serde(rename = "after-publish")]
    #[config(sub, collection = array_table)]
    pub after_publish: Vec<AfterPublishHookConfig>,
}

impl HooksConfig {
    /// What `tola help "[build.hooks]"` adds under its child tables.
    pub const HELP: &'static str = "\
Three stages run in lifecycle order: `before-build` produces declared site inputs before source
discovery, `generate-outputs` produces declared final outputs after the site program compiles, and
`after-publish` consumes a committed revision through a read-only view.

Commands run from the site root and finish before their stage continues. Each receives
`TOLA_HOOK_STAGE`, `TOLA_BUILD_MODE` (`dev` or `prod`), `TOLA_HOOK_CACHE_DIR` for results kept
between builds, and `TOLA_HOOK_TEMP_DIR`, which `TMPDIR`, `TMP`, and `TEMP` also name.
`TOLA_HOOK_INPUT_DIR` names what the stage reads; `generate-outputs` also gets
`TOLA_HOOK_OUTPUT_DIR`, the private directory it writes, whose paths are relative to the site
output. A variable a stage does not define is absent, never inherited.

A command is one finite argv array, run without a shell. A hook names a command to run, so the
sequencing, conditions, and tooling belong to the site's own runner: declare
`command = [\"just\", \"css\"]` and let that recipe decide how the stylesheet is built. Nothing
Tola does depends on `just` — it is only what the scaffold uses, and the examples below read from
one `justfile` at the site root.

Each stage's page shows one worked example. Write one stage at a time:

```toml
[[build.hooks.before-build]]
name = \"tailwind\"
command = [\"just\", \"css\"]
generates = [\"static/web-assets/tailwind-output/site.css\"]

[[build.hooks.generate-outputs]]
name = \"search\"
command = [\"just\", \"search\"]
outputs = [{ tree = \"assets/pagefind-search\" }]
```

These are trusted scripts: Tola validates declared paths, and a superseded development build
cancels its pre-publication commands, but nothing undoes their side effects.";
}

/// Chain one projection over the three hook stages in lifecycle order.
macro_rules! hook_stages {
    ($config:expr, keep $keep:expr, map $projection:expr) => {
        $config
            .before_build
            .iter()
            .filter($keep)
            .map($projection)
            .chain(
                $config
                    .generate_outputs
                    .iter()
                    .filter($keep)
                    .map($projection),
            )
            .chain($config.after_publish.iter().filter($keep).map($projection))
    };
}

impl HooksConfig {
    /// Enabled command arguments in lifecycle order, borrowed without copying.
    pub fn enabled_commands(&self) -> impl Iterator<Item = &[String]> {
        hook_stages!(
            self,
            keep | hook | hook.enable,
            map | hook | hook.command.as_slice()
        )
    }

    /// Additional literal paths whose edits rerun the commands the development session runs.
    pub fn development_rerun_paths(&self) -> impl Iterator<Item = &Path> {
        hook_stages!(
            self,
            keep | hook | hook.enable && hook.dev.participates_in_development(),
            map | hook | hook.rerun_on.iter()
        )
        .flatten()
        .map(PathBuf::as_path)
    }

    pub fn validate(&self, diag: &mut ConfigDiagnostics) {
        let mut named = Vec::new();
        for (index, hook) in self.before_build.iter().enumerate() {
            if !hook.enable {
                continue;
            }
            let fields = HookEntryFields::BEFORE_BUILD;
            diag.with_array_element(Self::FIELDS.before_build.as_str(), index, |diag| {
                validate_command(
                    &hook.name,
                    &hook.command,
                    &hook.rerun_on,
                    index,
                    fields,
                    diag,
                );
                validate_name_is_unique(&mut named, &hook.name, index, fields.name, diag);
                for (entry_index, generated) in hook.generates.iter().enumerate() {
                    validate_site_relative(
                        generated,
                        index,
                        entry_index,
                        BeforeBuildHookConfig::FIELDS.generates,
                        diag,
                    );
                }
            });
        }

        let outputs = OutputCommandConfig::FIELDS.outputs;
        let mut named = Vec::new();
        for (index, command) in self.generate_outputs.iter().enumerate() {
            if !command.enable {
                continue;
            }
            let fields = HookEntryFields::GENERATE_OUTPUTS;
            diag.with_array_element(Self::FIELDS.generate_outputs.as_str(), index, |diag| {
                validate_command(
                    &command.name,
                    &command.command,
                    &command.rerun_on,
                    index,
                    fields,
                    diag,
                );
                validate_name_is_unique(&mut named, &command.name, index, fields.name, diag);
                if command.outputs.is_empty() {
                    diag.error_with_help(
                        outputs,
                        format!("`{}` is empty", message_key(outputs, index)),
                        "declare each output with `{ file = … }` or `{ tree = … }`",
                    );
                }
                let mut paths = std::collections::BTreeSet::new();
                for output in &command.outputs {
                    let path = output.path();
                    if path.is_reserved_for_non_system_output() {
                        diag.error_with_help(
                            outputs,
                            format!("`{path}` is reserved"),
                            "choose a path outside `_tola`",
                        );
                    }
                    if !paths.insert(path) {
                        diag.error_with_help(
                            outputs,
                            format!("`{}` declares `{path}` twice", message_key(outputs, index)),
                            "keep one entry",
                        );
                    }
                }
            });
        }

        let mut named = Vec::new();
        for (index, hook) in self.after_publish.iter().enumerate() {
            if !hook.enable {
                continue;
            }
            let fields = HookEntryFields::AFTER_PUBLISH;
            diag.with_array_element(Self::FIELDS.after_publish.as_str(), index, |diag| {
                validate_command(
                    &hook.name,
                    &hook.command,
                    &hook.rerun_on,
                    index,
                    fields,
                    diag,
                );
                validate_name_is_unique(&mut named, &hook.name, index, fields.name, diag);
            });
        }
    }
}

/// The declared field paths of one entry, per hook stage.
///
/// A path names a key inside an entry, so a diagnostic about it points at the entry the author
/// must edit; [`message_key`] spells the same field with the entry's index.
#[derive(Clone, Copy)]
struct HookEntryFields {
    name: FieldPath,
    command: FieldPath,
    rerun_on: FieldPath,
}

impl HookEntryFields {
    const BEFORE_BUILD: Self = Self {
        name: BeforeBuildHookConfig::FIELDS.name,
        command: BeforeBuildHookConfig::FIELDS.command,
        rerun_on: BeforeBuildHookConfig::FIELDS.rerun_on,
    };

    const GENERATE_OUTPUTS: Self = Self {
        name: OutputCommandConfig::FIELDS.name,
        command: OutputCommandConfig::FIELDS.command,
        rerun_on: OutputCommandConfig::FIELDS.rerun_on,
    };

    const AFTER_PUBLISH: Self = Self {
        name: AfterPublishHookConfig::FIELDS.name,
        command: AfterPublishHookConfig::FIELDS.command,
        rerun_on: AfterPublishHookConfig::FIELDS.rerun_on,
    };
}

/// The key one entry's field writes, as messages spell it: `build.hooks.before-build[1].command`.
///
/// A declared field path names the field after the array's own path, so the entry's index joins
/// them where the document writes it.
fn message_key(field: FieldPath, index: usize) -> String {
    match field.as_str().rsplit_once('.') {
        Some((array, name)) => format!("{array}[{index}].{name}"),
        None => field.as_str().to_owned(),
    }
}

/// Generates declared site inputs before source discovery, so Typst and configured assets can
/// read them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Config)]
#[serde(default, deny_unknown_fields)]
#[config(section = "build.hooks.before-build", collection = array_table)]
pub struct BeforeBuildHookConfig {
    /// Whether this entry runs. A disabled entry reserves nothing, so another hook may
    /// declare the same paths.
    pub enable: bool,
    /// Names this entry, so status lines and diagnostics can point at it: one word without
    /// spaces, unique within its stage.
    pub name: String,
    /// Executable followed by arguments, run from the site root without an implicit shell.
    /// The command writes declared site inputs directly and reads no build directory, so
    /// no input or output directory variable is set.
    #[config(collection = inline)]
    pub command: Vec<String>,
    /// Whether `tola dev` runs this entry: `"run"` (the default) or `"skip"`.
    #[config(values = DevParticipation::VALUES)]
    pub dev: DevParticipation,
    /// Extra files or directories, relative to the site root, whose edits trigger a `tola dev`
    /// build, not only this entry. The field neither filters command execution nor caches
    /// command results.
    #[serde(rename = "rerun-on")]
    #[config(collection = inline)]
    pub rerun_on: Vec<PathBuf>,
    /// Files or directories this entry generates, relative to the site root;
    /// `build.publish-dir` and `.tola` are off limits. Tola checks that each declared path exists
    /// after the command and watches it during `tola dev`. Neither declaring nor reading its bytes
    /// publishes it: publish it through `[assets]` or a Bundle `asset(...)`, or use it to produce a
    /// document or another output.
    #[config(collection = inline)]
    pub generates: Vec<PathBuf>,
}

impl BeforeBuildHookConfig {
    /// What `tola help "[[build.hooks.before-build]]"` adds under its table.
    pub const HELP: &'static str = "\
A tool that runs before source discovery can produce a site input; here `just css` compiles the
site's Tailwind stylesheet with the version the site pinned:

```toml
[[build.hooks.before-build]]
name = \"tailwind\"
command = [\"just\", \"css\"]
generates = [\"static/web-assets/tailwind-output/site.css\"]
rerun-on = [\"static/tailwind-sources\", \"deno.json\", \"deno.lock\", \"justfile\"]
```

`css` is a recipe in the site's `justfile`, and `tailwind` is only the name this entry runs under.
Tola runs the argv, so any runner works. `rerun-on` covers what the tool reads but the build never
does — the stylesheet source and the runner's own files — while `generates` is the input Tola
checks after the command and watches during `tola dev`.";
}

/// Produces declared final output files after the site program compiles, adding them to the
/// candidate output graph before reference checks. They are unavailable to the preceding Typst
/// compilation, but pages may link their known URLs without an `[assets]` declaration, and
/// reference checks validate those links.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Config)]
#[serde(default, deny_unknown_fields)]
#[config(section = "build.hooks.generate-outputs", collection = array_table)]
pub struct OutputCommandConfig {
    /// Whether this entry runs. A disabled entry reserves nothing, so another hook may
    /// declare the same paths.
    pub enable: bool,
    /// Names this entry, so status lines and diagnostics can point at it: one word without
    /// spaces, unique within its stage.
    pub name: String,
    /// Executable followed by arguments, run from the site root without an implicit shell. Read
    /// the upstream snapshot below `TOLA_HOOK_INPUT_DIR` and write declared outputs below
    /// `TOLA_HOOK_OUTPUT_DIR`; written paths are relative to the final site output.
    #[config(collection = inline)]
    pub command: Vec<String>,
    /// Whether `tola dev` runs this entry: `"run"` (the default) or `"skip"`.
    #[config(values = DevParticipation::VALUES)]
    pub dev: DevParticipation,
    /// Extra files or directories, relative to the site root, whose edits trigger a `tola dev`
    /// build, not only this entry. The field neither filters command execution nor caches
    /// command results.
    #[serde(rename = "rerun-on")]
    #[config(collection = inline)]
    pub rerun_on: Vec<PathBuf>,
    /// Final site output paths this entry adds to the candidate, relative to the site output
    /// root, written as `{ file = "index.json" }` or `{ tree = "search" }`. Required: the
    /// written files must match these declarations, every path stays outside `_tola`, and a
    /// path another producer owns fails the build instead of replacing that output.
    #[config(collection = inline)]
    pub outputs: Vec<CommandOutput>,
}

impl OutputCommandConfig {
    /// What `tola help "[[build.hooks.generate-outputs]]"` adds under its table.
    pub const HELP: &'static str = "\
A search index built from the compiled site joins the published output directly:

```toml
[[build.hooks.generate-outputs]]
name = \"search\"
command = [\"just\", \"search\"]
outputs = [{ tree = \"assets/pagefind-search\" }]
rerun-on = [\"justfile\"]
```

```just
search:
    pagefind --site \"$TOLA_HOOK_INPUT_DIR\" \\
        --output-path \"$TOLA_HOOK_OUTPUT_DIR/assets/pagefind-search\"
```

The declared tree needs no `[assets]` entry, and only this hook owns that path. Link it with
`output-to-url(\"assets/pagefind-search/pagefind-ui.css\")`: `asset-url` answers for `[assets]`
declarations alone. No cache busting either — these bytes appear after the site compiles, so the
command names them itself; Pagefind hashes its data files while pages keep linking `pagefind.js`.

`search` is a recipe in the site's `justfile`, doing exactly the two lines above.";
}

/// Consumes one committed revision through a read-only view, without declaring further site
/// outputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Config)]
#[serde(default, deny_unknown_fields)]
#[config(section = "build.hooks.after-publish", collection = array_table)]
pub struct AfterPublishHookConfig {
    /// Whether this entry runs.
    pub enable: bool,
    /// Names this entry, so status lines and diagnostics can point at it: one word without
    /// spaces, unique within its stage.
    pub name: String,
    /// Executable followed by arguments, run from the site root without an implicit shell.
    /// `TOLA_HOOK_INPUT_DIR` is a read-only temporary view of the published site, so a failure
    /// here cannot undo publication.
    #[config(collection = inline)]
    pub command: Vec<String>,
    /// Whether `tola dev` runs this entry: `"skip"` (the default) or `"run"`.
    #[config(values = DevParticipation::VALUES)]
    pub dev: DevParticipation,
    /// Extra files or directories, relative to the site root, whose edits trigger a `tola dev`
    /// build, not only this entry. The field neither filters command execution nor caches
    /// command results.
    #[serde(rename = "rerun-on")]
    #[config(collection = inline)]
    pub rerun_on: Vec<PathBuf>,
}

impl AfterPublishHookConfig {
    /// What `tola help "[[build.hooks.after-publish]]"` adds under its table.
    pub const HELP: &'static str = "\
A consumer reads the committed revision through `TOLA_HOOK_INPUT_DIR`, which no later build
changes:

```toml
[[build.hooks.after-publish]]
name = \"deploy\"
command = [\"just\", \"deploy\"]
```

`deploy` is a recipe in the site's `justfile`: it can upload that view or notify a host that a new
revision is live. A failure is reported without undoing the publication.";
}

/// One declared generated file or exclusive generated directory tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CommandOutput {
    /// One exact output file.
    File(tola_address::OutputPath),
    /// An exclusively owned directory whose files may vary between invocations.
    Tree(tola_address::OutputPath),
}

impl CommandOutput {
    /// Logical path of the declared file or directory.
    pub fn path(&self) -> &tola_address::OutputPath {
        match self {
            Self::File(path) | Self::Tree(path) => path,
        }
    }
}

impl Default for BeforeBuildHookConfig {
    fn default() -> Self {
        Self {
            enable: true,
            name: String::new(),
            command: Vec::new(),
            dev: DevParticipation::Run,
            rerun_on: Vec::new(),
            generates: Vec::new(),
        }
    }
}

impl Default for OutputCommandConfig {
    fn default() -> Self {
        Self {
            enable: true,
            name: String::new(),
            command: Vec::new(),
            dev: DevParticipation::Run,
            rerun_on: Vec::new(),
            outputs: Vec::new(),
        }
    }
}

impl Default for AfterPublishHookConfig {
    fn default() -> Self {
        Self {
            enable: true,
            name: String::new(),
            command: Vec::new(),
            dev: DevParticipation::Skip,
            rerun_on: Vec::new(),
        }
    }
}

fn validate_command(
    name: &str,
    command: &[String],
    rerun_on: &[PathBuf],
    index: usize,
    fields: HookEntryFields,
    diag: &mut ConfigDiagnostics,
) {
    let key = message_key(fields.command, index);
    if command.is_empty() {
        diag.error_with_help(
            fields.command,
            format!("`{key}` is empty"),
            "name the executable first, then its arguments",
        );
    } else if command[0].trim().is_empty() {
        diag.error_with_help(
            fields.command,
            format!("`{key}[0]` is blank"),
            "name the executable first, then its arguments",
        );
    }
    validate_name(name, index, fields.name, diag);
    for (entry_index, path) in rerun_on.iter().enumerate() {
        validate_site_relative(path, index, entry_index, fields.rerun_on, diag);
    }
}

pub(crate) fn hook_identity(stage: HookStage, name: &str) -> String {
    format!("the `{name}` hook in `build.hooks.{}`", stage.as_str())
}

fn validate_name(name: &str, index: usize, field: FieldPath, diag: &mut ConfigDiagnostics) {
    let key = message_key(field, index);
    if name.trim().is_empty() {
        diag.error_with_help(
            field,
            format!("`{key}` is blank"),
            "name the command, such as `name = \"css-build\"`",
        );
    } else if name.chars().any(char::is_whitespace) {
        diag.error_with_help(
            field,
            format!("`{key}` is not one word"),
            "Remove the whitespace, as in `css-build`",
        );
    }
}

fn validate_name_is_unique<'a>(
    named: &mut Vec<(&'a str, usize)>,
    name: &'a str,
    index: usize,
    field: FieldPath,
    diag: &mut ConfigDiagnostics,
) {
    if name.trim().is_empty() {
        return;
    }
    if let Some((_, first)) = named.iter().find(|(seen, _)| *seen == name) {
        diag.error_with_help(
            field,
            format!(
                "`{}` repeats `{}`",
                message_key(field, index),
                message_key(field, *first)
            ),
            "give each entry its own name",
        );
        return;
    }
    named.push((name, index));
}

fn validate_site_relative(
    path: &Path,
    hook_index: usize,
    entry_index: usize,
    field: FieldPath,
    diag: &mut ConfigDiagnostics,
) {
    let key = format!("{}[{entry_index}]", message_key(field, hook_index));
    // A hook names a file or directory: the site root itself is not one.
    if path == Path::new(".") {
        diag.error_with_help(
            field,
            format!("`{key}` is `.`"),
            "name a file or directory inside the site root",
        );
        return;
    }
    crate::config::section::path::validate_site_relative_path(path, &key, field, diag);
}

/// Point in the build lifecycle at which a hook executes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookStage {
    BeforeBuild,
    GenerateOutputs,
    AfterPublish,
}

impl HookStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BeforeBuild => "before-build",
            Self::GenerateOutputs => "generate-outputs",
            Self::AfterPublish => "after-publish",
        }
    }
}

/// Whether an enabled finite command also runs in development.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DevParticipation {
    /// Run the command on every build that reaches its stage.
    #[default]
    Run,
    /// Leave the command out of the development session.
    ///
    /// Production builds still run it.
    Skip,
}

impl DevParticipation {
    const VALUES: &'static [&'static str] = &["\"run\"", "\"skip\""];

    /// Whether a development session runs this command at all.
    pub const fn participates_in_development(self) -> bool {
        !matches!(self, Self::Skip)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::{attached_diagnostics, diagnostic_mentioning};

    #[test]
    fn hook_template_preserves_stage_entries() {
        let config: crate::config::SiteConfigSchema = toml::from_str(
            r#"
[[build.hooks.before-build]]
name = "icons"
command = ["./scripts/icons.sh", "generated/icons"]
rerun-on = ["src/icons"]
generates = ["generated/icons"]

[[build.hooks.generate-outputs]]
name = "search"
command = ["node", "scripts/search.mjs"]
dev = "skip"
rerun-on = ["scripts/search.mjs"]
outputs = [{ file = "index.json" }, { tree = "search" }]

[[build.hooks.after-publish]]
name = "deploy"
command = ["./scripts/deploy.sh"]
dev = "run"
rerun-on = ["scripts/deploy.sh"]
"#,
        )
        .unwrap();
        let template =
            super::super::BuildSectionConfig::try_template_with_header_from(&config.build).unwrap();
        let decoded: crate::config::SiteConfigSchema = toml::from_str(&template).unwrap();

        assert_eq!(decoded.build.hooks, config.build.hooks);
    }

    #[test]
    fn development_inputs_match_participation() {
        let config: HooksConfig = toml::from_str(
            r#"
[[before-build]]
name = "prepare"
command = ["prepare"]
rerun-on = [" before "]

[[before-build]]
name = "production-prepare"
command = ["production-prepare"]
dev = "skip"
rerun-on = ["production-input"]

[[before-build]]
name = "disabled-prepare"
command = ["disabled-prepare"]
enable = false
rerun-on = ["disabled-input"]

[[generate-outputs]]
name = "generate"
command = ["generate"]
rerun-on = ["generated/*.json"]
outputs = [{ file = "search.json" }]

[[generate-outputs]]
name = "disabled-generate"
command = ["disabled-generate"]
enable = false
rerun-on = ["disabled-generated"]

[[after-publish]]
name = "production-consumer"
command = ["production-consumer"]
rerun-on = ["production-consumer-input"]

[[after-publish]]
name = "development-consumer"
command = ["development-consumer"]
dev = "run"
rerun-on = ["consumer"]

[[after-publish]]
name = "disabled-consumer"
command = ["disabled-consumer"]
enable = false
dev = "run"
rerun-on = ["disabled-consumer-input"]
"#,
        )
        .unwrap();
        assert_eq!(
            config.development_rerun_paths().collect::<Vec<_>>(),
            [" before ", "generated/*.json", "consumer"].map(Path::new)
        );
    }

    #[test]
    fn disabled_commands_are_not_validated() {
        let mut config: HooksConfig = toml::from_str(
            r#"
[[before-build]]
enable = false
command = [" "]
generates = ["../source"]

[[generate-outputs]]
enable = false
name = " "
rerun-on = ["/outside"]

[[after-publish]]
enable = false
rerun-on = ["../committed"]
"#,
        )
        .unwrap();
        let mut diagnostics = ConfigDiagnostics::new();
        config.validate(&mut diagnostics);
        crate::config::loading::validate_hook_output_boundaries(
            &config,
            Path::new("/site"),
            Path::new("/site/tola.toml"),
            Path::new("/site/public"),
            None,
            &mut diagnostics,
        );
        assert!(diagnostics.into_result().is_ok());

        config.before_build[0].enable = true;
        config.generate_outputs[0].enable = true;
        config.after_publish[0].enable = true;
        let mut diagnostics = ConfigDiagnostics::new();
        config.validate(&mut diagnostics);
        for field in [
            BeforeBuildHookConfig::FIELDS.command,
            BeforeBuildHookConfig::FIELDS.generates,
            OutputCommandConfig::FIELDS.name,
            OutputCommandConfig::FIELDS.rerun_on,
            AfterPublishHookConfig::FIELDS.rerun_on,
        ] {
            assert!(
                diagnostics
                    .errors()
                    .iter()
                    .any(|error| error.field == field)
            );
        }
    }

    fn input_generator(name: &str, output: &str) -> BeforeBuildHookConfig {
        BeforeBuildHookConfig {
            name: name.into(),
            command: vec!["generator".into()],
            generates: vec![output.into()],
            ..BeforeBuildHookConfig::default()
        }
    }

    #[test]
    fn stages_require_their_output_shape() {
        for source in [
            "[[before-build]]\ngenerates = [{ file = \"index.json\" }]\n",
            "[[generate-outputs]]\noutputs = [\"index.json\"]\n",
            "[[after-publish]]\noutputs = [\"index.json\"]\n",
        ] {
            assert!(toml::from_str::<HooksConfig>(source).is_err(), "{source}");
        }
    }

    #[test]
    fn rerun_paths_stay_inside_the_site() {
        let config: HooksConfig = toml::from_str(
            r#"
[[before-build]]
name = "echo-bad"
command = ["echo", "bad"]
rerun-on = ["/src", "../templates"]
"#,
        )
        .unwrap();
        let mut diag = ConfigDiagnostics::new();

        config.validate(&mut diag);

        assert_eq!(diag.errors().len(), 2);
        assert!(
            diag.errors()
                .iter()
                .all(|error| error.field == BeforeBuildHookConfig::FIELDS.rerun_on),
            "{:?}",
            diag.errors()
        );
    }

    #[test]
    fn enabled_commands_need_executable() {
        for command in [Vec::new(), vec!["  ".into()]] {
            let hooks = HooksConfig {
                before_build: vec![BeforeBuildHookConfig {
                    name: "generator".into(),
                    command,
                    ..BeforeBuildHookConfig::default()
                }],
                ..HooksConfig::default()
            };
            let mut diagnostics = ConfigDiagnostics::new();

            hooks.validate(&mut diagnostics);

            assert_eq!(diagnostics.errors().len(), 1);
            assert_eq!(
                diagnostics.errors()[0].field,
                BeforeBuildHookConfig::FIELDS.command
            );
        }
    }

    /// One enabled hook in each stage, every one named `name` and running `command`.
    fn one_hook_per_stage(name: &str, command: &str) -> HooksConfig {
        HooksConfig {
            before_build: vec![BeforeBuildHookConfig {
                name: name.into(),
                command: vec![command.into()],
                generates: vec!["generated".into()],
                ..BeforeBuildHookConfig::default()
            }],
            generate_outputs: vec![OutputCommandConfig {
                name: name.into(),
                command: vec![command.into()],
                outputs: vec![CommandOutput::File(
                    tola_address::OutputPath::parse("search.json").unwrap(),
                )],
                ..OutputCommandConfig::default()
            }],
            after_publish: vec![AfterPublishHookConfig {
                name: name.into(),
                command: vec![command.into()],
                ..AfterPublishHookConfig::default()
            }],
        }
    }

    #[test]
    fn enabled_names_require_one_word() {
        for name in ["", " ", "build stylesheet", "build\tstylesheet"] {
            let hooks = one_hook_per_stage(name, "generator");
            let mut diagnostics = ConfigDiagnostics::new();

            hooks.validate(&mut diagnostics);

            assert_eq!(
                diagnostics
                    .errors()
                    .iter()
                    .map(|error| error.field)
                    .collect::<Vec<_>>(),
                [
                    BeforeBuildHookConfig::FIELDS.name,
                    OutputCommandConfig::FIELDS.name,
                    AfterPublishHookConfig::FIELDS.name,
                ],
                "{name:?}",
            );
        }
    }

    #[test]
    fn names_are_unique_within_stage() {
        let mut hooks = one_hook_per_stage("styles", "generator");
        let mut diagnostics = ConfigDiagnostics::new();
        hooks.validate(&mut diagnostics);
        assert!(diagnostics.errors().is_empty());

        hooks.before_build.push(hooks.before_build[0].clone());
        hooks
            .generate_outputs
            .push(hooks.generate_outputs[0].clone());
        hooks.after_publish.push(hooks.after_publish[0].clone());
        let mut diagnostics = ConfigDiagnostics::new();
        hooks.validate(&mut diagnostics);
        assert_eq!(
            diagnostics
                .errors()
                .iter()
                .map(|error| error.field)
                .collect::<Vec<_>>(),
            [
                BeforeBuildHookConfig::FIELDS.name,
                OutputCommandConfig::FIELDS.name,
                AfterPublishHookConfig::FIELDS.name,
            ],
        );
    }

    /// The 1-based line of the last line `source` writes as `key`.
    fn last_written_line(source: &str, key: &str) -> usize {
        let lines = source.lines().collect::<Vec<_>>();
        lines.iter().rposition(|line| *line == key).unwrap() + 1
    }

    /// The location of the single diagnostic settings validation reports for `source`.
    fn reported_location(source: &str) -> (usize, String) {
        let parsed = crate::config::ConfigSource::parse(
            Path::new("/site/tola.toml"),
            source,
            crate::resources::InputScope::Online,
        )
        .unwrap()
        .decode_site()
        .unwrap();
        let error = parsed.validate_settings().unwrap_err();
        let diagnostics = attached_diagnostics(&error);
        let [diagnostic] = diagnostics else {
            panic!("expected one diagnostic for {source}: {diagnostics:#?}");
        };
        let location = diagnostic.location.as_ref().unwrap();
        (
            location.line.unwrap(),
            location.source_lines[0].text.clone(),
        )
    }

    #[test]
    fn later_entry_error_names_its_own_entry() {
        // The last case writes no `name` at all: a field the entry does not write points at the
        // entry's own header, never at the first entry of its array.
        for (source, key) in [
            (
                r#"
[[build.hooks.before-build]]
name = "first"
command = ["first.sh"]
generates = ["generated/first"]

[[build.hooks.before-build]]
name = "second"
command = []
generates = ["generated/second"]
"#,
                "command = []",
            ),
            (
                r#"
[[build.hooks.generate-outputs]]
name = "first"
command = ["first.sh"]
outputs = [{ file = "first.json" }]

[[build.hooks.generate-outputs]]
name = "second"
command = []
outputs = [{ file = "second.json" }]
"#,
                "command = []",
            ),
            (
                r#"
[[build.hooks.after-publish]]
name = "first"
command = ["first.sh"]

[[build.hooks.after-publish]]
name = "second"
command = []
"#,
                "command = []",
            ),
            (
                r#"
[[build.hooks.before-build]]
name = "styles"
command = ["first.sh"]
generates = ["generated/first"]

[[build.hooks.before-build]]
name = "styles"
command = ["second.sh"]
generates = ["generated/second"]
"#,
                r#"name = "styles""#,
            ),
            (
                r#"
[[build.hooks.before-build]]
name = "first"
command = ["first.sh"]
generates = ["generated/first"]

[[build.hooks.before-build]]
command = ["second.sh"]
generates = ["generated/second"]
"#,
                "[[build.hooks.before-build]]",
            ),
        ] {
            assert_eq!(
                reported_location(source),
                (last_written_line(source, key), key.to_owned()),
                "{source}"
            );
        }
    }

    /// Assert that a load of one schema whose single before-build hook declares `declared`
    /// reports it as a refused hook output.
    fn assert_hook_output_refused(root: &Path, declared: &str) {
        let mut schema = crate::config::SiteConfigSchema::default();
        schema.vendor.path = Some("vendor".into());
        schema.build.hooks.before_build = vec![input_generator("generator", declared)];

        let error = schema
            .resolve(
                &root.join("tola.toml"),
                tola_typst::PackageLocations::default(),
                &crate::config::loading::BuildOverrides::default(),
            )
            .unwrap_err();

        let diagnostic = diagnostic_mentioning(&error, declared);
        assert_eq!(diagnostic.code, crate::codes::config::INVALID);
        assert_eq!(
            diagnostic.notes,
            [BeforeBuildHookConfig::FIELDS.generates.as_str()]
        );
    }

    #[test]
    fn hook_outputs_cannot_reach_owned_roots() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        for declared in [
            "public/assets",
            ".tola/cache",
            ".vendor-vendor",
            ".vendor-vendor/marker.txt",
        ] {
            assert_hook_output_refused(root, declared);
        }

        #[cfg(unix)]
        {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path();
            let workspace = root.join(".vendor-vendor");
            std::fs::create_dir(&workspace).unwrap();
            std::os::unix::fs::symlink(&workspace, root.join("vendored")).unwrap();
            assert_hook_output_refused(root, "vendored/marker.txt");
        }
    }

    #[test]
    fn one_hook_owns_each_output_path() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let hooks = HooksConfig {
            before_build: vec![
                input_generator("directory", "generated"),
                input_generator("file", "generated/site.css"),
            ],
            ..HooksConfig::default()
        };
        let mut diag = ConfigDiagnostics::new();

        crate::config::loading::validate_hook_output_boundaries(
            &hooks,
            root,
            &root.join("tola.toml"),
            &root.join("public"),
            None,
            &mut diag,
        );

        assert_eq!(diag.errors().len(), 1);
        assert_eq!(
            diag.errors()[0].field,
            BeforeBuildHookConfig::FIELDS.generates
        );
    }
}
