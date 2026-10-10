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
/// only meaningful on the machine that wrote it. Waiters die with their
/// connection: the wait loop blocks on `read`s of its stdin — ticked by a
/// local keep-alive — so a killed rcargo ends its waiter via EOF instead
/// of leaving it to re-take the lock the moment `unlock` removes it.
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

    let cmd = wait_script(path, local_pid, max_wait);
    let out =
      ssh::ssh_capture_keepalive(host, &cmd, Duration::from_secs(1))?;
    if !out.is_empty() {
      println!("{out}");
    }
    Ok(Self {
      host: host.to_string(),
      path: path.to_string(),
    })
  }
}

/// The remote wait-loop script: claims `path` via atomic `mkdir` once
/// ownership is free, or after `max_wait` reports a timeout.
///
/// The loop is ticked by one `read` of stdin per wait iteration — the
/// local side's keep-alive supplies a newline each second. EOF on stdin
/// (the local rcargo died) ends the loop without claiming the lock, so
/// nothing survives to steal it back after `unlock`. The holder is only
/// written on a successful `mkdir`, so an EOF-interrupted waiter never
/// overwrites another owner's PID.
fn wait_script(
  path: &str,
  local_pid: u32,
  max_wait: Duration,
) -> String {
  let lock = ssh::shell_quote(path);
  format!(
    "LOCK={lock}\n\
     HOLDER={pid}\n\
     MAXWAIT={max_wait}\n\
     mkdir -p \"$(dirname \"$LOCK\")\"\n\
     if [ \"$(cat \"$LOCK/holder\" 2>/dev/null)\" = \"$HOLDER\" ]; then\n\
     exit 0\n\
     fi\n\
     if mkdir \"$LOCK\" 2>/dev/null; then\n\
     echo \"$HOLDER\" > \"$LOCK/holder\"\n\
     else\n\
     START=$(date +%s)\n\
     while read _; do\n\
     if mkdir \"$LOCK\" 2>/dev/null; then\n\
     echo \"$HOLDER\" > \"$LOCK/holder\"\n\
     break\n\
     fi\n\
     NOW=$(date +%s)\n\
     if [ $((NOW - START)) -ge \"$MAXWAIT\" ]; then\n\
     echo \"Timed out after ${{MAXWAIT}}s waiting for the remote lock at $LOCK\" >&2\n\
     echo \"A previous rcargo may be stuck. If none is running, clear it with: rcargo unlock\" >&2\n\
     exit 1\n\
     fi\n\
     done\n\
     fi",
    lock = lock,
    pid = local_pid,
    max_wait = max_wait.as_secs(),
  )
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

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Write;
  use std::process::{Command, Stdio};
  use std::time::Duration;

  fn sh_cmd(cmd: &str) -> bool {
    match Command::new("sh").args(["-c", cmd]).output() {
      Ok(o) => o.status.success(),
      Err(_) => false,
    }
  }

  fn sh_out(cmd: &str) -> String {
    match Command::new("sh").args(["-c", cmd]).output() {
      Ok(o) if o.status.success() => {
        String::from_utf8_lossy(&o.stdout).trim().to_string()
      }
      _ => "".to_string(),
    }
  }

  /// Run the wait script under `sh` with piped stdin: write one wake-up
  /// line when `wake` is set, then close stdin so a waiter whose writer
  /// is gone sees EOF. Returns whether the script exited successfully.
  fn run_wait_script(wake: bool, script: &str) -> bool {
    let mut child = match Command::new("sh")
      .args(["-c", script])
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::piped())
      .spawn()
    {
      Ok(c) => c,
      Err(_) => return false,
    };
    {
      let mut stdin = child.stdin.take().unwrap();
      if wake {
        let _ = stdin.write_all(b"wake\n");
      }
    } // stdin closed: a waiter whose writer died sees EOF here
    match child.wait() {
      Ok(s) => s.success(),
      Err(_) => false,
    }
  }

  /// A waiter whose stdin hit EOF must end without claiming a lock held
  /// by someone else: the holder PID stays intact and the lock survives.
  fn eof_does_not_claim(wake: bool) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let home = root.to_string_lossy();
    let lock = lock_path(&home);
    let holder = holder_path(&home);
    assert!(sh_cmd(&format!(
      "mkdir -p {l} && echo 999 > {h}",
      l = ssh::shell_quote(&lock),
      h = ssh::shell_quote(&holder),
    )));
    let script = wait_script(&lock, 1234, Duration::from_secs(10));
    assert!(run_wait_script(wake, &script));
    assert_eq!(
      sh_out(&format!("cat {h}", h = ssh::shell_quote(&holder))),
      "999"
    );
    assert!(sh_cmd(&format!(
      "test -d {l}",
      l = ssh::shell_quote(&lock)
    )));
  }

  #[test]
  fn eof_immediately_does_not_claim_foreign_lock() {
    eof_does_not_claim(false);
  }

  #[test]
  fn wake_without_release_does_not_claim_foreign_lock() {
    eof_does_not_claim(true);
  }

  #[test]
  fn waiter_claims_free_lock_and_writes_holder() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let home = root.to_string_lossy();
    let lock = lock_path(&home);
    let holder = holder_path(&home);
    let script = wait_script(&lock, 1234, Duration::from_secs(10));
    assert!(run_wait_script(true, &script));
    assert_eq!(
      sh_out(&format!("cat {h}", h = ssh::shell_quote(&holder))),
      "1234"
    );
    assert!(sh_cmd(&format!(
      "test -d {l}",
      l = ssh::shell_quote(&lock)
    )));
  }

  #[test]
  fn waiter_times_out_when_maxwait_elapses() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let home = root.to_string_lossy();
    let lock = lock_path(&home);
    let script = wait_script(&lock, 1234, Duration::from_secs(0));
    assert!(sh_cmd(&format!(
      "mkdir -p {l}",
      l = ssh::shell_quote(&lock)
    )));
    assert!(!run_wait_script(true, &script));
  }
}
