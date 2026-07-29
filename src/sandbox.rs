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

/// Build a remote cargo install command, sandboxed with nono.
///
/// Installs the binary from the project into `~/.cargo/bin/`.
pub fn install_cmd(
  config: &Config,
  remote_path: &str,
  home: &str,
  package: Option<&str>,
  debug: bool,
) -> String {
  let install_path = match package {
    Some(pkg) => format!("{remote_path}/{pkg}"),
    None => remote_path.to_string(),
  };
  let inner = format!(
    "cd {} && CARGO_TERM_PROGRESS_WHEN=never cargo install --path {} --force",
    shell_quote(remote_path),
    shell_quote(&install_path)
  );

  if !config.sandbox.enabled {
    return inner;
  }

  let mut args = vec![
    "NONO_NO_UPDATE_CHECK=1".into(),
    "nono".into(),
    "run".into(),
    "--silent".into(),
    "--allow-cwd".into(),
    "--workdir".into(),
    remote_path.to_string(),
  ];

  args.push("--allow".into());
  args.push(format!("{home}/.rustup"));
  args.push("--allow".into());
  args.push(format!("{home}/.cargo"));
  args.push("--allow".into());
  args.push(remote_path.to_string());
  args.push("--allow".into());
  args.push("/tmp".into());
  args.push("--read".into());
  args.push("/usr/libexec".into());
  args.push("--read".into());
  args.push("/usr/include".into());

  for w in &config.sandbox.allow.write {
    args.push("--allow".into());
    args.push(w.clone());
  }

  let default_domains = [
    "crates.io",
    "index.crates.io",
    "static.crates.io",
    "static.rust-lang.org",
    "github.com",
  ];
  for d in &default_domains {
    args.push("--allow-domain".into());
    args.push(d.to_string());
  }
  for d in &config.sandbox.allow.net {
    args.push("--allow-domain".into());
    args.push(d.clone());
  }

  args.push("--".into());

  let env_prefix: String = config
    .sandbox
    .env
    .iter()
    .map(|(k, v)| format!("export {k}={}", shell_quote(v)))
    .collect::<Vec<_>>()
    .join(" && ");
  let full_cmd = if env_prefix.is_empty() {
    format!("bash --norc --noprofile -c \"{inner}\"")
  } else {
    format!("bash --norc --noprofile -c \"{env_prefix} && {inner}\"")
  };
  args.push(full_cmd);

  let cmd = args.join(" ");
  if debug {
    eprintln!("[rcargo] sandbox cmd: {cmd}");
  }
  cmd
}

/// Build the inner cargo command (without nono wrapper).
/// Used by the shim for sandboxed execution.
pub fn inner_cmd(
  config: &Config,
  remote_path: &str,
  _home: &str,
  _debug: bool,
) -> String {
  let inner = format!(
    "cd {} && CARGO_TERM_PROGRESS_WHEN=never cargo build --release",
    shell_quote(remote_path)
  );
  build_inner_full(config, &inner)
}

/// Build the inner cargo test command (without nono wrapper).
pub fn inner_test_cmd(
  config: &Config,
  remote_path: &str,
  _home: &str,
  extra_args: &[String],
  _debug: bool,
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
