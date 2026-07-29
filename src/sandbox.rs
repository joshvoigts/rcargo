use crate::config::Config;
use crate::ssh::shell_quote;

/// Build a remote cargo check command (no sandbox needed for read-only check).
pub fn check_cmd(remote_path: &str) -> String {
  format!(
    "cd {} && CARGO_TERM_PROGRESS_WHEN=never cargo check --workspace",
    shell_quote(remote_path)
  )
}

/// Build a remote cargo clippy command.
pub fn clippy_cmd(remote_path: &str) -> String {
  format!(
    "cd {} && CARGO_TERM_PROGRESS_WHEN=never cargo clippy --workspace -- -D warnings",
    shell_quote(remote_path)
  )
}

pub fn inner_cmd(config: &Config, remote_path: &str) -> String {
  let inner = format!(
    "cd {} && CARGO_TERM_PROGRESS_WHEN=never cargo build --release",
    shell_quote(remote_path)
  );
  build_inner_full(config, &inner)
}

pub fn inner_test_cmd(
  config: &Config,
  remote_path: &str,
  extra_args: &[String],
) -> String {
  let args_str = if extra_args.is_empty() {
    String::new()
  } else {
    let quoted: Vec<String> =
      extra_args.iter().map(|a| shell_quote(a)).collect();
    format!(" {}", quoted.join(" "))
  };
  let inner = format!(
    "cd {} && CARGO_TERM_PROGRESS_WHEN=never cargo test{args_str}",
    shell_quote(remote_path)
  );
  build_inner_full(config, &inner)
}

fn build_inner_full(config: &Config, inner: &str) -> String {
  let mut env_vars: Vec<String> = config
    .sandbox
    .env
    .iter()
    .map(|(k, v)| format!("export {k}={}", shell_quote(v)))
    .collect();
  env_vars.push("export CARGO_TERM_COLOR=always".into());
  let env_prefix = env_vars.join(" && ");
  format!("{env_prefix} && {inner}")
}
