use std::fs;
use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
  /// The target host to deploy to (e.g. "myserver.local" or "user@host")
  pub target: String,

  /// Remote path for the repo. Defaults to `$HOME/build/{project_name}`
  #[serde(default)]
  pub remote_path: Option<String>,
}

impl Config {
  pub fn load() -> Result<Self, Box<dyn std::error::Error>> {
    let config_path = Path::new("deploy.toml");
    if !config_path.exists() {
      return Err(
        "deploy.toml not found. Create one with:\ntarget = \"your-server\""
          .into(),
      );
    }
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
