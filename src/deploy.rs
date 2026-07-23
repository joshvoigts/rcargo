use crate::config::Config;
use crate::git;
use crate::sandbox;
use crate::server;
use crate::ssh;
use std::error::Error;

fn service_name(package_name: &str) -> String {
  format!("{package_name}.service")
}

fn generate_service_file(
  package_name: &str,
  remote_path: &str,
  home: &str,
  config: &Config,
) -> String {
  let bin_path = format!("{home}/.cargo/bin/{package_name}");

  let mut env_lines = String::new();
  for (k, v) in &config.sandbox.env {
    env_lines.push_str(&format!("Environment=\"{k}={v}\"\n"));
  }

  format!(
    "[Unit]\n\
     Description={package_name}\n\
     After=network.target\n\
     \n\
     [Service]\n\
     Type=simple\n\
     ExecStart={bin_path}\n\
     WorkingDirectory={remote_path}\n\
     {env_lines}\
     Restart=on-failure\n\
     RestartSec=5\n\
     \n\
     [Install]\n\
     WantedBy=default.target\n"
  )
}

pub fn deploy(
  config: &Config,
  remote_path: &str,
  home: &str,
  package_name: &str,
  debug: bool,
) -> Result<(), Box<dyn Error>> {
  let host = &config.target;
  let svc = service_name(package_name);

  // Stop any existing process (PID file or systemd service)
  let _ = server::stop_server(host, remote_path, package_name);

  git::sync_repo(host, remote_path)?;

  server::run_hooks(config, remote_path, debug)?;

  println!("Installing on remote...");
  let cmd = sandbox::install_cmd(config, remote_path, home, debug);
  ssh::ssh_run(host, &cmd)?;

  println!("Configuring systemd service...");
  let service_content =
    generate_service_file(package_name, remote_path, home, config);
  let service_dir = format!("{home}/.config/systemd/user");
  let service_path = format!("{service_dir}/{svc}");

  ssh::ssh_capture(
    host,
    &format!(
      "mkdir -p {dir} && cat > {path} << 'RCARGO_EOF'\n{content}RCARGO_EOF",
      dir = ssh::shell_quote(&service_dir),
      path = ssh::shell_quote(&service_path),
      content = service_content,
    ),
  )?;

  ssh::ssh_run(host, "systemctl --user daemon-reload")?;

  // Enable lingering so the service persists after logout
  let user = ssh::ssh_capture(host, "whoami")?;
  let _ = ssh::ssh_capture(
    host,
    &format!("loginctl enable-linger {user} 2>/dev/null"),
  );

  println!("Starting service...");
  ssh::ssh_run(host, &format!("systemctl --user restart {svc}"))?;
  ssh::ssh_run(host, &format!("systemctl --user enable {svc}"))?;

  println!("Deployed {package_name} as systemd user service");
  Ok(())
}

pub fn undeploy(
  config: &Config,
  remote_path: &str,
  home: &str,
  package_name: &str,
) -> Result<(), Box<dyn Error>> {
  let host = &config.target;
  let svc = service_name(package_name);

  let _ = ssh::ssh_run(host, &format!("systemctl --user stop {svc}"));
  let _ =
    ssh::ssh_run(host, &format!("systemctl --user disable {svc}"));

  let service_path = format!("{home}/.config/systemd/user/{svc}");
  let _ = ssh::ssh_run(
    host,
    &format!("rm -f {}", ssh::shell_quote(&service_path)),
  );

  ssh::ssh_run(host, "systemctl --user daemon-reload")?;

  let bin_path = format!("{home}/.cargo/bin/{package_name}");
  let _ = ssh::ssh_run(
    host,
    &format!("rm -f {}", ssh::shell_quote(&bin_path)),
  );

  println!("Undeployed {package_name}");
  Ok(())
}
