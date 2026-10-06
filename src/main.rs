//! Static site generation with Typst Bundles.

// Passing Typst Bundles across `tokio::spawn_blocking` requires deeper `Send`
// auto-trait resolution than rustc's default (rust-lang/rust#159228).
#![recursion_limit = "256"]

mod cancellation;
mod cli;
mod codes;
mod config;
mod dev;
mod editor;
mod embed;
mod help;
mod i18n;
mod sys;
mod terminal;
mod writes;

fn main() -> std::process::ExitCode {
    cli::run()
}
