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
  // Ensure remote directory exists
  ssh_run(host, &format!("mkdir -p {remote_path}"))?;

  let mut rsync_args = vec!["-avz", "--delete"];

  if std::path::Path::new(".gitignore").exists() {
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
