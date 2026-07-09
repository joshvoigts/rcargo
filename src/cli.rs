use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
  name = "deploy",
  about = "Deploy Rust projects to remote servers",
  long_about = "A tool for deploying Rust projects to remote servers.\n\nConfiguration via deploy.toml (optional if using --target flag):\n  target = \"your-server\"  # SSH host to deploy to\n  remote_path = \"...\"      # Optional remote path (defaults to $HOME/build/{project_name})"
)]
pub struct App {
  #[command(subcommand)]
  pub cmd: Command,

  /// Override the target host from deploy.toml
  #[arg(long, short)]
  pub target: Option<String>,

  /// Override the branch (defaults to current branch)
  #[arg(long, short)]
  pub branch: Option<String>,
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
