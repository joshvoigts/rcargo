use crate::ssh;
use std::error::Error;
use std::process::Command;
use std::time::Duration;

/// Serialize rcargo invocations on a given remote build dir.
///
/// The lock is a directory: `mkdir` is atomic, so exactly one process
/// creates it. The name is fixed (not per-PID) so the atomicity is what
/// provides exclusion. The owner's local PID is stored inside
/// (`.rcargo.lock/holder`) so `rcargo unlock` can kill it if still alive.
/// Held for the whole remote operation, released on drop.
pub fn lock_path(remote_path: &str) -> String {
  format!("{remote_path}/.rcargo.lock")
}

fn holder_path(remote_path: &str) -> String {
  format!("{remote_path}/.rcargo.lock/holder")
}

/// Remove the remote lock, killing the local rcargo that owns it if it is
/// still running. Clears a stale lock left by an interrupted rcargo, which
/// never got to run its drop-based release.
pub fn unlock(
  host: &str,
  remote_path: &str,
) -> Result<(), Box<dyn Error>> {
  let path = lock_path(remote_path);
  let quoted = ssh::shell_quote(&path);
  let holder_q = ssh::shell_quote(&holder_path(remote_path));

  // Best-effort: read the owner's local PID and kill it if still alive.
  let holder =
    ssh::ssh_capture(host, &format!("cat {holder_q} 2>/dev/null"))
      .unwrap_or_default();
  if let Ok(pid) = holder.trim().parse::<u32>() {
    kill_local_rcargo(pid);
  }

  let existed = ssh::ssh_capture(host, &format!("test -d {quoted}"))
    .is_ok_and(|_| true);
  ssh::ssh_capture(host, &format!("rm -rf {quoted}"))?;
  if existed {
    println!("Removed remote lock at {path}");
  } else {
    println!("No remote lock at {path}");
  }
  Ok(())
}

/// Kill a local rcargo process by PID to free a lock it holds.
///
/// Only acts when the PID belongs to a `rcargo` binary, so running `unlock`
/// on a different machine than the lock owner can't kill an unrelated
/// process that happens to share the PID.
fn kill_local_rcargo(pid: u32) {
  let out = match Command::new("ps")
    .args(["-p", &pid.to_string(), "-o", "comm="])
    .output()
  {
    Ok(o) if o.status.success() => o,
    _ => return,
  };
  // comm is a path on macOS, a name on Linux; match on the basename.
  if String::from_utf8_lossy(&out.stdout)
    .trim()
    .rsplit('/')
    .next()
    != Some("rcargo")
  {
    return;
  }
  match Command::new("kill").arg(pid.to_string()).status() {
    Ok(s) if s.success() => println!("Killed rcargo PID {pid}"),
    _ => println!("rcargo PID {pid} is not running"),
  }
}

/// A held remote lock. Released on drop so the operation it guards releases
/// the lock on both the success and error paths.
pub struct Lock {
  host: String,
  path: String,
}

impl Lock {
  /// Block until the remote lock at `path` is held, then return it.
  ///
  /// The wait is a dedicated SSH call with no per-step timeout (bounded by
  /// the script's own `MAXWAIT`), so a queued invocation is not killed by
  /// the work timeout. Output is captured and shown only on completion, so
  /// the common lock-free path stays silent.
  pub fn acquire(
    host: &str,
    path: &str,
    local_pid: u32,
    max_wait: Duration,
  ) -> Result<Self, Box<dyn Error>> {
    let cmd = format!(
      "LOCK={lock}\n\
       HOLDER={pid}\n\
       MAXWAIT={max_wait}\n\
       mkdir -p \"$(dirname \"$LOCK\")\"\n\
       if ! mkdir \"$LOCK\" 2>/dev/null; then\n\
       echo \"Remote lock held by a previous rcargo; waiting for it to finish...\"\n\
       START=$(date +%s)\n\
       while ! mkdir \"$LOCK\" 2>/dev/null; do\n\
       NOW=$(date +%s)\n\
       if [ $((NOW - START)) -ge \"$MAXWAIT\" ]; then\n\
       echo \"Timed out after ${{MAXWAIT}}s waiting for the remote lock at $LOCK\" >&2\n\
       echo \"A previous rcargo may be stuck. If none is running, clear it with: rcargo unlock\" >&2\n\
       exit 1\n\
       fi\n\
       sleep 1\n\
       done\n\
       fi\n\
       echo \"$HOLDER\" > \"$LOCK/holder\"",
      lock = ssh::shell_quote(path),
      pid = local_pid,
      max_wait = max_wait.as_secs(),
    );
    let out = ssh::ssh_capture(host, &cmd)?;
    if !out.is_empty() {
      println!("{out}");
    }
    Ok(Self {
      host: host.to_string(),
      path: path.to_string(),
    })
  }
}

impl Drop for Lock {
  fn drop(&mut self) {
    // We only hold the lock if acquire succeeded, so removing it is safe.
    let _ = ssh::ssh_capture(
      &self.host,
      &format!("rm -rf {lock}", lock = ssh::shell_quote(&self.path)),
    );
  }
}
