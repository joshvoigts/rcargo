use crate::ssh::{self, ssh_capture};

/// Ensure the repo exists on the remote. Clones it if it doesn't.
pub fn ensure_repo(
  host: &str,
) -> Result<(), Box<dyn std::error::Error>> {
  let repo_path = crate::REMOTE_REPO_PATH;

  // Check if .git directory exists
  let result = ssh_capture(
    host,
    &format!("test -d {repo_path}/.git && echo exists"),
  );
  if matches!(&result, Ok(s) if s == "exists") {
    return Ok(());
  }

  println!("Cloning repository to {repo_path}...");
  let origin = get_local_origin()?;
  ssh::ssh_run(host, &format!("git clone {origin} {repo_path}"))?;
  println!("Repository cloned.");
  Ok(())
}

/// Checkout the specified branch on the remote. Errors if the working tree is dirty.
pub fn checkout_branch(
  host: &str,
  branch: &str,
) -> Result<(), Box<dyn std::error::Error>> {
  let repo_path = crate::REMOTE_REPO_PATH;

  // Check for dirty working tree
  let status = ssh_capture(
    host,
    &format!("cd {repo_path} && git status --porcelain"),
  );
  if let Ok(output) = &status {
    if !output.is_empty() {
      return Err("Remote working tree is dirty. Commit or stash changes first.".into());
    }
  } else {
    status?;
    return Ok(());
  }

  ssh::ssh_run(
    host,
    &format!("cd {repo_path} && git fetch && git checkout {branch}"),
  )?;
  Ok(())
}

/// Get the remote origin URL from the local git repo.
fn get_local_origin() -> Result<String, Box<dyn std::error::Error>> {
  let output = std::process::Command::new("git")
    .args(["remote", "get-url", "deploy"])
    .output()?;

  if !output.status.success() {
    return Err("Could not determine local git deploy origin. Make sure you're in a git repo.".into());
  }

  Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
