use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
  name = "rcargo",
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

  /// Workspace member to install (overrides deploy.toml)
  #[arg(long, short)]
  pub package: Option<String>,

  /// Binary name override (defaults to auto-detect from [[bin]] target)
  #[arg(long)]
  pub bin: Option<String>,

  /// Enable debug output
  #[arg(long)]
  pub debug: bool,

  /// Timeout in seconds for remote commands (default: 600)
  #[arg(long, default_value_t = 600)]
  pub timeout: u64,
}

#[derive(Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum Command {
  /// Build on remote
  Build,
  /// Check code on remote (cargo check)
  Check,
  /// Run clippy on remote (cargo clippy)
  Clippy,
  /// Run lint on remote (cargo lint, provided by a lint xtask)
  Lint,
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
  /// Run multiple commands on the remote in a single session
  Steps {
    /// Commands and their args, in order: each known command name
    /// (lint, clippy, check, test, build) starts a new step, and the
    /// tokens after it (until the next command name) are its args.
    /// e.g. `rcargo steps lint test --workspace -q`
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    raw: Vec<String>,
  },
  /// Install binary and set up as systemd user service
  Deploy,
  /// Remove systemd service and installed binary
  Undeploy,
  /// Show status of deployed or running process
  Status,
}

/// A single command within a `steps` run.
#[derive(Clone, Debug)]
pub struct Step {
  pub name: StepName,
  pub args: Vec<String>,
}

/// The supported step commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepName {
  Lint,
  Clippy,
  Check,
  Test,
  Build,
}

impl StepName {
  pub fn as_str(self) -> &'static str {
    match self {
      StepName::Lint => "lint",
      StepName::Clippy => "clippy",
      StepName::Check => "check",
      StepName::Test => "test",
      StepName::Build => "build",
    }
  }
}

/// Parse raw step tokens into ordered `Step`s. A token that names a known
/// command starts a new step; any other token becomes an arg of the current
/// step. e.g. `["lint", "test", "--workspace", "-q"]` -> lint; test --workspace -q.
pub fn parse_steps(raw: &[String]) -> Result<Vec<Step>, String> {
  let mut steps: Vec<Step> = Vec::new();
  let mut current: Option<Step> = None;
  for tok in raw {
    if let Some(name) = step_name(tok) {
      if let Some(step) = current.take() {
        steps.push(step);
      }
      current = Some(Step {
        name,
        args: Vec::new(),
      });
    } else if let Some(step) = current.as_mut() {
      step.args.push(tok.clone());
    } else {
      return Err(format!("'{tok}' is not a known command"));
    }
  }
  if let Some(step) = current.take() {
    steps.push(step);
  }
  if steps.is_empty() {
    return Err("no commands specified".into());
  }
  Ok(steps)
}

fn step_name(s: &str) -> Option<StepName> {
  match s {
    "lint" => Some(StepName::Lint),
    "clippy" => Some(StepName::Clippy),
    "check" => Some(StepName::Check),
    "test" => Some(StepName::Test),
    "build" => Some(StepName::Build),
    _ => None,
  }
}
