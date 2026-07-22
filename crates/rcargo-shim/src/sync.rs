use base64::Engine;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone)]
pub struct RemoteFile {
  pub path: String,
  pub size: u64,
  pub mtime: u64,
}

pub fn list_local_files(
  root: &Path,
) -> Result<Vec<RemoteFile>, String> {
  let mut files = Vec::new();
  list_recursive(root, root, &mut files)?;
  Ok(files)
}

fn list_recursive(
  base: &Path,
  dir: &Path,
  files: &mut Vec<RemoteFile>,
) -> Result<(), String> {
  let entries = fs::read_dir(dir)
    .map_err(|e| format!("read_dir {}: {e}", dir.display()))?;
  for entry in entries {
    let entry = entry.map_err(|e| format!("read_dir entry: {e}"))?;
    let path = entry.path();
    let metadata = path
      .metadata()
      .map_err(|e| format!("stat {}: {e}", path.display()))?;
    if metadata.is_dir() {
      if should_exclude_dir(&path, base) {
        continue;
      }
      list_recursive(base, &path, files)?;
    } else {
      let rel = path
        .strip_prefix(base)
        .map_err(|e| format!("strip_prefix: {e}"))?;
      let rel_str = rel.to_string_lossy().to_string();
      if should_exclude(&rel_str) {
        continue;
      }
      let mtime = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
      files.push(RemoteFile {
        path: rel_str,
        size: metadata.len(),
        mtime,
      });
    }
  }
  Ok(())
}

fn should_exclude_dir(path: &Path, base: &Path) -> bool {
  let rel = path.strip_prefix(base).unwrap_or(path);
  let name = match rel.file_name() {
    Some(n) => n.to_string_lossy(),
    None => return false,
  };
  name == ".git" || name == "target"
}

fn should_exclude(path: &str) -> bool {
  let parts: Vec<&str> = path.split('/').collect();
  for part in &parts {
    if *part == ".git" || *part == "target" {
      return true;
    }
  }
  false
}

pub fn apply_upload(
  full: &Path,
  data_b64: &str,
) -> Result<(), String> {
  let data = base64::engine::general_purpose::STANDARD
    .decode(data_b64)
    .map_err(|e| format!("base64 decode: {e}"))?;
  if let Some(parent) = full.parent() {
    fs::create_dir_all(parent)
      .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
  }
  fs::write(full, data)
    .map_err(|e| format!("write {}: {e}", full.display()))?;
  Ok(())
}

pub fn apply_delete(full: &Path) -> Result<(), String> {
  if full.exists() {
    if full.is_dir() {
      fs::remove_dir_all(full)
        .map_err(|e| format!("rmdir {}: {e}", full.display()))?;
    } else {
      fs::remove_file(full)
        .map_err(|e| format!("rm {}: {e}", full.display()))?;
    }
  }
  Ok(())
}

pub fn compute_signature(data: &[u8]) -> Vec<u8> {
  let sig = fast_rsync::Signature::calculate(
    data,
    fast_rsync::SignatureOptions {
      block_size: 4096,
      crypto_hash_size: 16,
    },
  );
  sig.serialized().to_vec()
}

pub fn apply_delta(
  old_data: &[u8],
  delta_b64: &str,
  expected_sha256: &str,
) -> Result<Vec<u8>, String> {
  let delta = base64::engine::general_purpose::STANDARD
    .decode(delta_b64)
    .map_err(|e| format!("base64 decode delta: {e}"))?;
  let mut new_data = Vec::new();
  fast_rsync::apply(old_data, &delta, &mut new_data)
    .map_err(|e| format!("apply delta: {e}"))?;

  let mut hasher = Sha256::new();
  hasher.update(&new_data);
  let actual = format!("{:x}", hasher.finalize());
  if actual != expected_sha256 {
    return Err(format!(
      "SHA-256 mismatch: expected {expected_sha256}, got {actual}"
    ));
  }
  Ok(new_data)
}

pub fn read_file(path: &Path) -> Result<Vec<u8>, String> {
  fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))
}

pub fn write_file(path: &Path, data: &[u8]) -> Result<(), String> {
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent)
      .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
  }
  fs::write(path, data)
    .map_err(|e| format!("write {}: {e}", path.display()))
}
