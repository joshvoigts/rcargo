use crate::config::Config;
use crate::shim_embed;
use crate::ssh::{self, shell_quote};
use base64::Engine;
use ignore::WalkBuilder;
use rcargo_protocol::{self as proto, Message, ProtocolWriter};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::process::{Child, Command, Stdio};
use std::time::UNIX_EPOCH;

const SHIM_DIR: &str = ".rcargo";
const SHIM_NAME: &str = "shim";

pub struct ShimSession {
  child: Child,
  writer: ProtocolWriter<BufWriter<Box<dyn Write>>>,
  reader: BufReader<Box<dyn Read + Send>>,
}

impl ShimSession {
  fn open(
    host: &str,
    shim_path: &str,
  ) -> Result<Self, Box<dyn std::error::Error>> {
    let mut child = Command::new("ssh")
      .args(["-T", "-o", "BatchMode=yes", host, shim_path])
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::piped())
      .spawn()?;

    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    let writer = ProtocolWriter::new(BufWriter::new(
      Box::new(stdin) as Box<dyn Write>
    ));
    let reader =
      BufReader::new(Box::new(stdout) as Box<dyn Read + Send>);

    // Drain stderr in the background.
    std::thread::spawn(move || {
      let mut buf = [0u8; 4096];
      let mut rdr = io::BufReader::new(stderr);
      loop {
        let n = match rdr.read(&mut buf) {
          Ok(0) | Err(_) => break,
          Ok(n) => n,
        };
        let _ = io::stderr().write_all(&buf[..n]);
      }
    });

    Ok(Self {
      child,
      writer,
      reader,
    })
  }

  fn send(&mut self, msg: &Message) -> io::Result<()> {
    self.writer.send(msg)
  }

  fn receive(&mut self) -> io::Result<Message> {
    proto::read_message(&mut self.reader)
  }

  fn stream_output(&mut self) -> io::Result<i32> {
    let mut buf = [0u8; 4096];
    loop {
      let n = self.reader.read(&mut buf)?;
      if n == 0 {
        break;
      }
      io::stdout().write_all(&buf[..n])?;
      io::stdout().flush()?;
    }
    let status = self.child.wait()?;
    Ok(status.code().unwrap_or(1))
  }

  fn stream_output_with_timeout(
    &mut self,
    timeout: std::time::Duration,
  ) -> io::Result<i32> {
    use std::sync::mpsc;
    use std::time::Instant;

    let (tx, rx) = mpsc::channel();
    let dummy =
      BufReader::new(Box::new(io::empty()) as Box<dyn Read + Send>);
    let mut stdout = std::mem::replace(&mut self.reader, dummy);

    let stdout_handle = std::thread::spawn(move || {
      let mut buf = [0u8; 4096];
      loop {
        match stdout.read(&mut buf) {
          Ok(0) => break,
          Ok(n) => {
            let _ = tx.send(Ok(buf[..n].to_vec()));
          }
          Err(e) => {
            let _ = tx.send(Err(e));
            break;
          }
        }
      }
    });

    let stderr_handle = {
      let stderr = self.child.stderr.take();
      std::thread::spawn(move || {
        if let Some(mut stderr) = stderr {
          let mut buf = [0u8; 4096];
          loop {
            match stderr.read(&mut buf) {
              Ok(0) | Err(_) => break,
              Ok(_) => {}
            }
          }
        }
      })
    };

    let start = Instant::now();
    let mut timed_out = false;

    loop {
      let remaining = timeout.saturating_sub(start.elapsed());
      if remaining.is_zero() {
        timed_out = true;
        break;
      }

      match rx.recv_timeout(remaining) {
        Ok(Ok(data)) => {
          io::stdout().write_all(&data)?;
          io::stdout().flush()?;
        }
        Ok(Err(e)) => return Err(e),
        Err(mpsc::RecvTimeoutError::Timeout) => {
          timed_out = true;
          break;
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => break,
      }
    }

    let _ = stdout_handle.join();
    let _ = stderr_handle.join();

    if timed_out {
      let _ = self.child.kill();
      let _ = self.child.wait();
      return Err(io::Error::new(
        io::ErrorKind::TimedOut,
        format!(
          "Remote command timed out after {} seconds",
          timeout.as_secs()
        ),
      ));
    }

    let status = self.child.wait()?;
    Ok(status.code().unwrap_or(1))
  }
}

pub fn ensure_shim(
  host: &str,
  home: &str,
) -> Result<String, Box<dyn std::error::Error>> {
  if !shim_embed::has_embedded_shim() {
    return Err(
      "no embedded shim binaries; rebuild with cross-compiled shim binaries"
        .into(),
    );
  }

  let shim_dir = format!("{home}/{SHIM_DIR}");
  let shim_path = format!("{shim_dir}/{SHIM_NAME}");
  let expected_version = rcargo_protocol::VERSION;

  // Check if shim exists and has the right version.
  let status = std::process::Command::new("ssh")
    .args([
      "-o",
      "BatchMode=yes",
      host,
      &format!("{shim_path} {expected_version}"),
    ])
    .status();
  if matches!(status, Ok(s) if s.success()) {
    return Ok(shim_path);
  }

  let os_arch = ssh::ssh_capture(host, "uname -s && uname -m")?;
  let parts: Vec<&str> = os_arch.split('\n').collect();
  let os = parts.first().map(|s| s.trim()).unwrap_or("");
  let arch = parts.get(1).map(|s| s.trim()).unwrap_or("");

  let os_name = match os {
    "Linux" => "Linux",
    "Darwin" => "Darwin",
    _ => os,
  };
  let arch_name = match arch {
    "x86_64" => "x86_64",
    "aarch64" | "arm64" => "aarch64",
    _ => arch,
  };

  let binary = shim_embed::get_shim_binary(os_name, arch_name)
    .ok_or_else(|| {
      format!("no embedded shim for {os_name}/{arch_name}")
    })?;

  ssh::ssh_capture(host, &format!("mkdir -p {shim_dir}"))?;

  // Write binary to a local temp file, then scp it
  // to the remote host. This avoids ARG_MAX issues
  // with piping base64 through echo.
  let tmp_path = std::env::temp_dir()
    .join(format!("rcargo-shim-{}.bin", std::process::id()));
  std::fs::write(&tmp_path, &binary)?;

  let scp_status = Command::new("scp")
    .args([
      "-q",
      "-o",
      "BatchMode=yes",
      tmp_path.to_str().unwrap(),
      &format!("{host}:{shim_path}"),
    ])
    .status();

  // Always clean up the temp file.
  std::fs::remove_file(&tmp_path).ok();

  let scp_status =
    scp_status.map_err(|e| format!("failed to run scp: {e}"))?;
  if !scp_status.success() {
    return Err(
      format!(
        "scp failed with exit code: {}",
        scp_status.code().unwrap_or(-1)
      )
      .into(),
    );
  }

  ssh::ssh_capture(host, &format!("chmod +x {shim_path}"))?;

  // Verify the deployed shim runs correctly.
  let status = std::process::Command::new("ssh")
    .args([
      "-o",
      "BatchMode=yes",
      host,
      &format!("{shim_path} {expected_version}"),
    ])
    .status();
  if !matches!(status, Ok(s) if s.success()) {
    return Err("shim bootstrap verification failed".into());
  }

  Ok(shim_path)
}

pub fn sync(
  config: &Config,
  remote_path: &str,
  home: &str,
) -> Result<(), Box<dyn std::error::Error>> {
  let shim_path = ensure_shim(&config.target, home)?;
  shim_sync(&config.target, &shim_path, remote_path)
}

fn shim_sync(
  host: &str,
  shim_path: &str,
  remote_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
  ssh::ssh_capture(
    host,
    &format!("mkdir -p {}", shell_quote(remote_path)),
  )?;

  let mut session = ShimSession::open(host, shim_path)?;

  let handshake = session.receive()?;
  match &handshake {
    Message::Handshake { version, os, arch } => {
      eprintln!("[shim] handshake: v{version} {os}/{arch}");
    }
    _ => {
      return Err(
        format!("expected HANDSHAKE, got {:?}", handshake).into(),
      );
    }
  }

  // Sandbox is disabled here because this sync path is used for
  // read-only commands (check, clippy) that don't execute target
  // code. The shim only applies sandboxing for build/test/run.
  session.send(&Message::Sandbox {
    enabled: false,
    workdir: remote_path.to_string(),
    write: vec![],
    read: vec![],
    net: vec![],
  })?;
  let ok = session.receive()?;
  if !matches!(ok, Message::Ok) {
    return Err(format!("sandbox config rejected: {ok:?}").into());
  }

  sync_files_via_session(&mut session)?;

  Ok(())
}

pub fn run_only(
  config: &Config,
  remote_path: &str,
  home: &str,
  cmd: &str,
  debug: bool,
) -> Result<i32, Box<dyn std::error::Error>> {
  let shim_path = ensure_shim(&config.target, home)?;

  ssh::ssh_capture(
    &config.target,
    &format!("mkdir -p {}", shell_quote(remote_path)),
  )?;

  let mut session = ShimSession::open(&config.target, &shim_path)?;

  let handshake = session.receive()?;
  if debug {
    eprintln!("[shim] handshake: {handshake:?}");
  }

  let sandbox = build_sandbox_message(config, remote_path, home);
  session.send(&sandbox)?;
  let ok = session.receive()?;
  if !matches!(ok, Message::Ok) {
    return Err(format!("sandbox config rejected: {ok:?}").into());
  }

  session.send(&Message::Run {
    command: cmd.to_string(),
  })?;

  let code = session.stream_output()?;
  Ok(code)
}

pub fn run_only_with_timeout(
  config: &Config,
  remote_path: &str,
  home: &str,
  cmd: &str,
  debug: bool,
  timeout: std::time::Duration,
) -> Result<i32, Box<dyn std::error::Error>> {
  let shim_path = ensure_shim(&config.target, home)?;

  ssh::ssh_capture(
    &config.target,
    &format!("mkdir -p {}", shell_quote(remote_path)),
  )?;

  let mut session = ShimSession::open(&config.target, &shim_path)?;

  let handshake = session.receive()?;
  if debug {
    eprintln!("[shim] handshake: {handshake:?}");
  }

  let sandbox = build_sandbox_message(config, remote_path, home);
  session.send(&sandbox)?;
  let ok = session.receive()?;
  if !matches!(ok, Message::Ok) {
    return Err(format!("sandbox config rejected: {ok:?}").into());
  }

  session.send(&Message::Run {
    command: cmd.to_string(),
  })?;

  let code = session.stream_output_with_timeout(timeout)?;
  Ok(code)
}

fn sync_files_via_session(
  session: &mut ShimSession,
) -> Result<(), Box<dyn std::error::Error>> {
  session.send(&Message::List)?;

  let mut remote_map: HashMap<String, (u64, u64)> = HashMap::new();
  loop {
    match session.receive()? {
      Message::File { path, size, mtime } => {
        remote_map.insert(path, (size, mtime));
      }
      Message::EndList => break,
      other => {
        return Err(
          format!("expected FILE/END_LIST, got {other:?}").into(),
        );
      }
    }
  }

  let local_files = build_local_file_list()?;
  let local_map: HashMap<String, (u64, u64)> = local_files
    .iter()
    .map(|f| (f.path.clone(), (f.size, f.mtime)))
    .collect();

  let mut delta_requests: Vec<String> = Vec::new();

  for (path, &(size, mtime)) in &local_map {
    match remote_map.get(path.as_str()) {
      Some(&(r_size, r_mtime))
        if r_size == size && r_mtime == mtime =>
      {
        session.send(&Message::Skip { path: path.clone() })?;
        session.receive()?;
      }
      Some(_) => {
        delta_requests.push(path.clone());
      }
      None => {
        let data = std::fs::read(path)?;
        let b64 =
          base64::engine::general_purpose::STANDARD.encode(&data);
        session.send(&Message::Data {
          path: path.clone(),
          data: b64,
        })?;
        session.receive()?;
      }
    }
  }

  let gitignore = build_gitignore_matcher()?;
  for path in remote_map.keys() {
    if !local_map.contains_key(path)
      && !should_exclude(path)
      && !is_gitignored(&gitignore, path)
    {
      session.send(&Message::Delete { path: path.clone() })?;
      session.receive()?;
    }
  }

  if !delta_requests.is_empty() {
    for path in &delta_requests {
      session.send(&Message::DeltaRequest { path: path.clone() })?;
    }
    session.send(&Message::EndSigsRequest)?;

    let mut signatures: HashMap<String, String> = HashMap::new();
    loop {
      match session.receive()? {
        Message::Sig { path, sig } => {
          signatures.insert(path, sig);
        }
        Message::EndSigs => break,
        other => {
          return Err(
            format!("unexpected during SIGS: {other:?}").into(),
          );
        }
      }
    }

    for path in &delta_requests {
      if let Some(sig_b64) = signatures.get(path) {
        let local_data = std::fs::read(path)?;
        let sig_bytes = base64::engine::general_purpose::STANDARD
          .decode(sig_b64)?;
        let sig = fast_rsync::Signature::deserialize(sig_bytes)
          .map_err(|e| format!("bad signature: {e}"))?;
        let mut delta = Vec::new();
        fast_rsync::diff(&sig.index(), &local_data, &mut delta)
          .map_err(|e| format!("diff error: {e}"))?;
        let delta_b64 =
          base64::engine::general_purpose::STANDARD.encode(&delta);
        let mut hasher = Sha256::new();
        hasher.update(&local_data);
        let sha256 = format!("{:x}", hasher.finalize());
        session.send(&Message::Delta {
          path: path.clone(),
          delta: delta_b64,
          sha256,
        })?;
        match session.receive()? {
          Message::Ok => {}
          Message::Error(e) => {
            eprintln!(
              "[rcargo] delta failed for {path}: {e}, \
               uploading full file"
            );
            let data = std::fs::read(path)?;
            let b64 =
              base64::engine::general_purpose::STANDARD.encode(&data);
            session.send(&Message::Data {
              path: path.clone(),
              data: b64,
            })?;
            session.receive()?;
          }
          other => {
            return Err(
              format!("unexpected response to DELTA: {other:?}")
                .into(),
            );
          }
        }
      }
    }
  }

  session.send(&Message::EndSync)?;
  session.receive()?;

  Ok(())
}

struct LocalFile {
  path: String,
  size: u64,
  mtime: u64,
}

fn build_local_file_list(
) -> Result<Vec<LocalFile>, Box<dyn std::error::Error>> {
  let walker =
    WalkBuilder::new(".").hidden(false).git_ignore(true).build();

  let mut files = Vec::new();
  for entry in walker {
    let entry = entry?;
    let metadata = entry.metadata()?;
    if metadata.is_dir() || metadata.is_symlink() {
      continue;
    }
    let path = entry.path();
    let path_str = path
      .strip_prefix("./")
      .unwrap_or(path)
      .to_string_lossy()
      .to_string();
    if should_exclude(&path_str) {
      continue;
    }
    let mtime = metadata
      .modified()
      .ok()
      .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
      .map(|d| d.as_secs())
      .unwrap_or(0);
    files.push(LocalFile {
      path: path_str,
      size: metadata.len(),
      mtime,
    });
  }
  Ok(files)
}

fn should_exclude(path: &str) -> bool {
  for part in path.split('/') {
    if part == ".git" || part == "target" {
      return true;
    }
  }
  false
}

fn build_gitignore_matcher(
) -> Result<ignore::gitignore::Gitignore, Box<dyn std::error::Error>>
{
  let mut builder = ignore::gitignore::GitignoreBuilder::new(".");
  if let Some(err) = builder.add(".gitignore") {
    return Err(format!("failed to parse .gitignore: {err}").into());
  }
  Ok(builder.build()?)
}

fn is_gitignored(
  gitignore: &ignore::gitignore::Gitignore,
  path: &str,
) -> bool {
  use ignore::Match;
  matches!(gitignore.matched(path, false), Match::Ignore(_))
}

fn build_sandbox_message(
  config: &Config,
  remote_path: &str,
  home: &str,
) -> Message {
  if !config.sandbox.enabled {
    return Message::Sandbox {
      enabled: false,
      workdir: remote_path.to_string(),
      write: vec![],
      read: vec![],
      net: vec![],
    };
  }

  let mut write = vec![
    format!("{home}/.rustup"),
    format!("{home}/.cargo"),
    remote_path.to_string(),
    "/tmp".to_string(),
  ];
  for w in &config.sandbox.allow.write {
    write.push(w.clone());
  }

  let mut read =
    vec!["/usr/libexec".to_string(), "/usr/include".to_string()];
  for r in &config.sandbox.allow.read {
    read.push(r.clone());
  }

  let default_domains = [
    "crates.io",
    "index.crates.io",
    "static.crates.io",
    "static.rust-lang.org",
    "github.com",
  ];
  let mut net: Vec<String> =
    default_domains.iter().map(|s| s.to_string()).collect();
  for d in &config.sandbox.allow.net {
    net.push(d.clone());
  }

  Message::Sandbox {
    enabled: true,
    workdir: remote_path.to_string(),
    write,
    read,
    net,
  }
}
