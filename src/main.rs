mod cli;
mod config;
mod deploy;
mod git;
mod sandbox;
mod server;
mod ssh;

use crate::config::Config;
use clap::Parser;
use cli::{App, Command, Step, StepName};
use std::error::Error;
use std::path::Path;
use std::process::{Command as ProcessCommand, ExitCode};
use std::time::Duration;

fn main() -> ExitCode {
  match run() {
    Ok(()) => ExitCode::SUCCESS,
    Err(e) => {
      eprintln!("{}", e);
      ExitCode::FAILURE
    }
  }
}

fn run() -> Result<(), Box<dyn Error>> {
  let app = App::parse();

  let mut cfg = Config::load()?;

  if let Some(target) = app.target {
    cfg.target = target;
  }

  if cfg.target.is_empty() {
    return Err(
      "No target specified. Provide --target flag or create rcargo.toml with: target = \"<ssh_target>\""
        .into(),
    );
  }

  // Verify SSH connectivity before doing any work.
  let status = ProcessCommand::new("ssh")
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

  let timeout = Duration::from_secs(app.timeout);

  match app.cmd {
    Command::Build => {
      run_steps(
        &cfg,
        &remote_path,
        &home,
        &one_step(StepName::Build),
        app.debug,
        timeout,
      )?;
    }
    Command::Check => {
      run_steps(
        &cfg,
        &remote_path,
        &home,
        &one_step(StepName::Check),
        app.debug,
        timeout,
      )?;
    }
    Command::Clippy => {
      run_steps(
        &cfg,
        &remote_path,
        &home,
        &one_step(StepName::Clippy),
        app.debug,
        timeout,
      )?;
    }
    Command::Lint => {
      run_steps(
        &cfg,
        &remote_path,
        &home,
        &one_step(StepName::Lint),
        app.debug,
        timeout,
      )?;
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
      run_steps(
        &cfg,
        &remote_path,
        &home,
        &[Step {
          name: StepName::Test,
          args,
        }],
        app.debug,
        timeout,
      )?;
    }
    Command::Custom(external) => {
      let name = external
        .first()
        .ok_or_else(|| "missing command name".to_string())?;
      if external.len() > 1 {
        return Err(
          format!("unexpected arguments for command '{name}'").into(),
        );
      }
      let steps = cfg
        .commands
        .get(name)
        .ok_or_else(|| {
          format!("no command '{name}' defined in [commands]")
        })?
        .iter()
        .map(|s| cli::parse_step(s))
        .collect::<Result<Vec<Step>, _>>()
        .map_err(|e: String| -> Box<dyn Error> { e.into() })?;
      run_steps(
        &cfg,
        &remote_path,
        &home,
        &steps,
        app.debug,
        timeout,
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
      if Path::new(&member_path).exists() {
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

fn one_step(name: StepName) -> Vec<Step> {
  vec![Step {
    name,
    args: Vec::new(),
  }]
}

/// Sync and run prebuild once, then execute each step in order,
/// short-circuiting on the first failure.
fn run_steps(
  config: &Config,
  remote_path: &str,
  home: &str,
  steps: &[Step],
  debug: bool,
  timeout: Duration,
) -> Result<(), Box<dyn Error>> {
  git::sync_repo(&config.target, remote_path)?;

  server::run_prebuild(config, remote_path, debug)?;

  for step in steps {
    let label = step.name.as_str();
    println!("Running {label} on remote...");
    let cmd = step_command(config, remote_path, home, step, debug);
    ssh::ssh_run_with_timeout(&config.target, &cmd, timeout)?;
    println!("{label} complete!");
  }
  Ok(())
}

fn step_command(
  config: &Config,
  remote_path: &str,
  home: &str,
  step: &Step,
  debug: bool,
) -> String {
  match step.name {
    StepName::Lint => {
      sandbox::lint_cmd(config, remote_path, home, &step.args, debug)
    }
    StepName::Clippy => sandbox::clippy_cmd(
      config,
      remote_path,
      home,
      &step.args,
      debug,
    ),
    StepName::Check => {
      sandbox::check_cmd(config, remote_path, home, &step.args, debug)
    }
    StepName::Test => {
      sandbox::test_cmd(config, remote_path, home, &step.args, debug)
    }
    StepName::Build => {
      sandbox::build_cmd(config, remote_path, home, &step.args, debug)
    }
  }
}
