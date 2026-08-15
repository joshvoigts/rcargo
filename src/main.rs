mod cli;
mod config;
mod deploy;
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
      package: None,
      bin: None,
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

  let (package_name, bin_targets) =
    detect_package_info(cfg.package.as_deref())?;
  let bin_name = resolve_bin_name(
    &package_name,
    &bin_targets,
    cfg.bin.as_deref(),
    app.bin.as_deref(),
  )?;
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
      build_remote(&cfg, &remote_path, &home, app.debug)?;
    }
    Command::Check => {
      check_remote(&cfg, &remote_path, app.debug)?;
    }
    Command::Clippy => {
      clippy_remote(&cfg, &remote_path, app.debug)?;
    }
    Command::Lint => {
      lint_remote(&cfg, &remote_path, app.debug)?;
    }
    Command::Run => {
      server::run_server(
        &cfg,
        &remote_path,
        &home,
        &branch,
        &bin_name,
        app.debug,
      )?;
    }
    Command::Stop => {
      server::stop_server(&cfg.target, &remote_path, &bin_name)?;
    }
    Command::Deploy => {
      deploy::deploy(
        &cfg,
        &remote_path,
        &home,
        cfg.package.as_deref(),
        &bin_name,
        app.debug,
      )?;
    }
    Command::Undeploy => {
      deploy::undeploy(&cfg, &remote_path, &home, &bin_name)?;
    }
    Command::Status => {
      server::status_server(&cfg.target, &remote_path, &bin_name)?;
    }
    Command::Test { args } => {
      test_remote(
        &cfg,
        &remote_path,
        &home,
        &branch,
        &args,
        app.debug,
        std::time::Duration::from_secs(app.timeout),
      )?;
    }
  }

  Ok(())
}

#[derive(serde::Deserialize)]
struct CargoToml {
  package: Package,
  #[serde(default)]
  bin: Vec<BinTarget>,
}

#[derive(serde::Deserialize)]
struct Package {
  name: String,
}

#[derive(serde::Deserialize)]
struct BinTarget {
  name: String,
}

/// Find and parse the relevant Cargo.toml, returning the package
/// name and any explicit [[bin]] target names.
fn detect_package_info(
  package: Option<&str>,
) -> Result<(String, Vec<String>), Box<dyn Error>> {
  let cargo_toml_path = match package {
    Some(pkg) => {
      let member_path = format!("{pkg}/Cargo.toml");
      if std::path::Path::new(&member_path).exists() {
        member_path
      } else {
        return Err(
          format!(
            "Package '{pkg}' not found: {member_path} does not exist"
          )
          .into(),
        );
      }
    }
    None => "Cargo.toml".to_string(),
  };

  let content = std::fs::read_to_string(&cargo_toml_path)?;
  let cargo: CargoToml = toml::from_str(&content)?;
  let bin_names: Vec<String> =
    cargo.bin.into_iter().map(|b| b.name).collect();
  Ok((cargo.package.name, bin_names))
}

/// Resolve the binary name from config/CLI/Cargo.toml.
fn resolve_bin_name(
  package_name: &str,
  bin_targets: &[String],
  config_bin: Option<&str>,
  cli_bin: Option<&str>,
) -> Result<String, Box<dyn Error>> {
  if let Some(bin) = cli_bin {
    return Ok(bin.to_string());
  }
  if let Some(bin) = config_bin {
    return Ok(bin.to_string());
  }
  match bin_targets.len() {
    0 => Ok(package_name.to_string()),
    1 => Ok(bin_targets[0].clone()),
    _ => Err(
      format!(
        "Multiple [[bin]] targets found: {bin_targets:?}. \
       Specify which one with --bin"
      )
      .into(),
    ),
  }
}

fn check_remote(
  config: &Config,
  remote_path: &str,
  debug: bool,
) -> Result<(), Box<dyn Error>> {
  git::sync_repo(&config.target, remote_path)?;

  server::run_hooks(config, remote_path, debug)?;

  println!("Checking on remote...");
  let cmd = sandbox::check_cmd(remote_path);
  ssh::ssh_run(&config.target, &cmd)?;

  println!("Check complete!");
  Ok(())
}

fn clippy_remote(
  config: &Config,
  remote_path: &str,
  debug: bool,
) -> Result<(), Box<dyn Error>> {
  git::sync_repo(&config.target, remote_path)?;

  server::run_hooks(config, remote_path, debug)?;

  println!("Running clippy on remote...");
  let cmd = sandbox::clippy_cmd(remote_path);
  ssh::ssh_run(&config.target, &cmd)?;

  println!("Clippy complete!");
  Ok(())
}

fn lint_remote(
  config: &Config,
  remote_path: &str,
  debug: bool,
) -> Result<(), Box<dyn Error>> {
  git::sync_repo(&config.target, remote_path)?;

  server::run_hooks(config, remote_path, debug)?;

  println!("Running lint on remote...");
  let cmd = sandbox::lint_cmd(remote_path);
  ssh::ssh_run(&config.target, &cmd)?;

  println!("Lint complete!");
  Ok(())
}

fn build_remote(
  config: &Config,
  remote_path: &str,
  home: &str,
  debug: bool,
) -> Result<(), Box<dyn Error>> {
  git::sync_repo(&config.target, remote_path)?;

  server::run_hooks(config, remote_path, debug)?;

  println!("Building on remote...");
  let cmd = sandbox::build_cmd(config, remote_path, home, debug);
  ssh::ssh_run(&config.target, &cmd)?;

  println!("Build complete!");
  Ok(())
}

fn test_remote(
  config: &Config,
  remote_path: &str,
  home: &str,
  _branch: &str,
  extra_args: &[String],
  debug: bool,
  timeout: std::time::Duration,
) -> Result<(), Box<dyn Error>> {
  git::sync_repo(&config.target, remote_path)?;

  server::run_hooks(config, remote_path, debug)?;

  println!("Running tests on remote...");
  let cmd =
    sandbox::test_cmd(config, remote_path, home, extra_args, debug);
  ssh::ssh_run_with_timeout(&config.target, &cmd, timeout)?;

  println!("Tests complete!");
  Ok(())
}
