use crate::ssh::{self, ssh_capture};

/// Stop the running server on the remote using the PID file.
pub fn stop_server() -> Result<(), Box<dyn std::error::Error>> {
  let host = crate::REMOTE_HOST;
  let pid_file = crate::REMOTE_PID_FILE;

  // Check if PID file exists
  let result =
    ssh_capture(host, &format!("test -f {pid_file} && echo exists"));
  if !matches!(&result, Ok(s) if s == "exists") {
    println!("No running server found (PID file does not exist).");
    return Ok(());
  }

  // Read PID
  let pid = ssh_capture(host, &format!("cat {pid_file}"))?;
  let pid: u32 = pid.parse()?;

  // Check if process is running
  let result = ssh_capture(
    host,
    &format!("kill -0 {pid} 2>/dev/null && echo running"),
  );
  if matches!(&result, Ok(s) if s == "running") {
    ssh::ssh_run(host, &format!("kill {pid}"))?;
    println!("Server (PID {pid}) stopped.");
  } else {
    println!(
      "Server (PID {pid}) is not running. Cleaning up PID file."
    );
  }

  // Remove PID file
  ssh::ssh_run(host, &format!("rm -f {pid_file}"))?;
  Ok(())
}

/// Run the server on the remote in the foreground.
pub fn run_server(
  branch: &str,
) -> Result<(), Box<dyn std::error::Error>> {
  stop_server()?;

  crate::git::ensure_repo(crate::REMOTE_HOST)?;
  crate::git::checkout_branch(crate::REMOTE_HOST, branch)?;

  ssh::ssh_run(
    crate::REMOTE_HOST,
    &format!(
      "cd {} && cargo build --release -p edwin-server",
      crate::REMOTE_REPO_PATH
    ),
  )?;

  let host = crate::REMOTE_HOST;
  let server_bin = format!(
    "{}/target/release/edwin-server",
    crate::REMOTE_REPO_PATH
  );
  let pid_file = crate::REMOTE_PID_FILE;
  let log_file = format!("{pid_file}.log");

  // Run server in background, write output to log file
  ssh::ssh_run(
    host,
    &format!(
      "nohup {server_bin} >> {log_file} 2>&1 & echo $! > {pid_file}"
    ),
  )?;

  let pid = ssh_capture(host, &format!("cat {pid_file}"))?;

  // Brief pause then check if process is still alive
  std::thread::sleep(std::time::Duration::from_secs(2));

  let result = ssh_capture(
    host,
    &format!("kill -0 {pid} 2>/dev/null && echo running"),
  );

  if matches!(&result, Ok(s) if s == "running") {
    println!("Server started on remote with PID {pid}.");
  } else {
    // Process died — show the log for debugging
    if let Ok(log) = ssh_capture(host, &format!("cat {log_file}")) {
      eprintln!("Server exited unexpectedly. Log:\n{log}");
    }
    return Err("Server process died after starting.".into());
  }

  Ok(())
}
