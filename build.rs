use base64::Engine;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
  let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
  let target = env::var("TARGET").unwrap();

  let shim_binary = match target.as_str() {
    "x86_64-unknown-linux-gnu" => build_shim(&target),
    "x86_64-apple-darwin" => build_shim(&target),
    _ => String::new(),
  };

  let content = format!(
    "use base64::Engine;\n\n\
     pub const SHIM_LINUX_X86_64: &str = \
     \"{}\";\n\
     pub const SHIM_LINUX_AARCH64: &str = \"\";\n\
     pub const SHIM_MACOS_X86_64: &str = \"{}\";\n\
     pub const SHIM_MACOS_AARCH64: &str = \"\";\n\n",
    if target == "x86_64-unknown-linux-gnu" {
      &shim_binary
    } else {
      ""
    },
    if target == "x86_64-apple-darwin" {
      &shim_binary
    } else {
      ""
    },
  );

  fs::write(out_dir.join("shim_embed.rs"), content).unwrap();

  println!("cargo:rerun-if-changed=crates/rcargo-shim/src/");
  println!("cargo:rerun-if-changed=crates/rcargo-protocol/src/");
}

fn build_shim(target: &str) -> String {
  let status = Command::new("cargo")
    .args([
      "build",
      "--release",
      "-p",
      "rcargo-shim",
      "--target",
      target,
    ])
    .status();

  match status {
    Ok(s) if s.success() => {}
    _ => return String::new(),
  }

  let bin_path = format!("target/{target}/release/rcargo-shim");
  let data = match fs::read(&bin_path) {
    Ok(d) => d,
    Err(_) => return String::new(),
  };

  base64::engine::general_purpose::STANDARD.encode(&data)
}
