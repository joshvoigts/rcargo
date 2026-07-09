use std::process::Command;

use crate::ssh::ssh_run;

/// Sync the local repo to the remote via rsync.
///
/// Copies the working tree *and* `.git` so the remote has the same
/// shape as the local repo (same branch, same unstaged changes).
/// Gitignored paths are excluded so build artifacts, databases, etc.
/// are untouched.
pub fn sync_repo(
  host: &str,
  remote_path: &str,
  branch: &str,
) -> Result<(), Box<dyn std::error::Error>> {
  // Ensure rsync is available
  let rsync_available = Command::new("rsync")
    .arg("--version")
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null())
    .status()
    .map(|s| s.success())
    .unwrap_or(false);

  if !rsync_available {
    return Err("rsync is required but not installed.".into());
  }

  // Ensure remote directory exists
  ssh_run(host, &format!("mkdir -p {remote_path}"))?;

  let mut rsync_args: Vec<String> =
    vec!["-avz".into(), "--delete".into()];

  if std::path::Path::new(".gitignore").exists() {
    rsync_args.push("--exclude-from=.gitignore".into());
  }

  rsync_args.push("./".into());
  rsync_args.push(format!("{host}:{remote_path}/"));

  println!("Syncing to remote...");
  let status = Command::new("rsync").args(&rsync_args).status()?;
  if !status.success() {
    return Err("rsync failed".into());
  }

  // Ensure the correct branch is checked out on the remote
  println!("Checking out branch '{branch}' on remote...");
  ssh_run(
    host,
    &format!("cd {remote_path} && git checkout {branch}"),
  )?;

  Ok(())
}

/// Detect the current local branch name.
pub fn current_branch() -> Result<String, Box<dyn std::error::Error>>
{
  let output = Command::new("git")
    .args(["rev-parse", "--abbrev-ref", "HEAD"])
    .output()?;
  if !output.status.success() {
    return Err("Failed to detect current branch".into());
  }
  Ok(String::from_utf8(output.stdout)?.trim().to_string())
}
