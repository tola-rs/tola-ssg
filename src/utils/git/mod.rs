//! Git operations for the static site generator.
//!
//! Handles repository initialization, commits, and remote pushing.

mod ignore;
mod remote;
mod repo;
mod tree;

pub use remote::push;
pub use repo::{commit_all, create_repo, open_repo};
