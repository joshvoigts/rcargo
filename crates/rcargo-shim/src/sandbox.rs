use rcargo_protocol::SandboxConfig;

#[cfg(target_os = "linux")]
pub fn apply_sandbox(
  config: &SandboxConfig,
  proxy_port: Option<u16>,
) -> Result<(), Box<dyn std::error::Error>> {
  use landlock::{
    Access, AccessFs, AccessNet, NetPort, PathBeneath, PathFd,
    Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus, ABI,
  };

  if !config.enabled {
    return Ok(());
  }

  // Try ABI V4 for network filtering, fall back to V3
  let abi = if proxy_port.is_some() {
    ABI::V4
  } else {
    ABI::V3
  };

  let access_all = AccessFs::from_all(abi);
  let access_read = AccessFs::from_read(abi);

  let mut ruleset = Ruleset::default().handle_access(access_all)?;
  if proxy_port.is_some() {
    ruleset = ruleset.handle_access(AccessNet::ConnectTcp)?;
  }
  let mut ruleset = ruleset.create()?;

  for p in &config.read {
    let fd = PathFd::new(p.as_str())?;
    ruleset = ruleset.add_rule(PathBeneath::new(fd, access_read))?;
  }

  for p in &config.write {
    let fd = PathFd::new(p.as_str())?;
    ruleset = ruleset.add_rule(PathBeneath::new(fd, access_all))?;
  }

  // If proxy is running, restrict TCP to proxy port only.
  // UDP (including DNS) is unrestricted by Landlock ABI V4, so DNS
  // works without an explicit rule.
  if let Some(port) = proxy_port {
    let net_access = NetPort::new(port, AccessNet::ConnectTcp);
    ruleset = ruleset.add_rule(net_access)?;
  }

  let status = ruleset.restrict_self()?;

  // Check if network sandboxing is fully enforced
  if proxy_port.is_some() {
    match status.ruleset {
      RulesetStatus::FullyEnforced => {
        // All good - network is restricted to proxy
      }
      RulesetStatus::PartiallyEnforced => {
        eprintln!(
          "[shim] warning: kernel does not fully support network sandboxing (ABI V4+). \
           Network filtering will be proxy-only (no kernel enforcement). \
           Upgrade to Linux 5.19+ for TCP network sandboxing."
        );
      }
      RulesetStatus::NotEnforced => {
        eprintln!(
          "[shim] warning: Landlock not available on this kernel. \
           Network filtering will be proxy-only (no kernel enforcement)."
        );
      }
    }
  }

  Ok(())
}

#[cfg(target_os = "macos")]
pub fn apply_sandbox(
  config: &SandboxConfig,
  proxy_port: Option<u16>,
) -> Result<(), Box<dyn std::error::Error>> {
  if !config.enabled {
    return Ok(());
  }

  let profile = build_seatbelt_profile(config, proxy_port);
  apply_seatbelt(&profile)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn apply_sandbox(
  config: &SandboxConfig,
  proxy_port: Option<u16>,
) -> Result<(), Box<dyn std::error::Error>> {
  if !config.enabled {
    return Ok(());
  }

  if proxy_port.is_some() {
    eprintln!(
      "[shim] warning: kernel sandbox not available on this platform. \
       Network filtering will be proxy-only (no kernel enforcement)."
    );
  } else {
    eprintln!("[shim] sandbox not supported on this platform");
  }

  Ok(())
}

#[cfg(target_os = "macos")]
fn escape_seatbelt_path(p: &str) -> String {
  p.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(target_os = "macos")]
fn build_seatbelt_profile(
  config: &SandboxConfig,
  proxy_port: Option<u16>,
) -> String {
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

  if let Some(port) = proxy_port {
    // Allow TCP to the proxy on localhost
    rules.push_str(&format!(
      "  (allow network-outbound (remote ip \"127.0.0.1:{port}\"))\n"
    ));
    // Allow UDP for DNS resolution (Seatbelt doesn't support port-level UDP filtering)
    rules.push_str("  (allow network-outbound (remote udp))\n");
  } else if !config.net.is_empty() {
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
