use crate::config::Config;
use crate::git;
use crate::sandbox;
use crate::ssh;
use std::{error::Error, time::Duration};

/// Run prebuild hooks on the remote host, outside the sandbox.
///
/// When `debug` is false, hook output is suppressed.
pub fn run_hooks(
  config: &Config,
  remote_path: &str,
  debug: bool,
) -> Result<(), Box<dyn Error>> {
  if let Some(ref hook) = config.hooks.prebuild {
    let env_prefix: String = config
      .sandbox
      .env
      .iter()
      .map(|(k, v)| format!("export {k}={}", ssh::shell_quote(v)))
      .collect::<Vec<_>>()
      .join(" && ");
    let hook_cmd = if env_prefix.is_empty() {
      format!(
        "cd {} && {}",
        ssh::shell_quote(remote_path),
        hook.as_command()
      )
    } else {
      format!(
        "cd {} && {env_prefix} && {}",
        ssh::shell_quote(remote_path),
        hook.as_command()
      )
    };
    if debug {
      println!("Running prebuild hook...");
      ssh::ssh_run(&config.target, &hook_cmd)?;
    } else {
      ssh::ssh_capture(&config.target, &hook_cmd)?;
    }
  }
  Ok(())
}

pub fn stop_server(
  host: &str,
  remote_path: &str,
) -> Result<(), Box<dyn Error>> {
  let pid_file =
    ssh::shell_quote(&format!("{remote_path}/rdeploy.pid"));

  let result = ssh::ssh_capture(
    host,
    &format!("test -f {pid_file} && echo exists"),
  );
  if !matches!(&result, Ok(s) if s == "exists") {
    println!("No running process found");
    return Ok(());
  }

  let pid = ssh::ssh_capture(host, &format!("cat {pid_file}"))?;
  let pid: u32 = pid.parse()?;

  let result = ssh::ssh_capture(
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
  config: &Config,
  remote_path: &str,
  home: &str,
  _branch: &str,
  package_name: &str,
  debug: bool,
) -> Result<(), Box<dyn Error>> {
  let host = &config.target;

  stop_server(host, remote_path)?;

  git::sync_repo(host, remote_path)?;

  run_hooks(config, remote_path, debug)?;

  println!("Building on remote...");
  let cmd = sandbox::build_cmd(config, remote_path, home, debug, &[]);
  ssh::ssh_run(host, &cmd)?;

  let bin_path = ssh::shell_quote(&format!(
    "{remote_path}/target/release/{package_name}"
  ));
  let pid_file =
    ssh::shell_quote(&format!("{remote_path}/rdeploy.pid"));
  let log_file =
    ssh::shell_quote(&format!("{remote_path}/rdeploy.pid.log"));

  let cd_path = ssh::shell_quote(remote_path);
  ssh::ssh_run(
    host,
    &format!(
      "cd {cd_path} && nohup {bin_path} > {log_file} 2>&1 & echo $! > {pid_file}"
    ),
  )?;

  let pid = ssh::ssh_capture(host, &format!("cat {pid_file}"))?;

  std::thread::sleep(Duration::from_secs(2));

  let result = ssh::ssh_capture(
    host,
    &format!("kill -0 {pid} 2>/dev/null && echo running"),
  );

  if matches!(&result, Ok(s) if s == "running") {
    println!("Process started with PID {pid}");
  } else {
    if let Ok(log) =
      ssh::ssh_capture(host, &format!("cat {log_file}"))
    {
      eprintln!("Process exited unexpectedly. Log:\n{log}");
    }
    return Err("Process failed to start".into());
  }

  Ok(())
}
