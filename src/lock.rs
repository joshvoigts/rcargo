use crate::ssh;
use std::error::Error;
use std::process::Command;
use std::time::Duration;

/// Serialize rcargo invocations on a given remote host.
///
/// The lock is a single directory at `~/.rcargo.lock` on the remote, shared
/// by every project: `mkdir` is atomic, so exactly one process creates it.
/// The owner's local PID is stored inside (`.rcargo.lock/holder`) so
/// `rcargo unlock` can kill it if still alive. Held for the whole remote
/// operation, released on drop.
///
/// A held lock is never a silent hang: `acquire` names the owner and the
/// wait time up front. A lock left behind by an interrupted rcargo
/// (Ctrl-C, kill, sleep, dropped connection) is cleared with `rcargo
/// unlock`; rcargo never steals a lock, because the recorded owner PID is
/// only meaningful on the machine that wrote it.
pub fn lock_path(home: &str) -> String {
  format!("{home}/.rcargo.lock")
}

fn holder_path(home: &str) -> String {
  format!("{home}/.rcargo.lock/holder")
}

/// Remove the remote lock, killing the local rcargo that owns it if it is
/// still running. Clears a stale lock left by an interrupted rcargo, which
/// never got to run its drop-based release.
pub fn unlock(host: &str, home: &str) -> Result<(), Box<dyn Error>> {
  let path = lock_path(home);
  let quoted = ssh::shell_quote(&path);
  let holder_q = ssh::shell_quote(&holder_path(home));

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

/// Whether a local PID currently refers to a running rcargo process.
///
/// `ps -o comm=` reports a path on macOS and a name on Linux, so compare
/// the basename. A PID that is missing or belongs to a different binary is
/// dead from rcargo's point of view.
fn is_live_rcargo(pid: u32) -> bool {
  let out = match Command::new("ps")
    .args(["-p", &pid.to_string(), "-o", "comm="])
    .output()
  {
    Ok(o) if o.status.success() => o,
    _ => return false,
  };
  String::from_utf8_lossy(&out.stdout)
    .trim()
    .rsplit('/')
    .next()
    == Some("rcargo")
}

/// Kill a local rcargo process by PID to free a lock it holds.
///
/// Only acts when the PID belongs to a `rcargo` binary, so running `unlock`
/// on a different machine than the lock owner can't kill an unrelated
/// process that happens to share the PID.
fn kill_local_rcargo(pid: u32) {
  if !is_live_rcargo(pid) {
    println!("PID {pid} is not a running rcargo; skipping");
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
  /// Fast path is a single SSH round trip when nobody holds the lock; the
  /// remote echoes ACQUIRED or HELD, so a broken round trip is reported as
  /// an unknown lock state instead of contention. When it is held, the
  /// owner is identified from the holder PID (when that PID is a running
  /// rcargo on this machine) and the wait is announced before blocking —
  /// so a lock is never a silent hang, and the user can decide to clear a
  /// stale one with `rcargo unlock`. A fast-path acquire interrupted by a
  /// dropped connection is self-healing: if the holder already names this
  /// invocation, the wait loop claims the lock immediately.
  pub fn acquire(
    host: &str,
    path: &str,
    local_pid: u32,
    max_wait: Duration,
  ) -> Result<Self, Box<dyn Error>> {
    let lock = ssh::shell_quote(path);
    let holder = ssh::shell_quote(&format!("{path}/holder"));

    // Fast path: single round trip when the lock is free. The remote always
    // exits 0 and echoes ACQUIRED or HELD, so a failed round trip can never
    // be mistaken for lock contention.
    let fast = format!(
      "mkdir -p \"$(dirname {lock})\" && if mkdir {lock} 2>/dev/null; \
       then echo {pid} > {holder}; echo ACQUIRED; else echo HELD; fi",
      lock = lock,
      pid = local_pid,
      holder = holder,
    );
    let out = ssh::ssh_capture(host, &fast);
    if let Ok(state) = out {
      if state == "ACQUIRED" {
        return Ok(Self {
          host: host.to_string(),
          path: path.to_string(),
        });
      }

      // Held — say so immediately, naming the owner when it is meaningful,
      // before blocking for up to max_wait.
      let owner_out = ssh::ssh_capture(
        host,
        &format!("cat {holder} 2>/dev/null", holder = holder),
      )
      .unwrap_or_default();
      let secs = max_wait.as_secs();
      match owner_out.trim().parse::<u32>().ok() {
        Some(pid) if is_live_rcargo(pid) => println!(
          "Remote build dir is locked by rcargo PID {pid} on this machine; \
           waiting up to {secs}s for it to finish...",
          pid = pid,
          secs = secs,
        ),
        Some(pid) => println!(
          "Remote build dir is locked by rcargo PID {pid} (not running here \
           — possibly stale, or owned by another machine). Waiting up to \
           {secs}s; interrupt and run `rcargo unlock` to take over now.",
          pid = pid,
          secs = secs,
        ),
        None => println!(
          "Remote build dir is locked (no live owner recorded). Waiting up \
           to {secs}s; interrupt and run `rcargo unlock` to take over now.",
          secs = secs,
        ),
      }
    } else {
      // The round trip itself broke, so the lock state is unknown. Say so
      // honestly; the wait below surfaces a real error if SSH stays down.
      println!(
        "Remote lock state unknown (SSH round trip failed); \
         retrying via the lock wait..."
      );
    }

    let cmd = format!(
      "LOCK={lock}\n\
       HOLDER={pid}\n\
       MAXWAIT={max_wait}\n\
       mkdir -p \"$(dirname \"$LOCK\")\"\n\
       if [ \"$(cat \"$LOCK/holder\" 2>/dev/null)\" = \"$HOLDER\" ]; then\n\
       exit 0\n\
       fi\n\
       if ! mkdir \"$LOCK\" 2>/dev/null; then\n\
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
      lock = lock,
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
