//! Context supplied by Tola for one hook, module command, or initializer.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use tola_input::InputScope;
use tola_subprocess::Command;

use crate::config::SiteConfig;

pub const PATH_ENV: &str = "_TOLA_CONTEXT_PATH";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    pub executable: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site: Option<Site>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<Module>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hook: Option<Hook>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub init: Option<Init>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report_path: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Site {
    pub root: PathBuf,
    pub config_path: PathBuf,
    pub content_root: PathBuf,
}

impl From<&SiteConfig> for Site {
    fn from(config: &SiteConfig) -> Self {
        Self {
            root: config.root().to_owned(),
            config_path: config.config_path().to_owned(),
            content_root: config.build().content_dir.clone(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Module {
    pub name: String,
    pub root: PathBuf,
    pub manifest: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hook {
    pub name: String,
    pub stage: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_dir: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_dir: Option<PathBuf>,
    /// Publish-relative files and trees declared for this generate-outputs hook.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<tola_module::CommandOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_dir: Option<PathBuf>,
    pub temp_dir: PathBuf,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Init {
    pub target_dir: PathBuf,
    pub output_dir: PathBuf,
    pub interactive: bool,
}

impl Context {
    pub fn new(executable: PathBuf, scope: Option<InputScope>) -> Self {
        Self {
            executable,
            input_scope: scope.map(|scope| scope.as_str().to_owned()),
            site: None,
            module: None,
            hook: None,
            init: None,
            report_path: None,
        }
    }

    pub fn read() -> Result<Option<Self>> {
        let Some(path) = std::env::var_os(PATH_ENV) else {
            return Ok(None);
        };
        let file = std::fs::File::open(path).context("could not open the Tola call context")?;
        serde_json::from_reader(file)
            .map(Some)
            .context("could not read the Tola call context")
    }

    pub fn write(&self, directory: &Path) -> Result<PathBuf> {
        let path = std::path::absolute(directory)?.join("context.json");
        let file =
            std::fs::File::create(&path).context("could not create the Tola call context")?;
        serde_json::to_writer(file, self).context("could not write the Tola call context")?;
        Ok(path)
    }

    pub fn command(&self, command: Command, directory: &Path) -> Result<Command> {
        Ok(command
            .env(PATH_ENV, self.write(directory)?)
            .env("TOLA_EXECUTABLE", &self.executable))
    }
}
