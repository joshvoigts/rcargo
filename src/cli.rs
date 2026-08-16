use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
  name = "rcargo",
  about = "Deploy or build rust projects on remote servers",
  allow_external_subcommands = true,
  long_about = "A tool for deploying or building rust projects on remote servers.\n\n\
    Configuration via rcargo.toml:\n  \
    target = \"myhost\"      # SSH target (hostname, user@host, or ~/.ssh/config alias)\n  \
    remote_path = \"...\"     # Optional remote path (overrides remote_build_dir; defaults to $HOME/build/{project_name})\n  \
    remote_build_dir = \"...\" # Optional build dir; repo lands at {remote_build_dir}/{project_name}\n  \
    [sandbox]\n  \
    enabled = true           # Enable sandboxed remote builds (default: true)\n  \
    [sandbox.env]\n  \
    DATABASE_URL = \"...\"    # Environment variables passed to the build\n  \
    prebuild = \"...\"        # Shell commands run before the build (outside sandbox)"
)]
pub struct App {
  #[command(subcommand)]
  pub cmd: Command,

  /// Override the target from rcargo.toml
  #[arg(long, short)]
  pub target: Option<String>,

  /// Override the branch (defaults to current branch)
  #[arg(long, short)]
  pub branch: Option<String>,

  /// Workspace member to install (overrides rcargo.toml)
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
  /// Run a user-defined command from `[commands]` in the config
  #[command(external_subcommand)]
  Custom(Vec<String>),
  /// Install binary and set up as systemd user service
  Deploy,
  /// Remove systemd service and installed binary
  Undeploy,
  /// Show status of deployed or running process
  Status,
}

/// A single step within a `[commands]` entry.
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

/// Parse a single step string like `"test --workspace -q"` into a `Step`.
pub fn parse_step(s: &str) -> Result<Step, String> {
  let mut parts = s.split_whitespace();
  let name = parts
    .next()
    .ok_or_else(|| "empty command in [commands]".to_string())?;
  let name = step_name(name)
    .ok_or_else(|| format!("'{name}' is not a known command"))?;
  let args: Vec<String> = parts.map(|p| p.to_string()).collect();
  Ok(Step { name, args })
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
