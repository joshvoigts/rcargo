use std::process::Command;

fn main() {
  println!("cargo:rerun-if-changed=.git/HEAD");

  let version = Command::new("git")
    .args(["describe", "--always", "--dirty"])
    .output()
    .ok()
    .and_then(|o| {
      if o.status.success() {
        String::from_utf8(o.stdout).ok()
      } else {
        None
      }
    })
    .map(|s| s.trim().to_string())
    .unwrap_or_else(|| std::env!("CARGO_PKG_VERSION").to_string());

  println!("cargo:rustc-env=CARGO_GIT_VERSION={version}");
}
