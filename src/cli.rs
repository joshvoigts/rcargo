use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "edwin-deploy")]
pub struct App {
  #[command(subcommand)]
  pub cmd: Command,
}

#[derive(Subcommand)]
pub enum Command {
  /// Build a target on the remote
  Build {
    #[arg(value_enum)]
    target: BuildTarget,
    #[arg(long, default_value = "acp")]
    branch: String,
  },
  /// Build and run the server on the remote
  Run {
    #[arg(value_enum)]
    target: BuildTarget,
    #[arg(long, default_value = "acp")]
    branch: String,
  },
  /// Stop the running server on the remote
  Stop,
}

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq)]
pub enum BuildTarget {
  Server,
  Cli,
}
