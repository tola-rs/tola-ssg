//! The declared initializer source and an optional already-selected snapshot.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use tola_input::{LockedModule, TarballLocation};
use tola_module::{ModuleDeclaration, ModuleSource};

use crate::cli::args::InitArgs;

pub(in crate::cli) struct Source {
    pub declaration: ModuleSource,
    pub cached: Option<(PathBuf, LockedModule)>,
}

impl Source {
    pub(super) fn from_args(args: &InitArgs) -> Result<Option<Self>> {
        let Some(from) = &args.from else {
            return Ok(None);
        };
        let path = Path::new(from);
        let text = from.to_str();
        let url = text.and_then(|text| url::Url::parse(text).ok());
        let remote = !path.is_dir()
            && text.is_some_and(|text| {
                text.contains("://")
                    || !path.is_absolute()
                        && !text.starts_with("./")
                        && !text.starts_with("../")
                        && text.contains('@')
                        && text.contains(':')
            });
        let filename = url.as_ref().map(|url| url.path()).or(text).unwrap_or("");
        let archive = !path.is_dir()
            && [".tar.gz", ".tar.zst", ".zip"]
                .iter()
                .any(|suffix| filename.to_ascii_lowercase().ends_with(suffix));
        let git = remote && !archive;
        ensure!(
            git || (args.branch.is_none()
                && args.tag.is_none()
                && args.rev.is_none()
                && args.subdir.is_none()),
            "`--branch`, `--tag`, `--rev`, and `--subdir` require a Git initializer source"
        );
        let declaration = if !remote && !archive {
            ModuleSource::Local {
                path: std::path::absolute(path)?,
            }
        } else {
            let mut fields = toml::Table::new();
            if archive {
                if remote {
                    fields.insert(
                        "url".into(),
                        text.context("initializer URL must be UTF-8")?.into(),
                    );
                } else {
                    fields.insert(
                        "path".into(),
                        toml::Value::try_from(std::path::absolute(path)?)?,
                    );
                }
            } else {
                fields.insert(
                    "url".into(),
                    text.context("initializer URL must be UTF-8")?.into(),
                );
                for (key, value) in [
                    ("branch", &args.branch),
                    ("tag", &args.tag),
                    ("rev", &args.rev),
                ] {
                    if let Some(value) = value {
                        fields.insert(key.into(), value.clone().into());
                    }
                }
                if let Some(path) = &args.subdir {
                    fields.insert("subdir".into(), toml::Value::try_from(path)?);
                }
            }
            let key = if archive { "tarball" } else { "git" };
            let module: ModuleDeclaration = toml::Value::Table(toml::Table::from_iter([(
                key.into(),
                toml::Value::Table(fields),
            )]))
            .try_into()
            .context("invalid initializer source")?;
            module.source
        };
        Ok(Some(Self {
            declaration,
            cached: None,
        }))
    }

    pub(in crate::cli) fn selected(
        source: &ModuleSource,
        site: &Path,
        directory: &Path,
        locked: Option<LockedModule>,
    ) -> Self {
        let mut declaration = source.clone();
        match &mut declaration {
            ModuleSource::Local { path } => *path = site.join(&*path),
            ModuleSource::Tarball { tarball } => {
                if let TarballLocation::File { path } = &mut tarball.location {
                    *path = site.join(&*path);
                }
            }
            ModuleSource::Git { .. } => {}
        }
        let cached = if matches!(declaration, ModuleSource::Local { .. }) {
            None
        } else {
            locked.map(|locked| (directory.to_owned(), locked))
        };
        Self {
            declaration,
            cached,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Commands};
    use clap::Parser;

    fn source(arguments: &[&str]) -> Result<Source> {
        let Commands::Init(args) = Cli::try_parse_from(arguments)?.command else {
            unreachable!()
        };
        Source::from_args(&args)?.context("missing source")
    }

    #[test]
    fn sources_preserve_selectors_and_reject_mixed_kinds() {
        let selected = source(&[
            "tola",
            "init",
            "--from",
            "https://github.com/example/theme",
            "--branch",
            "preview",
            "--subdir",
            "theme",
        ])
        .unwrap();
        let ModuleSource::Git { git } = selected.declaration else {
            panic!("expected Git source")
        };
        assert_eq!(git.subdir, Path::new("theme"));
        assert_eq!(
            git.revision,
            Some(tola_module::declaration::RemoteRevision::Branch(
                "preview".into()
            ))
        );
        assert!(matches!(
            source(&[
                "tola",
                "init",
                "--from",
                "https://example.com/theme.tar.gz?download=1"
            ])
            .unwrap()
            .declaration,
            ModuleSource::Tarball { .. }
        ));
        for source_url in ["https://example.com/theme.zip", "./theme"] {
            assert!(source(&["tola", "init", "--from", source_url, "--subdir", "theme"]).is_err());
        }
    }
}
