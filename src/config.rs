use serde::Deserialize;
use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use xdg::BaseDirectories;

#[derive(Debug, Deserialize, Default)]
pub struct Config {
  /// The target host to deploy to (anything you'd pass to `ssh`)
  pub target: String,

  /// Remote path for the repo. Defaults to `$HOME/build/{project_name}`
  #[serde(default)]
  pub remote_path: Option<String>,

  /// Workspace member to install (e.g. "edwin-server")
  #[serde(default)]
  pub package: Option<String>,

  /// Binary name override (defaults to auto-detect from [[bin]] target)
  #[serde(default)]
  pub bin: Option<String>,

  /// Sandbox configuration for remote builds
  #[serde(default)]
  pub sandbox: Sandbox,

  /// Shell hooks that run on the remote host outside the sandbox.
  #[serde(default)]
  pub hooks: Hooks,

  /// User-defined commands. Maps a command name to an ordered list of
  /// steps, each a `"<cmd> [args...]"` string. e.g. `ci = ["lint", "test --workspace -q"]`
  #[serde(default)]
  pub commands: HashMap<String, Vec<String>>,
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
  /// Load the effective config: global (XDG) provides defaults, then the
  /// project `rcargo.toml` overrides it with replace semantics.
  pub fn load() -> Result<Self, Box<dyn Error>> {
    let mut found = false;
    let mut value = toml::Value::Table(toml::map::Map::new());

    if let Some(path) = global_config_path() {
      if path.exists() {
        found = true;
        value = toml::from_str(&fs::read_to_string(&path)?)?;
      }
    }

    if let Some(path) = project_config_path() {
      if path.exists() {
        found = true;
        let project: toml::Value =
          toml::from_str(&fs::read_to_string(&path)?)?;
        overlay(
          value.as_table_mut().unwrap(),
          project.as_table().unwrap(),
        );
      }
    }

    if !found {
      return Err(
        "No config file found. Create rcargo.toml with:\ntarget = \"<ssh_target>\""
          .into(),
      );
    }

    let config: Config = value.try_into()?;
    Ok(config)
  }

  pub fn remote_path(&self, project_name: &str) -> String {
    self
      .remote_path
      .clone()
      .unwrap_or_else(|| format!("$HOME/build/{project_name}"))
  }
}

/// Global config at `$XDG_CONFIG_HOME/rcargo/rcargo.toml` (default
/// `~/.config/rcargo/rcargo.toml`).
fn global_config_path() -> Option<PathBuf> {
  BaseDirectories::with_prefix("rcargo")
    .get_config_file("rcargo.toml")
}

/// Project config in the current directory. `rcargo.toml` is canonical;
/// `deploy.toml` is still accepted for backwards compatibility.
fn project_config_path() -> Option<PathBuf> {
  ["rcargo.toml", "deploy.toml"]
    .iter()
    .map(Path::new)
    .find(|p| p.exists())
    .map(|p| p.to_path_buf())
}

/// Per-key replace: every key the project defines replaces the corresponding
/// global value wholesale, including entire sub-tables (no merging).
fn overlay(
  base: &mut toml::map::Map<String, toml::Value>,
  overlay: &toml::map::Map<String, toml::Value>,
) {
  for (key, value) in overlay {
    base.insert(key.clone(), value.clone());
  }
}
