mod sandbox;
mod sync;

use base64::Engine;
use rcargo_protocol::{
  Message, ProtocolReader, ProtocolWriter, SandboxConfig,
};
use std::io::{self};
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
  let args: Vec<String> = std::env::args().collect();

  if let Some(expected) = args.get(1) {
    if expected == rcargo_protocol::VERSION {
      std::process::exit(0);
    } else {
      std::process::exit(1);
    }
  }

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
      Message::Sandbox(config) => {
        workdir = Some(PathBuf::from(&config.workdir));
        sandbox_config = config;
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
  for component in path.split('/') {
    if component == ".." {
      return Err(
        format!("path component '..' is not allowed in {path}")
          .into(),
      );
    }
  }

  let full = workdir.join(path);
  let canonical_workdir = workdir
    .canonicalize()
    .map_err(|e| format!("cannot resolve workdir: {e}"))?;

  if let Ok(metadata) = full.symlink_metadata() {
    if metadata.file_type().is_symlink() {
      return Err(format!("symlink {path} is not allowed").into());
    }
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

#[cfg(test)]
mod tests {
  use super::*;
  use tempfile::TempDir;

  fn setup_workdir() -> TempDir {
    let tmp = TempDir::new().unwrap();
    // Create a sample file so validate_path has something
    // to check against.
    std::fs::write(tmp.path().join("hello.txt"), "hi").unwrap();
    tmp
  }

  #[test]
  fn validate_simple_path() {
    let tmp = setup_workdir();
    let result = validate_path(tmp.path(), "hello.txt").unwrap();
    assert_eq!(result, tmp.path().join("hello.txt"));
  }

  #[test]
  fn validate_nested_path() {
    let tmp = setup_workdir();
    std::fs::create_dir_all(tmp.path().join("a/b")).unwrap();
    let result = validate_path(tmp.path(), "a/b/hello.txt").unwrap();
    assert_eq!(result, tmp.path().join("a/b/hello.txt"));
  }

  #[test]
  fn validate_rejects_dotdot() {
    let tmp = setup_workdir();
    assert!(validate_path(tmp.path(), "../etc/passwd").is_err());
  }

  #[test]
  fn validate_rejects_dotdot_in_middle() {
    let tmp = setup_workdir();
    assert!(
      validate_path(tmp.path(), "foo/../../etc/passwd").is_err()
    );
  }

  #[test]
  fn validate_rejects_dotdot_embedded() {
    let tmp = setup_workdir();
    assert!(validate_path(tmp.path(), "foo/..").is_err());
  }

  #[test]
  fn validate_rejects_symlink() {
    let tmp = setup_workdir();
    std::os::unix::fs::symlink(
      "/etc/passwd",
      tmp.path().join("link"),
    )
    .unwrap();
    assert!(validate_path(tmp.path(), "link").is_err());
  }

  #[test]
  fn validate_rejects_symlink_to_parent() {
    let tmp = setup_workdir();
    std::os::unix::fs::symlink("..", tmp.path().join("sneaky"))
      .unwrap();
    assert!(validate_path(tmp.path(), "sneaky").is_err());
  }

  #[test]
  fn validate_file_not_exists_no_parent() {
    let tmp = setup_workdir();
    // nonexistent file whose parent also doesn't exist
    let result =
      validate_path(tmp.path(), "nonexistent/deep/file.txt");
    assert!(result.is_ok());
  }
}
