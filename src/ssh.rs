use std::io::{self, BufRead};
use std::process::{Command, Stdio};

/// Run a command on the remote via SSH, streaming output to the local terminal.
pub fn ssh_run(
  host: &str,
  cmd: &str,
) -> Result<(), Box<dyn std::error::Error>> {
  let mut child = Command::new("ssh")
    .args([host, cmd])
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()?;

  // Stream stdout
  let stdout = child.stdout.take().unwrap();
  let stderr = child.stderr.take().unwrap();

  let stdout_reader = io::BufReader::new(stdout);
  let stderr_reader = io::BufReader::new(stderr);

  // Use threads to stream both stdout and stderr
  let stdout_handle = std::thread::spawn(move || {
    for line in stdout_reader.lines() {
      if let Ok(line) = line {
        println!("{line}");
      }
    }
  });

  let stderr_handle = std::thread::spawn(move || {
    for line in stderr_reader.lines() {
      if let Ok(line) = line {
        eprintln!("{line}");
      }
    }
  });

  stdout_handle.join().unwrap();
  stderr_handle.join().unwrap();

  let status = child.wait()?;
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
