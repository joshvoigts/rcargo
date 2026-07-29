use std::{error::Error, process::Command};

/// Detect the current local branch name.
pub fn current_branch() -> Result<String, Box<dyn Error>> {
  let output = Command::new("git")
    .args(["rev-parse", "--abbrev-ref", "HEAD"])
    .output()?;
  if !output.status.success() {
    return Err("Failed to detect current branch".into());
  }
  Ok(String::from_utf8(output.stdout)?.trim().to_string())
}
