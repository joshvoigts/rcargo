use crate::config::Config;

/// Build a remote cargo build command, optionally sandboxed
/// with zerobox.
pub fn build_cmd(config: &Config, remote_path: &str) -> String {
  let inner = format!("cd {remote_path} && cargo build --release");

  if !config.sandbox.enabled {
    return inner;
  }

  let mut args = vec!["zerobox".to_string()];

  // Defaults needed for cargo/rustc to work
  args.push("--allow-env".into());
  args.push("--allow-write=$HOME/.cargo".into());
  args.push("--allow-write=$HOME/.rustup".into());
  // Shell configs (various shells)
  args.push("--allow-read=$HOME/.profile".into());
  args.push("--allow-read=$HOME/.bashrc".into());
  args.push("--allow-read=$HOME/.bash_profile".into());
  args.push("--allow-read=$HOME/.zshrc".into());
  args.push("--allow-read=$HOME/.zshenv".into());
  // Git/cargo config
  args.push("--allow-read=$HOME/.gitconfig".into());
  args.push("--allow-read=$HOME/.config/git".into());
  args.push(
    "--allow-net=crates.io,index.crates.io,static.crates.io,static.rust-lang.org,github.com"
      .into(),
  );

  // Project directory access
  args.push(format!("--allow-read={remote_path}"));
  args.push(format!("--allow-write={remote_path}"));

  // Config extras
  let allow = &config.sandbox.allow;
  for r in &allow.read {
    args.push(format!("--allow-read={r}"));
  }
  for w in &allow.write {
    args.push(format!("--allow-write={w}"));
  }
  if !allow.net.is_empty() {
    args.push(format!("--allow-net={}", allow.net.join(",")));
  }

  let deny = &config.sandbox.deny;
  for r in &deny.read {
    args.push(format!("--deny-read={r}"));
  }
  for w in &deny.write {
    args.push(format!("--deny-write={w}"));
  }

  args.push(format!("bash -c \"{inner}\""));

  args.join(" ")
}
