use base64::Engine;

// Empty by default — filled in by cross-compilation CI or `just build-all`.
// See docs/shim-design.md for the build process.
pub const SHIM_LINUX_X86_64: &str = "";
pub const SHIM_LINUX_AARCH64: &str = "";
pub const SHIM_MACOS_X86_64: &str = "";
pub const SHIM_MACOS_AARCH64: &str = "";

pub fn has_embedded_shim() -> bool {
  !(SHIM_LINUX_X86_64.is_empty()
    && SHIM_LINUX_AARCH64.is_empty()
    && SHIM_MACOS_X86_64.is_empty()
    && SHIM_MACOS_AARCH64.is_empty())
}

pub fn get_shim_binary(os: &str, arch: &str) -> Option<Vec<u8>> {
  let b64 = match (os, arch) {
    ("Linux", "x86_64") => SHIM_LINUX_X86_64,
    ("Linux", "aarch64") => SHIM_LINUX_AARCH64,
    ("Darwin", "x86_64") => SHIM_MACOS_X86_64,
    ("Darwin", "aarch64") => SHIM_MACOS_AARCH64,
    _ => return None,
  };
  if b64.is_empty() {
    return None;
  }
  base64::engine::general_purpose::STANDARD.decode(b64).ok()
}
