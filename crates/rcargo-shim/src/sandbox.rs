// Fields are used on different platforms: Linux uses read/write,
// macOS additionally uses net; workdir is set but not read by apply_sandbox.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SandboxConfig {
  pub enabled: bool,
  pub workdir: String,
  pub write: Vec<String>,
  pub read: Vec<String>,
  pub net: Vec<String>,
}

impl Default for SandboxConfig {
  fn default() -> Self {
    Self {
      enabled: false,
      workdir: String::new(),
      write: Vec::new(),
      read: Vec::new(),
      net: Vec::new(),
    }
  }
}

#[cfg(target_os = "linux")]
pub fn apply_sandbox(
  config: &SandboxConfig,
) -> Result<(), Box<dyn std::error::Error>> {
  use landlock::{
    Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr,
    RulesetCreatedAttr, ABI,
  };

  if !config.enabled {
    return Ok(());
  }

  let abi = ABI::V3;
  let access_all = AccessFs::from_all(abi);
  let access_read = AccessFs::from_read(abi);

  let mut ruleset =
    Ruleset::default().handle_access(access_all)?.create()?;

  for p in &config.read {
    let fd = PathFd::new(p.as_str())?;
    ruleset = ruleset.add_rule(PathBeneath::new(fd, access_read))?;
  }

  for p in &config.write {
    let fd = PathFd::new(p.as_str())?;
    ruleset = ruleset.add_rule(PathBeneath::new(fd, access_all))?;
  }

  ruleset.restrict_self()?;
  Ok(())
}

#[cfg(target_os = "macos")]
pub fn apply_sandbox(
  config: &SandboxConfig,
) -> Result<(), Box<dyn std::error::Error>> {
  if !config.enabled {
    return Ok(());
  }

  let profile = build_seatbelt_profile(config);
  apply_seatbelt(&profile)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn apply_sandbox(
  config: &SandboxConfig,
) -> Result<(), Box<dyn std::error::Error>> {
  if !config.enabled {
    return Ok(());
  }
  eprintln!("[shim] sandbox not supported on this platform");
  Ok(())
}

#[cfg(target_os = "macos")]
fn escape_seatbelt_path(p: &str) -> String {
  p.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(target_os = "macos")]
fn build_seatbelt_profile(config: &SandboxConfig) -> String {
  let mut rules = String::from("(version 1)\n(deny default)\n");

  for p in &config.read {
    let escaped = escape_seatbelt_path(p);
    rules.push_str(&format!(
      "  (allow file-read* (subpath \"{escaped}\"))\n"
    ));
  }

  for p in &config.write {
    let escaped = escape_seatbelt_path(p);
    rules.push_str(&format!(
      "  (allow file-read* (subpath \"{escaped}\"))\n"
    ));
    rules.push_str(&format!(
      "  (allow file-write* (subpath \"{escaped}\"))\n"
    ));
  }

  if !config.net.is_empty() {
    rules.push_str("  (allow network* (remote tcp))\n");
  }

  rules.push(')');
  rules
}

#[cfg(target_os = "macos")]
fn apply_seatbelt(
  profile: &str,
) -> Result<(), Box<dyn std::error::Error>> {
  use std::ffi::{CStr, CString};
  use std::os::raw::c_char;

  extern "C" {
    fn sandbox_init(
      profile: *const c_char,
      flags: u64,
      error: *mut *mut c_char,
    ) -> std::os::raw::c_int;
  }

  let c_profile = CString::new(profile)?;
  let mut err: *mut c_char = std::ptr::null_mut();
  let rc = unsafe { sandbox_init(c_profile.as_ptr(), 0, &mut err) };
  if rc != 0 {
    let msg = unsafe { CStr::from_ptr(err) };
    return Err(
      format!("sandbox_init failed: {}", msg.to_string_lossy())
        .into(),
    );
  }
  Ok(())
}
