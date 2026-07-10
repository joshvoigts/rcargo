use crate::config::Config;

/// Build a remote cargo build command, sandboxed with bwrap + zerobox proxy.
///
/// `home` is the resolved `$HOME` on the remote host.
///
/// We call bwrap directly for whitelist-based filesystem sandboxing
/// instead of using zerobox's `--allow-read`, which on Linux creates
/// an empty tmpfs root with individual `--ro-bind` mounts that break
/// cargo/rustc binary execution (EACCES on execve).
///
/// zerobox wraps the bwrap command with `--no-sandbox` to provide the
/// network proxy (restricting outbound traffic to allowed domains).
pub fn build_cmd(
  config: &Config,
  remote_path: &str,
  home: &str,
) -> String {
  let inner = format!("cd {remote_path} && cargo build --release");

  if !config.sandbox.enabled {
    return inner;
  }

  // bwrap: whitelist-based filesystem sandbox
  let mut b = vec!["bwrap".to_string()];

  b.push("--tmpfs".into());
  b.push("/".into());

  for p in ["/usr", "/lib", "/lib64", "/bin", "/sbin", "/etc"] {
    b.push("--ro-bind".into());
    b.push(p.into());
    b.push(p.into());
  }

  b.push("--dev".into());
  b.push("/dev".into());
  b.push("--proc".into());
  b.push("/proc".into());
  b.push("--tmpfs".into());
  b.push("/tmp".into());

  for p in [format!("{home}/.rustup"), format!("{home}/.cargo")] {
    b.push("--ro-bind".into());
    b.push(p.clone());
    b.push(p);
  }

  for p in [
    format!("{home}/.rustup"),
    format!("{home}/.cargo"),
    remote_path.to_string(),
  ] {
    b.push("--bind".into());
    b.push(p.clone());
    b.push(p);
  }

  for w in &config.sandbox.allow.write {
    b.push("--bind".into());
    b.push(w.clone());
    b.push(w.clone());
  }

  b.push("--die-with-parent".into());
  b.push("--new-session".into());
  b.push("--".into());
  b.push("bash".into());
  b.push("-c".into());
  b.push(inner);

  let bwrap_cmd = b.join(" ");

  // zerobox: network proxy only
  let mut net = vec![
    "crates.io".to_string(),
    "index.crates.io".to_string(),
    "static.crates.io".to_string(),
    "static.rust-lang.org".to_string(),
    "github.com".to_string(),
  ];
  net.extend(config.sandbox.allow.net.iter().cloned());

  let mut args = vec!["zerobox".to_string()];
  args.push("--no-sandbox".into());
  args.push("--allow-env".into());
  args.push("--debug".into());
  args.push(format!("--allow-net={}", net.join(",")));
  args.push("--".into());
  args.push(bwrap_cmd);

  let cmd = args.join(" ");
  eprintln!("[rdeploy] sandbox cmd: {cmd}");
  cmd
}
