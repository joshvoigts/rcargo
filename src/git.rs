use crate::ssh::{shell_quote, ssh_run};
use ignore::gitignore::Gitignore;
use std::error::Error;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Sync the local repo to the remote via rsync.
///
/// rsync mirrors the whole working tree (`--delete`) but gitignored paths
/// are excluded so build artifacts and databases are untouched. The
/// `ignore` crate's `Gitignore` matcher decides what is ignored, honouring
/// `!` negations. `.git` is skipped explicitly.
pub fn sync_repo(
  host: &str,
  remote_path: &str,
) -> Result<(), Box<dyn Error>> {
  // Ensure remote directory exists
  ssh_run(host, &format!("mkdir -p {}", shell_quote(remote_path)))?;

  println!("Syncing to remote...");
  let root = Path::new(".");
  let (matcher, _) = Gitignore::new(".gitignore");
  let mut excludes = Vec::new();
  collect_excludes(root, &matcher, &mut excludes, root)?;

  let mut child = Command::new("rsync")
    .args([
      "-az",
      "--delete",
      "--exclude=.git",
      "--exclude-from=-",
      "./",
    ])
    .arg(format!("{host}:{remote_path}/"))
    .stdin(Stdio::piped())
    .spawn()?;
  if let Some(mut stdin) = child.stdin.take() {
    for exclude in &excludes {
      writeln!(stdin, "{exclude}")?;
    }
  }
  let status = child.wait()?;
  if !status.success() {
    return Err("rsync failed".into());
  }

  Ok(())
}

/// Recursively collect gitignored paths under `dir` as rsync exclude rules
/// (ignored files as-is, ignored directories with a trailing slash so rsync
/// skips their whole subtree). Whitelisted via `!` are descended into;
/// fully-ignored directories are pruned.
fn collect_excludes(
  root: &Path,
  matcher: &Gitignore,
  excludes: &mut Vec<String>,
  dir: &Path,
) -> Result<(), Box<dyn Error>> {
  for entry in fs::read_dir(dir)? {
    let entry = entry?;
    let path = entry.path();
    if path.file_name().and_then(|n| n.to_str()) == Some(".git") {
      continue;
    }
    let is_dir = entry.file_type()?.is_dir();
    let rel = path.strip_prefix(root).unwrap_or(&path);
    if matcher.matched(rel, is_dir).is_ignore() {
      let mut exclude = rel.to_string_lossy().into_owned();
      if is_dir {
        exclude.push('/');
      }
      excludes.push(exclude);
    } else if is_dir {
      collect_excludes(root, matcher, excludes, &path)?;
    }
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

#[cfg(test)]
mod tests {
  use super::{collect_excludes, Gitignore};

  #[test]
  fn negation_reincludes_config() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join(".cargo")).unwrap();
    std::fs::write(root.join(".cargo/config.toml"), "[net]\n")
      .unwrap();
    std::fs::write(root.join(".cargo/other"), "x").unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main(){}").unwrap();
    std::fs::create_dir_all(root.join("target")).unwrap();
    std::fs::write(root.join("target/boom"), "x").unwrap();
    std::fs::write(
      root.join(".gitignore"),
      "/.cargo/*\n!/.cargo/config.toml\n/target\n",
    )
    .unwrap();

    let (matcher, _) = Gitignore::new(root.join(".gitignore"));
    let mut excludes = Vec::new();
    collect_excludes(&root, &matcher, &mut excludes, &root).unwrap();

    assert!(excludes.contains(&".cargo/other".to_string()));
    assert!(excludes.contains(&"target/".to_string()));
    assert!(!excludes.contains(&".cargo/".to_string()));
    assert!(!excludes.contains(&".cargo/config.toml".to_string()));
    assert!(!excludes.contains(&"src/main.rs".to_string()));
  }

  #[test]
  fn no_gitignore_has_no_excludes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("a.txt"), "a").unwrap();
    std::fs::write(root.join("sub/b.txt"), "b").unwrap();

    let (matcher, _) = Gitignore::new(root.join(".gitignore"));
    let mut excludes = Vec::new();
    collect_excludes(&root, &matcher, &mut excludes, &root).unwrap();
    assert!(excludes.is_empty());
  }
}
