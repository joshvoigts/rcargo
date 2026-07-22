mod sandbox;
mod sync;

use base64::Engine;
use rcargo_protocol::{Message, ProtocolReader, ProtocolWriter};
use sandbox::SandboxConfig;
use std::io::{self};
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
  let args: Vec<String> = std::env::args().collect();

  match args.get(1).map(|s| s.as_str()) {
    Some("--version") => {
      println!("rcargo-shim {}", rcargo_protocol::VERSION);
      return;
    }
    Some("--shim") => {
      run_shim_mode();
      return;
    }
    _ => {}
  }

  eprintln!("Usage: rcargo-shim [--version|--shim]");
  std::process::exit(1);
}

fn run_shim_mode() {
  if let Err(e) = shim_loop() {
    eprintln!("[shim] fatal: {e}");
    std::process::exit(1);
  }
}

fn shim_loop() -> Result<(), Box<dyn std::error::Error>> {
  let stdin = io::stdin();
  let stdout = io::stdout();
  let mut reader = ProtocolReader::new(stdin.lock());
  let mut writer = ProtocolWriter::new(stdout.lock());

  writer.send(&Message::Handshake {
    version: rcargo_protocol::VERSION.to_string(),
    os: std::env::consts::OS.to_string(),
    arch: std::env::consts::ARCH.to_string(),
  })?;

  let mut sandbox_config = SandboxConfig::default();
  let mut workdir: Option<PathBuf> = None;

  loop {
    let msg = reader.receive()?;
    match msg {
      Message::List => {
        let wd = workdir.as_ref().ok_or("no workdir set")?;
        let files = sync::list_local_files(wd)?;
        for f in &files {
          writer.send(&Message::File {
            path: f.path.clone(),
            size: f.size,
            mtime: f.mtime,
          })?;
        }
        writer.send(&Message::EndList)?;
      }
      Message::Sandbox {
        enabled,
        workdir: wd,
        write,
        read,
        net,
      } => {
        sandbox_config = SandboxConfig {
          enabled,
          workdir: wd.clone(),
          write,
          read,
          net,
        };
        workdir = Some(PathBuf::from(&wd));
        writer.send(&Message::Ok)?;
      }
      Message::Skip { .. } => {
        writer.send(&Message::Ok)?;
      }
      Message::Data { path, data } => {
        let wd = workdir.as_ref().ok_or("no workdir set")?;
        let full = validate_path(wd, &path)?;
        sync::apply_upload(&full, &data)?;
        writer.send(&Message::Ok)?;
      }
      Message::Delete { path } => {
        let wd = workdir.as_ref().ok_or("no workdir set")?;
        let full = validate_path(wd, &path)?;
        sync::apply_delete(&full)?;
        writer.send(&Message::Ok)?;
      }
      Message::DeltaRequest { path } => {
        let wd = workdir.as_ref().ok_or("no workdir set")?;
        let full = validate_path(wd, &path)?;
        let data = sync::read_file(&full)?;
        let sig = sync::compute_signature(&data);
        let sig_b64 =
          base64::engine::general_purpose::STANDARD.encode(&sig);
        writer.send(&Message::Sig { path, sig: sig_b64 })?;
      }
      Message::EndSigsRequest => {
        writer.send(&Message::EndSigs)?;
      }
      Message::Delta {
        path,
        delta,
        sha256,
      } => {
        let wd = workdir.as_ref().ok_or("no workdir set")?;
        let full = validate_path(wd, &path)?;
        match sync::read_file(&full) {
          Ok(old_data) => {
            match sync::apply_delta(&old_data, &delta, &sha256) {
              Ok(new_data) => {
                sync::write_file(&full, &new_data)?;
                writer.send(&Message::Ok)?;
              }
              Err(e) => {
                writer.send(&Message::Error(format!(
                  "delta failed: {e}, need full upload"
                )))?;
              }
            }
          }
          Err(_) => {
            writer.send(&Message::Error(format!(
              "cannot read {path} for delta, need full upload"
            )))?;
          }
        }
      }
      Message::EndSync => {
        writer.send(&Message::Ok)?;
      }
      Message::Run { command } => {
        if sandbox_config.enabled {
          sandbox::apply_sandbox(&sandbox_config)?;
        }

        drop(reader);
        drop(writer);

        let status = Command::new("bash")
          .args(["--norc", "--noprofile", "-c", &command])
          .status();

        let code = match status {
          Ok(s) => s.code().unwrap_or(1),
          Err(e) => {
            eprintln!("[shim] exec error: {e}");
            1
          }
        };
        std::process::exit(code);
      }
      _ => {
        writer.send(&Message::Error(format!(
          "unexpected message: {:?}",
          msg
        )))?;
      }
    }
  }
}

fn validate_path(
  workdir: &Path,
  path: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
  let full = workdir.join(path);
  let canonical_workdir = workdir
    .canonicalize()
    .map_err(|e| format!("cannot resolve workdir: {e}"))?;
  if full.exists() {
    let canonical = full.canonicalize().map_err(|e| {
      format!("cannot resolve {}: {e}", full.display())
    })?;
    if !canonical.starts_with(&canonical_workdir) {
      return Err(format!("path {path} escapes workdir").into());
    }
  } else if let Some(parent) = full.parent() {
    if parent.exists() {
      let canonical_parent = parent.canonicalize().map_err(|e| {
        format!("cannot resolve {}: {e}", parent.display())
      })?;
      if !canonical_parent.starts_with(&canonical_workdir) {
        return Err(format!("path {path} escapes workdir").into());
      }
    }
  }
  Ok(full)
}
