use crate::ssh::{self, ssh_capture};

/// Ensure a bare repo exists on the remote with a post-receive hook
/// that checks out code to the working directory.
pub fn ensure_repo(host: &str) -> Result<(), Box<dyn std::error::Error>> {
  let work_dir = crate::REMOTE_REPO_PATH;
  let bare_dir = format!("{work_dir}.git");

  let is_new;

  // Create bare repo if it doesn't exist
  let exists = ssh_capture(
    host,
    &format!("test -d {bare_dir}/HEAD && echo exists"),
  );
  if matches!(&exists, Ok(s) if s == "exists") {
    is_new = false;
  } else {
    println!("Initializing bare repository on remote...");
    // Remove stale repo if it exists but is incomplete
    ssh::ssh_run(host, &format!("rm -rf {bare_dir}"))?;
    ssh::ssh_run(host, &format!("git init --bare {bare_dir}"))?;
    is_new = true;
  }

  // Set up post-receive hook to update working tree
  let hook_path = format!("{bare_dir}/hooks/post-receive");
  ssh::ssh_run(
    host,
    &format!(
      r#"cat > {hook_path} << 'HOOK'
#!/bin/sh
mkdir -p {work_dir}
while read oldrev newrev refname; do
  branch=$(echo $refname | sed 's|refs/heads/||')
  git -C {bare_dir} --work-tree={work_dir} --git-dir={bare_dir} checkout -f $branch
done
HOOK
chmod +x {hook_path}"#
    ),
  )?;

  // Update local remote URL to point to the bare repo
  let bare_url = format!("{host}:{bare_dir}");
  let output = std::process::Command::new("git")
    .args(["remote", "set-url", host, &bare_url])
    .output()?;
  if !output.status.success() {
    return Err("Failed to update local deploy remote URL.".into());
  }

  // If this is a new bare repo, push all branches to seed it
  if is_new {
    println!("Seeding bare repository from local...");
    let output = std::process::Command::new("git")
      .args(["push", host, "--all", "--force"])
      .output()?;
    if !output.status.success() {
      let err = String::from_utf8_lossy(&output.stderr);
      return Err(format!("Failed to seed bare repo: {err}").into());
    }
  }

  Ok(())
}

/// Push the local branch to the remote bare repo.
/// The post-receive hook will checkout the branch to the working directory.
pub fn checkout_branch(
  host: &str,
  branch: &str,
) -> Result<(), Box<dyn std::error::Error>> {
  // Force push to the bare repo (triggers post-receive hook)
  let output = std::process::Command::new("git")
    .args(["push", "--force", host, branch])
    .output()?;

  if !output.status.success() {
    let err = String::from_utf8_lossy(&output.stderr);
    return Err(format!("Failed to push branch '{branch}' to remote: {err}").into());
  }

  Ok(())
}
