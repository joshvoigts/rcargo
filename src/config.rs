use serde::Deserialize;
use std::error::Error;
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize, Default)]
pub struct Config {
  /// The target host to deploy to (anything you'd pass to `ssh`)
  pub target: String,

  /// Remote path for the repo. Defaults to `$HOME/build/{project_name}`
  #[serde(default)]
  pub remote_path: Option<String>,

  /// Sandbox configuration for remote builds
  #[serde(default)]
  pub sandbox: Sandbox,

  /// Shell hooks that run on the remote host outside the sandbox.
  #[serde(default)]
  pub hooks: Hooks,
}

#[derive(Debug, Deserialize, Default)]
pub struct Hooks {
  /// Command(s) to run after sync but before the sandboxed build.
  #[serde(default)]
  pub prebuild: Option<Hook>,
}

/// A hook command. Accepts either a single string or a list of strings.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Hook {
  Single(String),
  List(Vec<String>),
}

impl Hook {
  pub fn as_command(&self) -> String {
    match self {
      Hook::Single(s) => s.clone(),
      Hook::List(v) => v.join(" && "),
    }
  }
}

#[derive(Debug, Deserialize)]
pub struct Sandbox {
  #[serde(default = "default_true")]
  pub enabled: bool,

  #[serde(default)]
  pub allow: SandboxAllow,

  #[serde(default)]
  pub env: std::collections::HashMap<String, String>,
}

impl Default for Sandbox {
  fn default() -> Self {
    Self {
      enabled: true,
      allow: Default::default(),
      env: Default::default(),
    }
  }
}

fn default_true() -> bool {
  true
}

#[derive(Debug, Deserialize, Default)]
pub struct SandboxAllow {
  #[serde(default)]
  pub write: Vec<String>,

  #[serde(default)]
  pub net: Vec<String>,
}

impl Config {
  pub fn load() -> Result<Self, Box<dyn Error>> {
    let candidates = ["deploy.toml", "rdeploy.toml"];
    let config_path =
      candidates.iter().map(Path::new).find(|p| p.exists());
    let config_path = match config_path {
      Some(p) => p,
      None => {
        return Err(
          "No config file found. Create deploy.toml with:\ntarget = \"<ssh_target>\""
            .into(),
        );
      }
    };
    let content = fs::read_to_string(config_path)?;
    let config: Config = toml::from_str(&content)?;
    Ok(config)
  }

  pub fn remote_path(&self, project_name: &str) -> String {
    self
      .remote_path
      .clone()
      .unwrap_or_else(|| format!("$HOME/build/{project_name}"))
  }
}
