use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
  name = "rdeploy",
  about = "Deploy or build rust projects on remote servers",
  long_about = "A tool for deploying or building rust projects on remote servers.\n\n\
    Configuration via deploy.toml:\n  \
    target = \"myhost\"      # SSH target (hostname, user@host, or ~/.ssh/config alias)\n  \
    remote_path = \"...\"     # Optional remote path (defaults to $HOME/build/{project_name})\n  \
    [sandbox]\n  \
    enabled = true           # Enable sandboxed remote builds (default: true)\n  \
    [sandbox.env]\n  \
    DATABASE_URL = \"...\"    # Environment variables passed to the build\n  \
    [hooks]\n  \
    prebuild = \"...\"        # Shell commands run before the build (outside sandbox)"
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
  /// Run tests on remote
  Test {
    /// Extra arguments passed through to cargo test (e.g. -- --skip foo)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
  },
}
