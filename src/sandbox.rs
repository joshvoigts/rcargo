use crate::config::Config;
use crate::ssh::shell_quote;

/// Build a remote cargo build command, sandboxed with nono.
///
/// `home` is the resolved `$HOME` on the remote host.
///
/// nono uses Landlock (Linux) / Seatbelt (macOS) for kernel-level
/// filesystem sandboxing — deny-all reads, then whitelist specific
/// paths. Unlike bubblewrap's mount-namespace approach, binary
/// execution works because the filesystem is intact; the kernel
/// just denies access to non-whitelisted paths.
///
/// Network is blocked by default (--block-net), with specific
/// domains whitelisted via --allow-domain.
pub fn build_cmd(
  config: &Config,
  remote_path: &str,
  home: &str,
  debug: bool,
) -> String {
  let inner = format!(
    "cd {} && cargo build --release",
    shell_quote(remote_path)
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

  let env_prefix: String = config
    .sandbox
    .env
    .iter()
    .map(|(k, v)| format!("export {k}={}", shell_quote(v)))
    .collect::<Vec<_>>()
    .join(" && ");
  let full_cmd = if env_prefix.is_empty() {
    format!("bash -c \"{inner}\"")
  } else {
    format!("bash -c \"{env_prefix} && {inner}\"")
  };
  args.push(full_cmd);

  let cmd = args.join(" ");
  if debug {
    eprintln!("[rdeploy] sandbox cmd: {cmd}");
  }
  cmd
}
