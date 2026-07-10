use shell_quote::Sh;
use std::{error::Error, process::Command};

/// Quote a string for safe use in POSIX shell commands.
pub fn shell_quote(s: &str) -> String {
  // Sh::quote_vec always produces ASCII, so this is safe.
  String::from_utf8(Sh::quote_vec(s)).unwrap()
}

/// Run a command on the remote via SSH, streaming output to the local terminal.
///
/// Allocates a pseudo-terminal (`-t`) so remote programs can emit
/// colors. Stderr from the PTY teardown ("Connection closed") is
/// suppressed because it's cosmetic noise.
pub fn ssh_run(host: &str, cmd: &str) -> Result<(), Box<dyn Error>> {
  let status = Command::new("ssh")
    .args(["-t", host, cmd])
    .stderr(std::process::Stdio::null())
    .status()?;

  if !status.success() {
    return Err(
      format!("SSH command failed with status: {status}").into(),
    );
  }

  Ok(())
}

/// Copy a file from the remote to the local machine via SCP.
pub fn scp_from(
  host: &str,
  remote_path: &str,
  local_path: &str,
) -> Result<(), Box<dyn Error>> {
  let remote_spec = format!("{}:{}", host, shell_quote(remote_path));
  let output = Command::new("scp")
    .args([remote_spec, local_path.to_string()])
    .output()?;

  if !output.status.success() {
    let err = String::from_utf8_lossy(&output.stderr);
    return Err(format!("SCP failed: {err}").into());
  }

  Ok(())
}

/// Resolve `$HOME` on the remote host.
pub fn resolve_home(host: &str) -> Result<String, Box<dyn Error>> {
  ssh_capture(host, "echo $HOME")
}

/// Run a command on the remote via SSH and capture stdout.
pub fn ssh_capture(
  host: &str,
  cmd: &str,
) -> Result<String, Box<dyn Error>> {
  let output = Command::new("ssh").args([host, cmd]).output()?;

  if !output.status.success() {
    let err = String::from_utf8_lossy(&output.stderr);
    return Err(format!("SSH command failed: {err}").into());
  }

  Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
