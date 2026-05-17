mod cli;
mod git;
mod server;
mod ssh;

use clap::Parser;
use cli::{App, BuildTarget, Command};

const REMOTE_HOST: &str = "edwin";
const REMOTE_REPO_PATH: &str = "/home/josh/build/edwin";
const REMOTE_PID_FILE: &str =
  "/home/josh/build/edwin/edwin-server.pid";

fn main() -> Result<(), Box<dyn std::error::Error>> {
  let app = App::parse();

  match app.cmd {
    Command::Build { target, branch } => match target {
      BuildTarget::Server => build_server(&branch)?,
      BuildTarget::Cli => build_cli(&branch)?,
    },
    Command::Run { target, branch } => match target {
      BuildTarget::Server => server::run_server(&branch)?,
      BuildTarget::Cli => panic!("Unimplemented"),
    },
    Command::Stop => server::stop_server()?,
  }

  Ok(())
}

fn build_server(
  branch: &str,
) -> Result<(), Box<dyn std::error::Error>> {
  git::ensure_repo(REMOTE_HOST)?;
  git::checkout_branch(REMOTE_HOST, branch)?;
  ssh::ssh_run(
    REMOTE_HOST,
    &format!(
      "cd {} && cargo build --release -p edwin-server",
      REMOTE_REPO_PATH
    ),
  )?;
  println!("edwin-server built successfully on remote.");
  Ok(())
}

fn build_cli(branch: &str) -> Result<(), Box<dyn std::error::Error>> {
  git::ensure_repo(REMOTE_HOST)?;
  git::checkout_branch(REMOTE_HOST, branch)?;
  ssh::ssh_run(
    REMOTE_HOST,
    &format!(
      "cd {} && cargo build --release -p edwin-cli",
      REMOTE_REPO_PATH
    ),
  )?;

  std::fs::create_dir_all("builds")?;
  ssh::scp_from(
    REMOTE_HOST,
    &format!("{}/target/release/edwin-cli", REMOTE_REPO_PATH),
    "builds/edwin-cli",
  )?;
  println!("edwin-cli copied to ./builds/edwin-cli");
  Ok(())
}
