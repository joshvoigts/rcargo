use serde::Deserialize;
use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use toml::map::Map;
use toml::Value;
use xdg::BaseDirectories;

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
  /// The target host to deploy to (anything you'd pass to `ssh`)
  #[serde(default)]
  pub target: String,

  /// Remote path for the repo. Defaults to `$HOME/build/{project_name}`
  #[serde(default)]
  pub remote_path: Option<String>,

  /// Remote build directory. If set, the final repo path is
  /// `{remote_build_dir}/{project_name}`. Has no effect when `remote_path`
  /// is also set.
  #[serde(default)]
  pub remote_build_dir: Option<String>,

  /// Workspace member to install (e.g. "edwin-server")
  #[serde(default)]
  pub package: Option<String>,

  /// Binary name override (defaults to auto-detect from [[bin]] target)
  #[serde(default)]
  pub bin: Option<String>,

  /// Sandbox configuration for remote builds
  #[serde(default)]
  pub sandbox: Sandbox,

  /// Command(s) to run after sync but before the sandboxed build.
  #[serde(default)]
  pub prebuild: Option<Hook>,

  /// User-defined commands. Maps a command name to an ordered list of
  /// steps, each a `"<cmd> [args...]"` string. e.g. `ci = ["lint", "test --workspace -q"]`
  #[serde(default)]
  pub commands: HashMap<String, Vec<String>>,
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
#[serde(deny_unknown_fields)]
pub struct Sandbox {
  #[serde(default = "default_true")]
  pub enabled: bool,

  #[serde(default)]
  pub allow: SandboxAllow,

  #[serde(default)]
  pub env: HashMap<String, String>,
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
#[serde(deny_unknown_fields)]
pub struct SandboxAllow {
  #[serde(default)]
  pub write: Vec<String>,

  #[serde(default)]
  pub net: Vec<String>,
}

impl Config {
  /// Load the effective config: global (XDG) provides defaults, then the
  /// project `rcargo.toml` overrides it with replace semantics. If no
  /// config file exists at all, returns an empty config so CLI flags
  /// (e.g. `--target`) can still be used alone.
  pub fn load() -> Result<Self, Box<dyn Error>> {
    let mut value = Value::Table(Map::new());

    if let Some(path) = global_config_path() {
      if path.exists() {
        value = toml::from_str(&fs::read_to_string(&path)?)?;
      }
    }

    if let Some(path) = project_config_path() {
      if path.exists() {
        let project: Value =
          toml::from_str(&fs::read_to_string(&path)?)?;
        overlay(
          value.as_table_mut().unwrap(),
          project.as_table().unwrap(),
        );
      }
    }

    let config: Config = value.try_into()?;
    Ok(config)
  }

  pub fn remote_path(&self, project_name: &str) -> String {
    if let Some(path) = &self.remote_path {
      path.clone()
    } else if let Some(dir) = &self.remote_build_dir {
      format!("{}/{}", dir.trim_end_matches('/'), project_name)
    } else {
      format!("$HOME/build/{project_name}")
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn parse(toml: &str) -> Config {
    toml::from_str(toml).unwrap()
  }

  #[test]
  fn remote_path_precedence() {
    let cfg = parse("remote_path = \"/exact/path\"\nremote_build_dir = \"/base/dir\"");
    assert_eq!(cfg.remote_path("myapp"), "/exact/path");
  }

  #[test]
  fn remote_build_dir_joins_project() {
    let cfg = parse("remote_build_dir = \"/home/james/build\"");
    assert_eq!(cfg.remote_path("myapp"), "/home/james/build/myapp");
  }

  #[test]
  fn remote_build_dir_trims_trailing_slash() {
    let cfg = parse("remote_build_dir = \"/base/dir/\"");
    assert_eq!(cfg.remote_path("myapp"), "/base/dir/myapp");
  }

  #[test]
  fn remote_path_defaults_to_home() {
    let cfg = parse("");
    assert_eq!(cfg.remote_path("myapp"), "$HOME/build/myapp");
  }

  #[test]
  fn unknown_field_is_rejected() {
    let err =
      toml::from_str::<Config>("blah = \"hello\"").unwrap_err();
    assert!(
      err.to_string().contains("unknown field"),
      "unexpected error: {err}"
    );
  }

  #[test]
  fn unknown_field_is_rejected_through_value_load_path() {
    let value: Value = toml::from_str(
      "target = \"edwin\"\nremote_path = \"/home/josh/build/edwinmain\"\nblah = \"hello\"\n\n[sandbox.env]\nDATABASE_URL = \"sqlite://db.sqlite3\"",
    )
    .unwrap();
    let err = Config::deserialize(value).unwrap_err();
    assert!(
      err.to_string().contains("unknown field"),
      "unexpected error: {err}"
    );
  }

  #[test]
  fn project_config_found_in_parent() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let config = root.join("rcargo.toml");
    std::fs::write(&config, "target = \"host\"\n").unwrap();

    let nested = root.join("a").join("b").join("c");
    std::fs::create_dir_all(&nested).unwrap();
    let found = project_config_path_from(&nested).unwrap();
    assert_eq!(found, config);
  }

  #[test]
  fn project_config_prefers_nearest() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let root_config = root.join("rcargo.toml");
    std::fs::write(&root_config, "target = \"root\"\n").unwrap();

    let sub = root.join("sub").join("deep");
    std::fs::create_dir_all(&sub).unwrap();
    let legacy = sub.join("deploy.toml");
    std::fs::write(&legacy, "target = \"sub\"\n").unwrap();

    let found = project_config_path_from(&sub).unwrap();
    assert_eq!(found, legacy);
  }

  #[test]
  fn project_config_missing_returns_none() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let nested = root.join("a").join("b");
    std::fs::create_dir_all(&nested).unwrap();
    assert!(project_config_path_from(&nested).is_none());
  }
}

/// Global config at `$XDG_CONFIG_HOME/rcargo/rcargo.toml` (default
/// `~/.config/rcargo/rcargo.toml`). Uses `get_config_home` (not
/// `get_config_file`) so no directory is created as a side effect.
fn global_config_path() -> Option<PathBuf> {
  Some(
    BaseDirectories::with_prefix("rcargo")
      .get_config_home()?
      .join("rcargo.toml"),
  )
}

/// Project config, discovered by walking up from a starting directory so it
/// works regardless of the current working directory (e.g. when invoked from
/// a workspace subdirectory). `rcargo.toml` is canonical; `deploy.toml` is
/// still accepted for backwards compatibility.
fn project_config_path() -> Option<PathBuf> {
  project_config_path_from(&std::env::current_dir().ok()?)
}

fn project_config_path_from(start: &Path) -> Option<PathBuf> {
  let mut dir = start.to_path_buf();
  loop {
    for name in ["rcargo.toml", "deploy.toml"] {
      let candidate = dir.join(name);
      if candidate.is_file() {
        return Some(candidate);
      }
    }
    if !dir.pop() {
      return None;
    }
  }
}

/// Per-key replace: every key the project defines replaces the corresponding
/// global value wholesale, including entire sub-tables (no merging).
fn overlay(
  base: &mut Map<String, Value>,
  overlay: &Map<String, Value>,
) {
  for (key, value) in overlay {
    base.insert(key.clone(), value.clone());
  }
}
