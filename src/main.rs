mod cli;
mod config;
mod git;
mod sandbox;
mod server;
mod ssh;

use crate::config::Config;
use clap::Parser;
use cli::{App, Command};
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
  let app = App::parse();

  let mut cfg = match Config::load() {
    Ok(c) => c,
    Err(_) => Config {
      target: String::new(),
      remote_path: None,
      sandbox: Default::default(),
      hooks: Default::default(),
    },
  };

  if let Some(target) = app.target {
    cfg.target = target;
  }

  if cfg.target.is_empty() {
    return Err(
      "No target specified. Provide --target flag or create deploy.toml with: target = \"<ssh_target>\""
        .into(),
    );
  }

  // Verify SSH connectivity before doing any work.
  let status = std::process::Command::new("ssh")
    .args([
      "-o",
      "BatchMode=yes",
      "-o",
      "ConnectTimeout=5",
      &cfg.target,
      "true",
    ])
    .status();
  if !matches!(status, Ok(s) if s.success()) {
    return Err(
      format!(
        "Cannot connect to remote host '{}' via SSH",
        cfg.target
      )
      .into(),
    );
  }

  let package_name = detect_package_name()?;
  let mut remote_path = cfg.remote_path(&package_name);

  // Always resolve remote $HOME — needed for rsync,
  // scp, and sandbox path arguments.
  let home = ssh::resolve_home(&cfg.target)?;
  if remote_path.contains("$HOME") {
    remote_path = remote_path.replace("$HOME", &home);
  }

  let branch = match &app.branch {
    Some(b) => b.clone(),
    None => git::current_branch()?,
  };

  match app.cmd {
    Command::Build => {
      build_remote(
        &cfg,
        &remote_path,
        &home,
        &branch,
        &package_name,
        app.debug,
      )?;
    }
    Command::Run => {
      server::run_server(
        &cfg,
        &remote_path,
        &home,
        &branch,
        &package_name,
        app.debug,
      )?;
    }
    Command::Stop => {
      server::stop_server(&cfg.target, &remote_path)?;
    }
    Command::Test { args } => {
      test_remote(
        &cfg,
        &remote_path,
        &home,
        &branch,
        &args,
        app.debug,
      )?;
    }
  }

  Ok(())
}

#[derive(serde::Deserialize)]
struct CargoToml {
  package: Package,
}

#[derive(serde::Deserialize)]
struct Package {
  name: String,
}

fn detect_package_name() -> Result<String, Box<dyn Error>> {
  let content = std::fs::read_to_string("Cargo.toml")?;
  let cargo: CargoToml = toml::from_str(&content)?;
  Ok(cargo.package.name)
}

fn build_remote(
  config: &Config,
  remote_path: &str,
  home: &str,
  _branch: &str,
  package_name: &str,
  debug: bool,
) -> Result<(), Box<dyn Error>> {
  git::sync_repo(&config.target, remote_path)?;

  server::run_hooks(config, remote_path, debug)?;

  println!("Building on remote...");
  let cmd = sandbox::build_cmd(config, remote_path, home, debug);
  ssh::ssh_run(&config.target, &cmd)?;

  std::fs::create_dir_all("builds")?;
  let remote_bin =
    format!("{remote_path}/target/release/{package_name}");
  let local_bin = format!("builds/{package_name}");

  println!("Copying binary back...");
  ssh::scp_from(&config.target, &remote_bin, &local_bin)?;

  println!("Build complete! Binary at: {local_bin}");
  Ok(())
}

fn test_remote(
  config: &Config,
  remote_path: &str,
  home: &str,
  _branch: &str,
  extra_args: &[String],
  debug: bool,
) -> Result<(), Box<dyn Error>> {
  git::sync_repo(&config.target, remote_path)?;

  server::run_hooks(config, remote_path, debug)?;

  println!("Running tests on remote...");
  let cmd =
    sandbox::test_cmd(config, remote_path, home, extra_args, debug);
  ssh::ssh_run(&config.target, &cmd)?;

  println!("Tests complete!");
  Ok(())
}
