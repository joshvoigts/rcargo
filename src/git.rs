use std::fs;
use std::process::Command;

use crate::ssh::ssh_run;

/// Ensure the remote working directory exists.
pub fn ensure_repo(
  host: &str,
) -> Result<(), Box<dyn std::error::Error>> {
  let work_dir = crate::REMOTE_REPO_PATH;
  ssh_run(host, &format!("mkdir -p {work_dir}"))?;
  Ok(())
}

/// Sync the local branch to the remote via rsync through a temporary archive.
/// Excludes gitignored paths so build artifacts, databases, etc. are untouched.
pub fn checkout_branch(
  host: &str,
  branch: &str,
) -> Result<(), Box<dyn std::error::Error>> {
  let work_dir = crate::REMOTE_REPO_PATH;
  let staging_dir = format!("/tmp/edwin-deploy-{branch}");

  // Clean up any leftover staging directory
  let _ = fs::remove_dir_all(&staging_dir);
  fs::create_dir_all(&staging_dir)?;

  // Archive the branch into the staging directory
  println!("Archiving branch '{branch}'...");
  let status = Command::new("sh")
    .args([
      "-c",
      &format!("git archive {branch} | tar -x -C {staging_dir}"),
    ])
    .status()?;
  if !status.success() {
    return Err("Failed to archive branch".into());
  }

  let mut rsync_args: Vec<String> = vec![
    "-avz".into(),
    "--delete".into(),
    "--exclude-from=.gitignore".into(),
  ];
  rsync_args.push(format!("{staging_dir}/"));
  rsync_args.push(format!("{host}:{work_dir}/"));

  println!("Syncing to remote...");
  let status = Command::new("rsync").args(&rsync_args).status()?;
  if !status.success() {
    return Err("rsync failed".into());
  }

  // Clean up staging directory
  let _ = fs::remove_dir_all(&staging_dir);

  Ok(())
}
