mod cli;
mod config;
mod git;
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
    },
  };

  if let Some(target) = app.target {
    cfg.target = target;
  }

  if cfg.target.is_empty() {
    return Err(
      "No target specified. Provide --target flag or create deploy.toml with: target = \"your-server\""
        .into(),
    );
  }

  let package_name = detect_package_name()?;
  let remote_path = cfg.remote_path(&package_name);

  let branch = match &app.branch {
    Some(b) => b.clone(),
    None => git::current_branch()?,
  };

  match app.cmd {
    Command::Build => {
      build_remote(
        &cfg.target,
        &remote_path,
        &branch,
        &package_name,
      )?;
    }
    Command::Run => {
      server::run_server(
        &cfg.target,
        &remote_path,
        &branch,
        &package_name,
      )?;
    }
    Command::Stop => {
      server::stop_server(&cfg.target, &remote_path)?;
    }
  }

  Ok(())
}

fn detect_package_name() -> Result<String, Box<dyn Error>> {
  #[derive(serde::Deserialize)]
  struct CargoToml {
    package: Package,
  }
  #[derive(serde::Deserialize)]
  struct Package {
    name: String,
  }

  let content = std::fs::read_to_string("Cargo.toml")?;
  let cargo: CargoToml = toml::from_str(&content)?;
  Ok(cargo.package.name)
}

fn build_remote(
  host: &str,
  remote_path: &str,
  branch: &str,
  package_name: &str,
) -> Result<(), Box<dyn Error>> {
  git::sync_repo(host, remote_path, branch)?;

  println!("Building on remote...");
  let cmd = format!("cd {remote_path} && cargo build --release");
  ssh::ssh_run(host, &cmd)?;

  std::fs::create_dir_all("builds")?;
  let remote_bin =
    format!("{remote_path}/target/release/{package_name}");
  let local_bin = format!("builds/{package_name}");

  println!("Copying binary back...");
  ssh::scp_from(host, &remote_bin, &local_bin)?;

  println!("Build complete! Binary at: {local_bin}");
  Ok(())
}
