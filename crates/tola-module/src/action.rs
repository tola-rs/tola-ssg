//! Executable actions shared by hooks, module commands and initializers.

use std::path::{Component, PathBuf};

/// An embedded Nu script or an external executable with literal arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Script path relative to the declaration and arguments to its main command.
    Nu { path: PathBuf, args: Vec<String> },
    /// Executable followed by its arguments.
    Argv(Vec<String>),
}

impl Action {
    /// Check mutually exclusive action fields and build the selected action.
    pub fn from_fields(
        nu: Option<PathBuf>,
        argv: Option<Vec<String>>,
        args: Option<Vec<String>>,
    ) -> Result<Self, String> {
        match (nu, argv, args) {
            (Some(path), None, args) => {
                if !crate::is_relative_path(&path)
                    || !path
                        .components()
                        .any(|part| matches!(part, Component::Normal(_)))
                {
                    return Err("`nu` must name a script inside the declaring directory".into());
                }
                Ok(Self::Nu {
                    path,
                    args: args.unwrap_or_default(),
                })
            }
            (None, Some(argv), None) => {
                if argv.first().is_none_or(|program| program.trim().is_empty()) {
                    return Err("`argv` needs an executable as its first element".into());
                }
                Ok(Self::Argv(argv))
            }
            (None, Some(_), Some(_)) => Err("`args` is only available with `nu`".into()),
            _ => Err("declare exactly one of `nu` and `argv`".into()),
        }
    }
}
