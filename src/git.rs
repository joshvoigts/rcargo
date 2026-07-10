use crate::ssh::ssh_run;
use std::{error::Error, path::Path, process::Command};

/// Sync the local repo to the remote via rsync.
///
/// Copies the working tree *and* `.git` so the remote has the same
/// shape as the local repo (same branch, same unstaged changes).
/// Gitignored paths are excluded so build artifacts, databases, etc.
/// are untouched.
pub fn sync_repo(
  host: &str,
  remote_path: &str,
) -> Result<(), Box<dyn Error>> {
  // Ensure remote directory exists
  ssh_run(host, &format!("mkdir -p {remote_path}"))?;

  let mut rsync_args = vec!["-az", "--delete", "--exclude=.git"];

  if Path::new(".gitignore").exists() {
    rsync_args.push("--exclude-from=.gitignore");
  }

  rsync_args.push("./");

  println!("Syncing to remote...");
  let status = Command::new("rsync")
    .args(&rsync_args)
    .arg(format!("{host}:{remote_path}/"))
    .status()?;
  if !status.success() {
    return Err("rsync failed".into());
  }

  Ok(())
}

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
