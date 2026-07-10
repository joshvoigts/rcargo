use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
  name = "rdeploy",
  about = "Deploy or build rust projects on remote servers",
  long_about = "A tool for deploying or building rust projects on remote servers.\n\nConfiguration via deploy.toml:\n  target = \"myhost\"      # SSH target (hostname, user@host, or ~/.ssh/config alias)\n  remote_path = \"...\"     # Optional remote path (defaults to $HOME/build/{project_name})"
)]
pub struct App {
  #[command(subcommand)]
  pub cmd: Command,

  /// Override the target from deploy.toml
  #[arg(long, short)]
  pub target: Option<String>,

  /// Override the branch (defaults to current branch)
  #[arg(long, short)]
  pub branch: Option<String>,

  /// Enable debug output
  #[arg(long)]
  pub debug: bool,
}

#[derive(Subcommand)]
pub enum Command {
  /// Build on remote and copy binary back
  Build,
  /// Build and run on remote
  Run,
  /// Stop the running process on remote
  Stop,
}
