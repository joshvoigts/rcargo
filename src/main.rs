mod cli;
mod config;
mod deploy;
mod sandbox;
mod server;
mod shim;
mod shim_embed;
mod ssh;

use crate::config::{CargoToml, Config};
use clap::Parser;
use cli::{App, Command};
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
  let app = App::parse();

  let mut cfg = match Config::load() {
    Ok(c) => c,
    Err(e) => {
      eprintln!("warning: {e}");
      Config {
        target: String::new(),
        remote_path: None,
        package: None,
        bin: None,
        sandbox: Default::default(),
        hooks: Default::default(),
      }
    }
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

  // Always resolve remote $HOME — needed for scp
  // and sandbox path arguments.
  let home = ssh::resolve_home(&cfg.target)?;
  if remote_path.contains("$HOME") {
    remote_path = remote_path.replace("$HOME", &home);
  }

  match app.cmd {
    Some(Command::Build) => {
      build_remote(&cfg, &remote_path, &home, app.debug)?;
    }
    Some(Command::Check) => {
      check_remote(&cfg, &remote_path, &home, app.debug)?;
    }
    Some(Command::Clippy) => {
      clippy_remote(&cfg, &remote_path, &home, app.debug)?;
    }
    Some(Command::Run) => {
      server::run_server(
        &cfg,
        &remote_path,
        &home,
        &bin_name,
        app.debug,
      )?;
    }
    Some(Command::Stop) => {
      server::stop_server(&cfg.target, &remote_path, &bin_name)?;
    }
    Some(Command::Deploy) => {
      deploy::deploy(
        &cfg,
        &remote_path,
        &home,
        &bin_name,
        app.debug,
      )?;
    }
    Some(Command::Undeploy) => {
      deploy::undeploy(&cfg, &remote_path, &home, &bin_name)?;
    }
    Some(Command::Status) => {
      server::status_server(&cfg.target, &remote_path, &bin_name)?;
    }
    Some(Command::Test { args }) => {
      test_remote(
        &cfg,
        &remote_path,
        &home,
        &args,
        app.debug,
        std::time::Duration::from_secs(app.timeout),
      )?;
    }
    None => {
      eprintln!("No command specified. Use --help for usage.");
      std::process::exit(1);
    }
  }

  Ok(())
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
        "Multiple [[bin]] targets found ({}) — specify one with --bin",
        bin_targets.join(", ")
      )
      .into(),
    ),
  }
}

fn check_remote(
  config: &Config,
  remote_path: &str,
  home: &str,
  debug: bool,
) -> Result<(), Box<dyn Error>> {
  shim::sync_only(config, remote_path, home)?;

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
  home: &str,
  debug: bool,
) -> Result<(), Box<dyn Error>> {
  shim::sync_only(config, remote_path, home)?;

  server::run_hooks(config, remote_path, debug)?;

  println!("Running clippy on remote...");
  let cmd = sandbox::clippy_cmd(remote_path);
  ssh::ssh_run(&config.target, &cmd)?;

  println!("Clippy complete!");
  Ok(())
}

fn build_remote(
  config: &Config,
  remote_path: &str,
  home: &str,
  debug: bool,
) -> Result<(), Box<dyn Error>> {
  let shim_path = shim::sync_only(config, remote_path, home)?;

  server::run_hooks(config, remote_path, debug)?;

  println!("Building on remote...");
  let cmd = sandbox::inner_cmd(config, remote_path);
  match shim::run_only(
    config,
    remote_path,
    home,
    &shim_path,
    &cmd,
    debug,
  ) {
    Ok(0) => {}
    Ok(code) => {
      return Err(
        format!("Remote build failed with exit code: {code}").into(),
      );
    }
    Err(e) => {
      return Err(format!("Remote build failed: {e}").into());
    }
  }

  println!("Build complete!");
  Ok(())
}

fn test_remote(
  config: &Config,
  remote_path: &str,
  home: &str,
  extra_args: &[String],
  debug: bool,
  timeout: std::time::Duration,
) -> Result<(), Box<dyn Error>> {
  let shim_path = shim::sync_only(config, remote_path, home)?;

  server::run_hooks(config, remote_path, debug)?;

  println!("Running tests on remote...");
  let cmd = sandbox::inner_test_cmd(config, remote_path, extra_args);
  match shim::run_only_with_timeout(
    config,
    remote_path,
    home,
    &shim_path,
    &cmd,
    debug,
    timeout,
  ) {
    Ok(0) => {}
    Ok(code) => {
      return Err(
        format!("Remote tests failed with exit code: {code}").into(),
      );
    }
    Err(e) => {
      return Err(format!("Remote tests failed: {e}").into());
    }
  }

  println!("Tests complete!");
  Ok(())
}
