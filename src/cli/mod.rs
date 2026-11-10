//! The `tola` application's command layer: parsed intent, command policy, and the terminal output
//! every command writes through.
//!
//! Modules here decide which operation runs and how its outcome reads; they own no build semantics,
//! and no domain or build module may depend on this layer. Configuration loading shared by commands
//! is `config`; process setup, cancellation, and exit status are `run`.
mod args;
mod argv;
mod color;
mod commands;
mod config;
mod dispatch;
mod external;
mod input;
mod interaction;
pub(crate) mod log;
pub(crate) mod output;
mod run;
mod usage;

pub(crate) use args::{
    BuildArgs, BuildOverrideArgs, Cli, Commands, CompilerFontArgs, CompletionShell, ConfigFileArgs,
    DevelopmentArgs, EditorCommand, HelpArgs, IconInspectArgs, InspectCommand, ServerArgs,
    SiteArgs, SourceInspectArgs, TypstPackageArgs, UpdateArgs, VendorArgs,
};
pub(crate) use commands::nu::hook_runner;
pub(crate) use run::run;
