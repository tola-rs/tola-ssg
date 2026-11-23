//! Static site generation with Typst Bundles.

// Passing Typst Bundles across `tokio::spawn_blocking` requires deeper `Send`
// auto-trait resolution than rustc's default (rust-lang/rust#159228).
#![recursion_limit = "256"]

mod cancellation;
mod cli;
mod codes;
mod command_line;
mod config;
mod dev;
mod editor;
mod embed;
mod help;
mod i18n;
mod sys;
mod terminal;
mod tree;
mod writes;

fn main() -> std::process::ExitCode {
    tola_module::install_builtin_modules(&[
        tola_module::BuiltinModule {
            name: "resize-images",
            files: &[
                (
                    "tola-module.toml",
                    include_bytes!("../modules/resize-images/tola-module.toml"),
                ),
                (
                    "resize.nu",
                    include_bytes!("../modules/resize-images/resize.nu"),
                ),
            ],
        },
        tola_module::BuiltinModule {
            name: "tailwind",
            files: &[
                (
                    "tola-module.toml",
                    include_bytes!("../modules/tailwind/tola-module.toml"),
                ),
                (
                    "compile.nu",
                    include_bytes!("../modules/tailwind/compile.nu"),
                ),
                ("cache.nu", include_bytes!("../modules/tailwind/cache.nu")),
            ],
        },
    ])
    .expect("embedded site modules have valid paths");
    cli::run(&std::env::args_os().collect::<Vec<_>>())
}
