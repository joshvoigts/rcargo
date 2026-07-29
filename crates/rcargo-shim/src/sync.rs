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

#[cfg(test)]
mod tests {
  use super::*;
  use tempfile::TempDir;

  #[test]
  fn list_local_files_finds_files() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("a.rs"), "fn main() {}").unwrap();
    fs::create_dir(tmp.path().join("src")).unwrap();
    fs::write(tmp.path().join("src/lib.rs"), "").unwrap();

    let files = list_local_files(tmp.path()).unwrap();
    let paths: Vec<&str> =
      files.iter().map(|f| f.path.as_str()).collect();
    assert!(paths.contains(&"a.rs"));
    assert!(paths.contains(&"src/lib.rs"));
  }

  #[test]
  fn list_local_files_excludes_git() {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join(".git/objects")).unwrap();
    fs::write(tmp.path().join(".git/HEAD"), "").unwrap();
    fs::write(tmp.path().join("a.rs"), "").unwrap();

    let files = list_local_files(tmp.path()).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "a.rs");
  }

  #[test]
  fn list_local_files_excludes_target() {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("target/release")).unwrap();
    fs::write(tmp.path().join("target/release/binary"), "").unwrap();
    fs::write(tmp.path().join("a.rs"), "").unwrap();

    let files = list_local_files(tmp.path()).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "a.rs");
  }

  #[test]
  fn apply_upload_creates_file() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("hello.txt");
    let data = b"hello world";
    let b64 = base64::engine::general_purpose::STANDARD.encode(data);

    apply_upload(&file_path, &b64).unwrap();
    let contents = fs::read(&file_path).unwrap();
    assert_eq!(contents, data);
  }

  #[test]
  fn apply_upload_creates_parent_dirs() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("a/b/c/file.txt");
    let data = b"nested";
    let b64 = base64::engine::general_purpose::STANDARD.encode(data);

    apply_upload(&file_path, &b64).unwrap();
    let contents = fs::read(&file_path).unwrap();
    assert_eq!(contents, data);
  }

  #[test]
  fn apply_delete_removes_file() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("to_delete.txt");
    fs::write(&file_path, "bye").unwrap();
    assert!(file_path.exists());

    apply_delete(&file_path).unwrap();
    assert!(!file_path.exists());
  }

  #[test]
  fn apply_delete_removes_dir() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("to_delete_dir");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("file.txt"), "").unwrap();

    apply_delete(&dir).unwrap();
    assert!(!dir.exists());
  }

  #[test]
  fn apply_delete_noop_on_missing() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("nope.txt");
    apply_delete(&file_path).unwrap();
  }

  #[test]
  fn signature_and_delta_roundtrip() {
    let old_data = b"hello world, this is the old content";
    let new_data = b"hello world, this is the new content!!!";

    let sig = compute_signature(old_data);
    assert!(!sig.is_empty());

    let mut delta = Vec::new();
    let sig_obj =
      fast_rsync::Signature::deserialize(sig.clone()).unwrap();
    fast_rsync::diff(&sig_obj.index(), new_data, &mut delta).unwrap();

    let result = apply_delta(
      old_data,
      &base64::engine::general_purpose::STANDARD.encode(&delta),
      &sha256_hex(new_data),
    )
    .unwrap();
    assert_eq!(result, new_data);
  }

  #[test]
  fn delta_rejects_wrong_sha256() {
    let old_data = b"old";
    let new_data = b"new";

    let sig = compute_signature(old_data);
    let mut delta = Vec::new();
    let sig_obj = fast_rsync::Signature::deserialize(sig).unwrap();
    fast_rsync::diff(&sig_obj.index(), new_data, &mut delta).unwrap();

    let wrong_hash = "0".repeat(64);
    let result = apply_delta(
      old_data,
      &base64::engine::general_purpose::STANDARD.encode(&delta),
      &wrong_hash,
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("SHA-256 mismatch"));
  }

  #[test]
  fn write_file_creates_dirs() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("a/b/c/file.bin");
    write_file(&path, b"data").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"data");
  }

  fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
  }
}
