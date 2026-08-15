use crate::config::Config;
use crate::ssh::shell_quote;

/// Wrap an inner remote command in a nono sandbox unless disabled.
///
/// nono uses Landlock (Linux) / Seatbelt (macOS) for kernel-level
/// filesystem sandboxing — deny-all reads, then whitelist specific
/// paths. Unlike bubblewrap's mount-namespace approach, binary
/// execution works because the filesystem is intact; the kernel
/// just denies access to non-whitelisted paths.
///
/// Network is blocked by default (--block-net), with specific
/// domains whitelisted via --allow-domain.
fn sandbox_cmd(
  config: &Config,
  remote_path: &str,
  home: &str,
  debug: bool,
  inner: &str,
) -> String {
  if !config.sandbox.enabled {
    return inner.to_string();
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

  // Filesystem: read+write for cargo caches and project dir.
  // nono's default profile includes system_read_linux_core
  // which grants read access to /usr, /lib, /bin, /dev, /proc, etc.
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

  // Network: allow only specific domains via proxy filtering.
  // Everything else is blocked by the proxy.
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

  let mut env_vars: Vec<String> = config
    .sandbox
    .env
    .iter()
    .map(|(k, v)| format!("export {k}={}", shell_quote(v)))
    .collect();
  env_vars.push("export CARGO_TERM_COLOR=always".into());
  let env_prefix = env_vars.join(" && ");
  let full_cmd =
    format!("bash --norc --noprofile -c \"{env_prefix} && {inner}\"");
  args.push(full_cmd);

  let cmd = args.join(" ");
  if debug {
    eprintln!("[rcargo] sandbox cmd: {cmd}");
  }
  cmd
}

/// Quote and join extra args into a single shell-safe string, prefixed
/// with a space (or empty when there are none).
fn quoted_args(args: &[String]) -> String {
  if args.is_empty() {
    String::new()
  } else {
    let quoted: Vec<String> =
      args.iter().map(|a| shell_quote(a)).collect();
    format!(" {}", quoted.join(" "))
  }
}

/// Build a remote cargo build command, sandboxed with nono.
pub fn build_cmd(
  config: &Config,
  remote_path: &str,
  home: &str,
  extra_args: &[String],
  debug: bool,
) -> String {
  let inner = format!(
    "cd {} && CARGO_TERM_PROGRESS_WHEN=never cargo build --release{}",
    shell_quote(remote_path),
    quoted_args(extra_args)
  );
  sandbox_cmd(config, remote_path, home, debug, &inner)
}

/// Build a remote cargo test command, sandboxed with nono.
pub fn test_cmd(
  config: &Config,
  remote_path: &str,
  home: &str,
  extra_args: &[String],
  debug: bool,
) -> String {
  let inner = format!(
    "cd {} && CARGO_TERM_PROGRESS_WHEN=never cargo test{}",
    shell_quote(remote_path),
    quoted_args(extra_args)
  );
  sandbox_cmd(config, remote_path, home, debug, &inner)
}

/// Build a remote cargo check command, sandboxed with nono.
pub fn check_cmd(
  config: &Config,
  remote_path: &str,
  home: &str,
  extra_args: &[String],
  debug: bool,
) -> String {
  let inner = format!(
    "cd {} && CARGO_TERM_PROGRESS_WHEN=never cargo check --workspace{}",
    shell_quote(remote_path),
    quoted_args(extra_args)
  );
  sandbox_cmd(config, remote_path, home, debug, &inner)
}

/// Build a remote cargo clippy command, sandboxed with nono.
pub fn clippy_cmd(
  config: &Config,
  remote_path: &str,
  home: &str,
  extra_args: &[String],
  debug: bool,
) -> String {
  let inner = format!(
    "cd {} && CARGO_TERM_PROGRESS_WHEN=never cargo clippy --workspace -- -D warnings{}",
    shell_quote(remote_path),
    quoted_args(extra_args)
  );
  sandbox_cmd(config, remote_path, home, debug, &inner)
}

/// Build a remote cargo lint command (provided by a lint xtask),
/// sandboxed with nono.
pub fn lint_cmd(
  config: &Config,
  remote_path: &str,
  home: &str,
  extra_args: &[String],
  debug: bool,
) -> String {
  let inner = format!(
    "cd {} && CARGO_TERM_PROGRESS_WHEN=never cargo lint{}",
    shell_quote(remote_path),
    quoted_args(extra_args)
  );
  sandbox_cmd(config, remote_path, home, debug, &inner)
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
    shell_quote(&install_path),
  );
  sandbox_cmd(config, remote_path, home, debug, &inner)
}
