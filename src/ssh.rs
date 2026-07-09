use std::process::Command;

/// Run a command on the remote via SSH, streaming output to the local terminal.
pub fn ssh_run(
  host: &str,
  cmd: &str,
) -> Result<(), Box<dyn std::error::Error>> {
  let status = Command::new("ssh")
    .args([host, cmd])
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
) -> Result<(), Box<dyn std::error::Error>> {
  let output = Command::new("scp")
    .args([format!("{host}:{remote_path}"), local_path.to_string()])
    .output()?;

  if !output.status.success() {
    let err = String::from_utf8_lossy(&output.stderr);
    return Err(format!("SCP failed: {err}").into());
  }

  Ok(())
}

/// Run a command on the remote via SSH and capture stdout.
pub fn ssh_capture(
  host: &str,
  cmd: &str,
) -> Result<String, Box<dyn std::error::Error>> {
  let output = Command::new("ssh").args([host, cmd]).output()?;

  if !output.status.success() {
    let err = String::from_utf8_lossy(&output.stderr);
    return Err(format!("SSH command failed: {err}").into());
  }

  Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
