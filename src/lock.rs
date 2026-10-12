//! Serialize rcargo invocations on a given remote host.
//!
//! The lock is a single directory at `~/.rcargo.lock` on the remote,
//! shared by every project: `mkdir` is atomic, so exactly one process
//! creates it. Inside it, `holder` records the owner as
//! `"<pid> <machine>"`. A bare PID is only meaningful on the machine
//! that wrote it, and rcargo never runs on the remote it locks.
//!
//! A held lock is never a silent hang: `acquire` names the owner and the
//! wait time up front. Three cases follow from the holder record:
//!
//! - a live rcargo on this machine: wait for it.
//! - an rcargo that named *this* machine whose PID is gone: the lock is
//!   stale. `acquire` takes it over, then reaps the build work that owner
//!   left running ([`ssh::reap_remote_work`], matched by the exported job
//!   marker) — no `max_wait`, no manual step.
//! - another machine: never touched. Its PID means nothing here, so only
//!   `rcargo unlock` clears it.
//!
//! Every release is compare-and-delete: the directory is removed only
//! while `holder` still names the releaser, so taking over a stale lock
//! cannot delete a lock a live rcargo claimed in the meantime.
//!
//! Waiters die with their connection: the wait loop blocks on `read`s of
//! its stdin — ticked by a local keep-alive — so a killed rcargo ends its
//! waiter via EOF instead of leaving it to re-take the lock the moment
//! `unlock` removes it.

use crate::ssh;
use std::error::Error;
use std::process::Command;
use std::sync::LazyLock;
use std::time::Duration;

pub fn lock_path(home: &str) -> String {
  format!("{home}/.rcargo.lock")
}

fn holder_file(path: &str) -> String {
  format!("{path}/holder")
}

/// Name of the machine running this rcargo, recorded with the holder PID.
///
/// Empty when `hostname` cannot be run. Reclaim then fails closed, so a
/// PID recorded elsewhere is never mistaken for a dead local one.
fn local_machine() -> &'static str {
  static MACHINE: LazyLock<String> = LazyLock::new(|| {
    Command::new("hostname")
      .output()
      .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
      .unwrap_or_default()
  });
  MACHINE.as_str()
}

/// Token a holder writes for itself: `"<pid> <machine>"`.
fn owner_token(pid: u32) -> String {
  format!("{pid} {}", local_machine()).trim_end().to_string()
}

/// Read the remote holder record; empty when there is none.
fn read_holder(host: &str, path: &str) -> String {
  ssh::ssh_capture(
    host,
    &format!(
      "cat {} 2>/dev/null",
      ssh::shell_quote(&holder_file(path))
    ),
  )
  .unwrap_or_default()
  .trim()
  .to_string()
}

/// Remove the remote lock, then make the remote tree idle.
///
/// Clears a stale lock left by an rcargo that never got to run its
/// drop-based release. A live owner on this machine is killed first, and
/// the build work it left behind is reaped, so the next holder starts
/// against an idle tree instead of racing a `sync_repo` still in flight.
pub fn unlock(host: &str, home: &str) -> Result<(), Box<dyn Error>> {
  let path = lock_path(home);
  let recorded = read_holder(host, &path);

  // Best-effort: only a live rcargo on this machine can be stopped here.
  if let Holder::Live { pid } =
    classify_holder(&recorded, local_machine(), is_live_rcargo)
  {
    kill_local_rcargo(pid);
  }

  // Stopping the holder does not stop the build it started.
  let reaped = ssh::reap_remote_work(host);
  if reaped > 0 {
    println!("Reaped {reaped} orphaned remote build process(es)");
  }

  let quoted = ssh::shell_quote(&path);
  let existed =
    ssh::ssh_capture(host, &format!("test -d {quoted}")).is_ok();
  ssh::ssh_capture(host, &format!("rm -rf {quoted}"))?;
  if existed {
    println!("Removed remote lock at {path}");
  } else {
    println!("No remote lock at {path}");
  }
  Ok(())
}

/// Report the remote lock state without taking it.
///
/// Read-only counterpart to [`unlock`]: says whether the shared per-host
/// build lock is held and who owns it, without risk of killing a live
/// process. A holder that names another machine is reported as such —
/// this machine cannot judge a PID it did not write.
pub fn status(host: &str, home: &str) -> Result<(), Box<dyn Error>> {
  let path = lock_path(home);
  let quoted = ssh::shell_quote(&path);

  // `test -d` exits non-zero when the lock directory is absent, which
  // ssh_capture reports as an error — i.e. the lock is free.
  if ssh::ssh_capture(host, &format!("test -d {quoted}")).is_err() {
    println!("Remote lock: free");
    return Ok(());
  }

  let recorded = read_holder(host, &path);
  match classify_holder(&recorded, local_machine(), is_live_rcargo) {
    Holder::Unknown => {
      println!("Remote lock: held (no owner recorded)");
    }
    Holder::Live { pid } => {
      println!("Remote lock: held by rcargo PID {pid} on this machine");
    }
    Holder::DeadHere { pid, .. } => println!(
      "Remote lock: stale — rcargo PID {pid} on this machine is not \
       running. The next rcargo command takes it over automatically."
    ),
    Holder::Foreign { pid, machine: "", .. } => {
      println!(
        "Remote lock: held by rcargo PID {pid} \
         (owner machine unknown — possibly stale)"
      );
    }
    Holder::Foreign { pid, machine, .. } => println!(
      "Remote lock: held by rcargo PID {pid} on {machine}; this machine \
       cannot tell whether that process is alive."
    ),
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

/// What a holder record says about who owns the lock right now.
#[derive(Debug, PartialEq)]
enum Holder<'a> {
  /// No record, or one that names no PID.
  Unknown,
  /// A live rcargo on this machine owns it.
  Live { pid: u32 },
  /// An rcargo that named this machine is gone: reclaimable.
  DeadHere { owner: &'a str, pid: u32 },
  /// Owned by another machine, or by an rcargo too old to have named one.
  Foreign {
    owner: &'a str,
    pid: u32,
    machine: &'a str,
  },
}

/// Read a holder record against this machine.
///
/// `machine` is [`local_machine`]; empty means the comparison is
/// impossible, so nothing can be called dead here.
fn classify_holder<'a>(
  recorded: &'a str,
  machine: &str,
  is_live: impl Fn(u32) -> bool,
) -> Holder<'a> {
  if recorded.is_empty() {
    return Holder::Unknown;
  }
  let (pid_field, machine_field) =
    recorded.split_once(' ').unwrap_or((recorded, ""));
  let Ok(pid) = pid_field.parse::<u32>() else {
    return Holder::Unknown;
  };
  // Fail closed: without a machine that matches, only `rcargo unlock` may
  // clear the lock.
  if machine.is_empty() || machine_field != machine {
    return Holder::Foreign {
      owner: recorded,
      pid,
      machine: machine_field,
    };
  }
  if is_live(pid) {
    Holder::Live { pid }
  } else {
    Holder::DeadHere {
      owner: recorded,
      pid,
    }
  }
}

/// Outcome of trying to take the lock in one round trip.
#[derive(Debug, PartialEq)]
enum Claimed {
  Acquired,
  Contended,
  /// The round trip broke, so the state is unknown.
  Unknown,
}

/// The remote script that claims `path` for `owner`.
///
/// Always exits 0 and echoes ACQUIRED or HELD, so a failed round trip can
/// never be read as contention.
fn claim_script(path: &str, owner: &str) -> String {
  let lock = ssh::shell_quote(path);
  let holder = ssh::shell_quote(&holder_file(path));
  let owner = ssh::shell_quote(owner);
  format!(
    "mkdir -p \"$(dirname {lock})\" && \
     if mkdir {lock} 2>/dev/null; \
     then echo {owner} > {holder}; echo ACQUIRED; \
     else echo HELD; fi"
  )
}

fn claim(host: &str, path: &str, owner: &str) -> Claimed {
  match ssh::ssh_capture(host, &claim_script(path, owner)) {
    Ok(out) if out.contains("ACQUIRED") => Claimed::Acquired,
    Ok(_) => Claimed::Contended,
    Err(_) => Claimed::Unknown,
  }
}

/// The remote script that removes `path` only while `holder` names
/// `owner`, echoing RELEASED when it does.
fn release_script(path: &str, owner: &str) -> String {
  let lock = ssh::shell_quote(path);
  let holder = ssh::shell_quote(&holder_file(path));
  let owner = ssh::shell_quote(owner);
  format!(
    "if [ \"$(cat {holder} 2>/dev/null)\" = {owner} ]; \
     then rm -rf {lock}; echo RELEASED; fi"
  )
}

/// Release the lock if — and only if — `owner` still holds it.
///
/// Compare-and-delete is what makes reclaiming safe: when a live rcargo
/// claimed the lock after it was read, its name is in the file and the
/// directory stays put.
fn release(
  host: &str,
  path: &str,
  owner: &str,
) -> Result<bool, Box<dyn Error>> {
  Ok(
    ssh::ssh_capture(host, &release_script(path, owner))?
      .contains("RELEASED"),
  )
}

/// A held remote lock. Released on drop so the operation it guards
/// releases the lock on both the success and error paths.
pub struct Lock {
  host: String,
  path: String,
  owner: String,
}

impl Lock {
  /// Block until the remote lock at `path` is held, then return it.
  ///
  /// Fast path is a single SSH round trip when nobody holds the lock. When
  /// it is held, the holder record decides what happens: wait for a live
  /// local owner, take over a dead one (after reaping its orphaned build
  /// work), and never touch another machine's lock. The owner is named and
  /// the wait announced before blocking, so a lock is never a silent hang.
  pub fn acquire(
    host: &str,
    path: &str,
    local_pid: u32,
    max_wait: Duration,
  ) -> Result<Self, Box<dyn Error>> {
    let ours = owner_token(local_pid);
    let secs = max_wait.as_secs();

    match claim(host, path, &ours) {
      Claimed::Acquired => return Ok(Self::held(host, path, ours)),
      Claimed::Unknown => {
        // The round trip itself broke, so the lock state is unknown. Say
        // so honestly; the wait below surfaces a real error if SSH stays
        // down.
        println!(
          "Remote lock state unknown (SSH round trip failed); \
           retrying via the lock wait..."
        );
      }
      Claimed::Contended => {
        let recorded = read_holder(host, path);
        // A wait loop interrupted by a dropped connection may already
        // have claimed the lock for this very invocation.
        if recorded == ours {
          return Ok(Self::held(host, path, ours));
        }
        match classify_holder(
          &recorded,
          local_machine(),
          is_live_rcargo,
        ) {
          Holder::Live { pid } => println!(
            "Remote build dir is locked by rcargo PID {pid} on this \
             machine; waiting up to {secs}s for it to finish..."
          ),
          Holder::DeadHere { owner, pid } => {
            println!(
              "Remote build dir is locked by rcargo PID {pid} on this \
               machine, which is no longer running. Taking it over..."
            );
            // Take the lock first: only the lock holder may reap, or a
            // concurrent reclaim could get its live build signalled.
            let taken = release(host, path, owner).unwrap_or(false)
              && claim(host, path, &ours) == Claimed::Acquired;
            if !taken {
              println!(
                "Another rcargo claimed the lock first; waiting up to \
                 {secs}s..."
              );
            } else {
              // We hold the lock, so every RCARGO_JOB process on the
              // host is the dead owner's orphaned work, never a live
              // build.
              let reaped = ssh::reap_remote_work(host);
              if reaped > 0 {
                println!(
                  "Reaped {reaped} orphaned remote build process(es)"
                );
              }
              return Ok(Self::held(host, path, ours));
            }
          }
          Holder::Foreign { pid, machine: "", .. } => {
            println!(
              "Remote build dir is locked by rcargo PID {pid}, written \
               by an rcargo that named no machine. Waiting up to \
               {secs}s; interrupt and run `rcargo unlock` to take over \
               now."
            );
          }
          Holder::Foreign { pid, machine, .. } => println!(
            "Remote build dir is locked by rcargo PID {pid} on \
             {machine}. That PID means nothing here, so rcargo will not \
             take it over. Waiting up to {secs}s; run `rcargo unlock` to \
             clear it now."
          ),
          Holder::Unknown => println!(
            "Remote build dir is locked (no owner recorded). Waiting up \
             to {secs}s; interrupt and run `rcargo unlock` to take over \
             now."
          ),
        }
      }
    }

    let cmd = wait_script(path, &ours, max_wait);
    let out =
      ssh::ssh_capture_keepalive(host, &cmd, Duration::from_secs(1))?;
    if !out.is_empty() {
      println!("{out}");
    }
    Ok(Self::held(host, path, ours))
  }

  fn held(host: &str, path: &str, owner: String) -> Self {
    Self {
      host: host.to_string(),
      path: path.to_string(),
      owner,
    }
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
  owner: &str,
  max_wait: Duration,
) -> String {
  let lock = ssh::shell_quote(path);
  let owner = ssh::shell_quote(owner);
  let max_wait = max_wait.as_secs();
  format!(
    "LOCK={lock}\n\
     HOLDER={owner}\n\
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
     fi"
  )
}

impl Drop for Lock {
  fn drop(&mut self) {
    // We only get here if acquire succeeded, so the lock is ours unless
    // something cleared it or re-took it — which compare-and-delete
    // leaves alone. A lock we could not release is reported, never
    // swallowed: the next rcargo would read it as unexplained contention.
    match release(&self.host, &self.path, &self.owner) {
      Ok(true) => (),
      Ok(false) => eprintln!(
        "rcargo left the remote lock at {}: it is gone or owned by \
         another rcargo.",
        self.path
      ),
      Err(e) => eprintln!(
        "rcargo could not release the remote lock at {}: {e}",
        self.path
      ),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Write;
  use std::process::{Command, Stdio};
  use std::time::Duration;

  const OURS: &str = "1234 testhost";

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

  fn mkdir_holder(lock: &str, owner: &str) {
    assert!(sh_cmd(&format!(
      "mkdir -p {l} && echo {o} > {h}",
      l = ssh::shell_quote(lock),
      o = ssh::shell_quote(owner),
      h = ssh::shell_quote(&holder_file(lock)),
    )));
  }

  fn lock_exists(lock: &str) -> bool {
    sh_cmd(&format!("test -d {}", ssh::shell_quote(lock)))
  }

  fn recorded_holder(lock: &str) -> String {
    sh_out(&format!("cat {}", ssh::shell_quote(&holder_file(lock))))
  }

  /// A waiter whose stdin hit EOF must end without claiming a lock held
  /// by someone else: the holder PID stays intact and the lock survives.
  fn eof_does_not_claim(wake: bool) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let home = root.to_string_lossy();
    let lock = lock_path(&home);
    mkdir_holder(&lock, "9999 otherhost");
    let script = wait_script(&lock, OURS, Duration::from_secs(10));
    assert!(run_wait_script(wake, &script));
    assert_eq!(recorded_holder(&lock), "9999 otherhost");
    assert!(lock_exists(&lock));
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
    let script = wait_script(&lock, OURS, Duration::from_secs(10));
    assert!(run_wait_script(true, &script));
    assert_eq!(recorded_holder(&lock), OURS);
    assert!(lock_exists(&lock));
  }

  #[test]
  fn waiter_times_out_when_maxwait_elapses() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let home = root.to_string_lossy();
    let lock = lock_path(&home);
    let script = wait_script(&lock, OURS, Duration::from_secs(0));
    assert!(sh_cmd(&format!("mkdir -p {}", ssh::shell_quote(&lock))));
    assert!(!run_wait_script(true, &script));
  }

  #[test]
  fn claim_reports_acquired_then_contended() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let lock = lock_path(&root.to_string_lossy());
    assert_eq!(
      sh_out(&claim_script(&lock, OURS)),
      "ACQUIRED",
      "a free path must be acquired"
    );
    assert_eq!(recorded_holder(&lock), OURS);
    assert_eq!(
      sh_out(&claim_script(&lock, "5555 otherhost")),
      "HELD",
      "a held path must report contention, not acquire"
    );
    assert_eq!(recorded_holder(&lock), OURS);
  }

  #[test]
  fn release_removes_a_lock_we_still_own() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let lock = lock_path(&root.to_string_lossy());
    mkdir_holder(&lock, OURS);
    assert_eq!(
      sh_out(&release_script(&lock, OURS)),
      "RELEASED",
      "our own lock must be removed"
    );
    assert!(!lock_exists(&lock));
  }

  /// The reclaim race: the lock changed hands after we read it, so the
  /// new owner's directory must survive our delete.
  #[test]
  fn release_spares_a_lock_another_rcargo_took() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let lock = lock_path(&root.to_string_lossy());
    mkdir_holder(&lock, "9999 testhost");
    assert_eq!(
      sh_out(&release_script(&lock, OURS)),
      "",
      "a lock owned by someone else must not be removed"
    );
    assert!(lock_exists(&lock));
    assert_eq!(recorded_holder(&lock), "9999 testhost");
  }

  #[test]
  fn release_without_a_lock_is_a_noop() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let lock = lock_path(&root.to_string_lossy());
    assert_eq!(sh_out(&release_script(&lock, OURS)), "");
    assert!(!lock_exists(&lock));
  }

  fn classify<'a>(recorded: &'a str, machine: &str) -> Holder<'a> {
    // Nothing is alive in these fixtures.
    classify_holder(recorded, machine, |_| false)
  }

  #[test]
  fn dead_holder_on_this_machine_is_reclaimable() {
    assert_eq!(
      classify("4242 testhost", "testhost"),
      Holder::DeadHere {
        owner: "4242 testhost",
        pid: 4242
      }
    );
  }

  #[test]
  fn live_holder_on_this_machine_only_waits() {
    assert_eq!(
      classify_holder("4242 testhost", "testhost", |_| true),
      Holder::Live { pid: 4242 }
    );
  }

  #[test]
  fn holder_on_another_machine_is_never_reclaimable() {
    assert_eq!(
      classify("4242 otherhost", "testhost"),
      Holder::Foreign {
        owner: "4242 otherhost",
        pid: 4242,
        machine: "otherhost",
      }
    );
  }

  /// A PID-only holder (older rcargo) cannot be judged here, and neither
  /// can any holder when this machine's name is unknown.
  #[test]
  fn holder_without_a_machine_is_never_reclaimable() {
    assert_eq!(
      classify("4242", "testhost"),
      Holder::Foreign {
        owner: "4242",
        pid: 4242,
        machine: ""
      }
    );
    assert_eq!(
      classify("4242 testhost", ""),
      Holder::Foreign {
        owner: "4242 testhost",
        pid: 4242,
        machine: "testhost",
      }
    );
  }

  #[test]
  fn empty_or_garbage_holder_is_unknown() {
    assert_eq!(classify("", "testhost"), Holder::Unknown);
    assert_eq!(classify("rcargo", "testhost"), Holder::Unknown);
  }

  #[test]
  fn owner_token_names_this_machine() {
    let token = owner_token(4242);
    let machine = local_machine();
    assert!(token.starts_with("4242"));
    // Without `hostname` the token stays a bare PID, which classifies as
    // Foreign and so can never be reclaimed.
    if machine.is_empty() {
      assert_eq!(token, "4242");
    } else {
      assert_eq!(token, format!("4242 {machine}"));
      assert_eq!(
        classify(&token, machine),
        Holder::DeadHere {
          owner: &token,
          pid: 4242
        }
      );
    }
  }
}
