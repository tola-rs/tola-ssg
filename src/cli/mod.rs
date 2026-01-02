//! Command-line interface module.

mod args;
pub mod build;
pub mod common;
pub mod deploy;
pub mod init;
pub mod query;
pub mod serve;
pub mod validate;

pub use args::{BuildArgs, Cli, Commands, ValidateArgs};
