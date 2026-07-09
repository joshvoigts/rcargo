use crate::ssh::{self, ssh_capture};

pub fn stop_server(
  host: &str,
  remote_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
  let pid_file = format!("{remote_path}/deploy.pid");

  let result =
    ssh_capture(host, &format!("test -f {pid_file} && echo exists"));
  if !matches!(&result, Ok(s) if s == "exists") {
    println!("No running process found");
    return Ok(());
  }

  let pid = ssh_capture(host, &format!("cat {pid_file}"))?;
  let pid: u32 = pid.parse()?;

  let result = ssh_capture(
    host,
    &format!("kill -0 {pid} 2>/dev/null && echo running"),
  );
  if matches!(&result, Ok(s) if s == "running") {
    ssh::ssh_run(host, &format!("kill {pid}"))?;
    println!("Process (PID {pid}) stopped");
  } else {
    println!("Process (PID {pid}) is not running");
  }

  ssh::ssh_run(host, &format!("rm -f {pid_file}"))?;
  Ok(())
}

pub fn run_server(
  host: &str,
  remote_path: &str,
  branch: &str,
  package_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
  stop_server(host, remote_path)?;

  crate::git::sync_repo(host, remote_path, branch)?;

  println!("Building on remote...");
  let cmd = format!("cd {remote_path} && cargo build --release");
  ssh::ssh_run(host, &cmd)?;

  let bin_path =
    format!("{remote_path}/target/release/{package_name}");
  let pid_file = format!("{remote_path}/deploy.pid");
  let log_file = format!("{pid_file}.log");

  ssh::ssh_run(
    host,
    &format!(
      "cd {remote_path} && nohup {bin_path} > {log_file} 2>&1 & echo $! > {pid_file}"
    ),
  )?;

  let pid = ssh_capture(host, &format!("cat {pid_file}"))?;

  std::thread::sleep(std::time::Duration::from_secs(2));

  let result = ssh_capture(
    host,
    &format!("kill -0 {pid} 2>/dev/null && echo running"),
  );

  if matches!(&result, Ok(s) if s == "running") {
    println!("Process started with PID {pid}");
  } else {
    if let Ok(log) = ssh_capture(host, &format!("cat {log_file}")) {
      eprintln!("Process exited unexpectedly. Log:\n{log}");
    }
    return Err("Process failed to start".into());
  }

  Ok(())
}
