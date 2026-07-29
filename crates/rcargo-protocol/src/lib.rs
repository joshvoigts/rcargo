use std::io::{self, BufReader, BufWriter, Read, Write};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Default)]
pub struct SandboxConfig {
  pub enabled: bool,
  pub workdir: String,
  pub write: Vec<String>,
  pub read: Vec<String>,
  pub net: Vec<String>,
}

impl SandboxConfig {
  pub fn encode(&self) -> String {
    let mut parts = vec![
      if self.enabled { "1" } else { "0" }.to_string(),
      self.workdir.clone(),
    ];
    for p in &self.write {
      parts.push(format!("write:{p}"));
    }
    for p in &self.read {
      parts.push(format!("read:{p}"));
    }
    if !self.net.is_empty() {
      parts.push("--".to_string());
      for d in &self.net {
        parts.push(d.clone());
      }
    }
    parts.join(" ")
  }

  pub fn decode(s: &str) -> Result<Self, String> {
    let parts: Vec<&str> = s.split(" -- ").collect();
    let before_net = parts[0];
    let net: Vec<String> = if parts.len() > 1 {
      parts[1].split_whitespace().map(String::from).collect()
    } else {
      Vec::new()
    };
    let mut tokens = before_net.split_whitespace();
    let enabled =
      tokens.next().ok_or("missing SANDBOX enabled")? == "1";
    let workdir =
      tokens.next().ok_or("missing SANDBOX workdir")?.to_string();
    let mut write = Vec::new();
    let mut read = Vec::new();
    for tok in tokens {
      if let Some(p) = tok.strip_prefix("write:") {
        write.push(p.to_string());
      } else if let Some(p) = tok.strip_prefix("read:") {
        read.push(p.to_string());
      }
    }
    Ok(SandboxConfig {
      enabled,
      workdir,
      write,
      read,
      net,
    })
  }
}

#[derive(Debug, Clone)]
pub enum Message {
  Handshake {
    version: String,
    os: String,
    arch: String,
  },
  List,
  File {
    path: String,
    size: u64,
    mtime: u64,
  },
  EndList,
  Skip {
    path: String,
  },
  Delete {
    path: String,
  },
  DeltaRequest {
    path: String,
  },
  EndSigsRequest,
  Sig {
    path: String,
    sig: String,
  },
  EndSigs,
  Delta {
    path: String,
    delta: String,
    sha256: String,
  },
  Data {
    path: String,
    data: String,
  },
  EndSync,
  Sandbox {
    enabled: bool,
    workdir: String,
    write: Vec<String>,
    read: Vec<String>,
    net: Vec<String>,
  },
  Run {
    command: String,
  },
  Error(String),
  Ok,
}

impl Message {
  pub fn encode(&self) -> String {
    match self {
      Message::Handshake { version, os, arch } => {
        format!("HANDSHAKE {version} {os} {arch}")
      }
      Message::List => "LIST".to_string(),
      Message::File { path, size, mtime } => {
        format!("FILE {path} {size} {mtime}")
      }
      Message::EndList => "END_LIST".to_string(),
      Message::Skip { path } => format!("SKIP {path}"),
      Message::Delete { path } => format!("DELETE {path}"),
      Message::DeltaRequest { path } => {
        format!("DELTA_REQUEST {path}")
      }
      Message::EndSigsRequest => "END_SIGS_REQUEST".to_string(),
      Message::Sig { path, sig } => {
        format!("SIG {path} {sig}")
      }
      Message::EndSigs => "END_SIGS".to_string(),
      Message::Delta {
        path,
        delta,
        sha256,
      } => {
        format!("DELTA {path} {delta} {sha256}")
      }
      Message::Data { path, data } => {
        format!("DATA {path} {data}")
      }
      Message::EndSync => "END_SYNC".to_string(),
      Message::Sandbox {
        enabled,
        workdir,
        write,
        read,
        net,
      } => {
        let config = SandboxConfig {
          enabled: *enabled,
          workdir: workdir.clone(),
          write: write.clone(),
          read: read.clone(),
          net: net.clone(),
        };
        format!("SANDBOX {}", config.encode())
      }
      Message::Run { command } => {
        format!("RUN {command}")
      }
      Message::Error(msg) => {
        format!("ERROR {msg}")
      }
      Message::Ok => "OK".to_string(),
    }
  }

  pub fn decode(s: &str) -> Result<Self, String> {
    let (type_name, rest) = match s.split_once(' ') {
      Some((t, r)) => (t, r),
      None => (s, ""),
    };
    match type_name {
      "HANDSHAKE" => {
        let parts: Vec<&str> = rest.splitn(3, ' ').collect();
        if parts.len() != 3 {
          return Err("invalid HANDSHAKE".into());
        }
        Ok(Message::Handshake {
          version: parts[0].to_string(),
          os: parts[1].to_string(),
          arch: parts[2].to_string(),
        })
      }
      "LIST" => Ok(Message::List),
      "FILE" => {
        let last = rest.rfind(' ').ok_or("invalid FILE")?;
        let mtime_str = &rest[last + 1..];
        let before = &rest[..last];
        let last2 = before.rfind(' ').ok_or("invalid FILE")?;
        let size_str = &before[last2 + 1..];
        let path = &before[..last2];
        Ok(Message::File {
          path: path.to_string(),
          size: size_str
            .parse()
            .map_err(|e| format!("bad size: {e}"))?,
          mtime: mtime_str
            .parse()
            .map_err(|e| format!("bad mtime: {e}"))?,
        })
      }
      "END_LIST" => Ok(Message::EndList),
      "SKIP" => Ok(Message::Skip {
        path: rest.to_string(),
      }),
      "DELETE" => Ok(Message::Delete {
        path: rest.to_string(),
      }),
      "DELTA_REQUEST" => Ok(Message::DeltaRequest {
        path: rest.to_string(),
      }),
      "END_SIGS_REQUEST" => Ok(Message::EndSigsRequest),
      "SIG" => {
        let last = rest.rfind(' ').ok_or("invalid SIG")?;
        let sig = &rest[last + 1..];
        let path = &rest[..last];
        Ok(Message::Sig {
          path: path.to_string(),
          sig: sig.to_string(),
        })
      }
      "END_SIGS" => Ok(Message::EndSigs),
      "DELTA" => {
        let last = rest.rfind(' ').ok_or("invalid DELTA")?;
        let sha256 = &rest[last + 1..];
        let before = &rest[..last];
        let last2 = before.rfind(' ').ok_or("invalid DELTA")?;
        let delta = &before[last2 + 1..];
        let path = &before[..last2];
        Ok(Message::Delta {
          path: path.to_string(),
          delta: delta.to_string(),
          sha256: sha256.to_string(),
        })
      }
      "DATA" => {
        let last = rest.rfind(' ').ok_or("invalid DATA")?;
        let data = &rest[last + 1..];
        let path = &rest[..last];
        Ok(Message::Data {
          path: path.to_string(),
          data: data.to_string(),
        })
      }
      "END_SYNC" => Ok(Message::EndSync),
      "SANDBOX" => {
        let config = SandboxConfig::decode(rest)?;
        Ok(Message::Sandbox {
          enabled: config.enabled,
          workdir: config.workdir,
          write: config.write,
          read: config.read,
          net: config.net,
        })
      }
      "RUN" => Ok(Message::Run {
        command: rest.to_string(),
      }),
      "ERROR" => Ok(Message::Error(rest.to_string())),
      "OK" => Ok(Message::Ok),
      _ => Err(format!("unknown message type: {type_name}")),
    }
  }
}

pub struct ProtocolReader<R: Read> {
  reader: BufReader<R>,
}

impl<R: Read> ProtocolReader<R> {
  pub fn new(reader: R) -> Self {
    Self {
      reader: BufReader::new(reader),
    }
  }

  pub fn receive(&mut self) -> io::Result<Message> {
    let mut len_buf = [0u8; 4];
    self.reader.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut payload = vec![0u8; len];
    self.reader.read_exact(&mut payload)?;
    let payload = String::from_utf8(payload)
      .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Message::decode(&payload)
      .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
  }
}

pub struct ProtocolWriter<W: Write> {
  writer: BufWriter<W>,
}

impl<W: Write> ProtocolWriter<W> {
  pub fn new(writer: W) -> Self {
    Self {
      writer: BufWriter::new(writer),
    }
  }

  pub fn send(&mut self, msg: &Message) -> io::Result<()> {
    let payload = msg.encode();
    let len = payload.len() as u32;
    self.writer.write_all(&len.to_be_bytes())?;
    self.writer.write_all(payload.as_bytes())?;
    self.writer.flush()?;
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  fn roundtrip(msg: &Message) {
    let encoded = msg.encode();
    let decoded = Message::decode(&encoded).expect("decode failed");
    assert_eq!(msg.encode(), decoded.encode());
  }

  fn wire_roundtrip(msg: &Message) {
    let encoded = msg.encode();
    let len = encoded.len() as u32;
    let mut buf = Vec::new();
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(encoded.as_bytes());

    let mut reader = ProtocolReader::new(Cursor::new(buf));
    let decoded = reader.receive().expect("wire read failed");
    assert_eq!(msg.encode(), decoded.encode());
  }

  #[test]
  fn handshake_roundtrip() {
    roundtrip(&Message::Handshake {
      version: "0.1.0".into(),
      os: "linux".into(),
      arch: "x86_64".into(),
    });
  }

  #[test]
  fn list_roundtrip() {
    roundtrip(&Message::List);
    roundtrip(&Message::EndList);
  }

  #[test]
  fn file_roundtrip() {
    roundtrip(&Message::File {
      path: "src/main.rs".into(),
      size: 1234,
      mtime: 1700000000,
    });
  }

  #[test]
  fn file_with_spaces_roundtrip() {
    roundtrip(&Message::File {
      path: "src/my file.rs".into(),
      size: 1234,
      mtime: 1700000000,
    });
  }

  #[test]
  fn skip_delete_roundtrip() {
    roundtrip(&Message::Skip {
      path: "foo.rs".into(),
    });
    roundtrip(&Message::Delete {
      path: "old.rs".into(),
    });
  }

  #[test]
  fn delta_request_roundtrip() {
    roundtrip(&Message::DeltaRequest {
      path: "big.bin".into(),
    });
    roundtrip(&Message::EndSigsRequest);
    roundtrip(&Message::EndSigs);
  }

  #[test]
  fn sig_roundtrip() {
    roundtrip(&Message::Sig {
      path: "a.rs".into(),
      sig: "base64data".into(),
    });
  }

  #[test]
  fn sig_with_spaces_roundtrip() {
    roundtrip(&Message::Sig {
      path: "my file.rs".into(),
      sig: "base64data".into(),
    });
  }

  #[test]
  fn delta_roundtrip() {
    roundtrip(&Message::Delta {
      path: "a.rs".into(),
      delta: "base64delta".into(),
      sha256: "a".repeat(64),
    });
  }

  #[test]
  fn delta_with_spaces_roundtrip() {
    roundtrip(&Message::Delta {
      path: "src/my file.rs".into(),
      delta: "base64delta".into(),
      sha256: "a".repeat(64),
    });
  }

  #[test]
  fn data_roundtrip() {
    roundtrip(&Message::Data {
      path: "a.rs".into(),
      data: "base64content".into(),
    });
  }

  #[test]
  fn data_with_spaces_roundtrip() {
    roundtrip(&Message::Data {
      path: "src/my file.rs".into(),
      data: "base64content".into(),
    });
  }

  #[test]
  fn end_sync_roundtrip() {
    roundtrip(&Message::EndSync);
  }

  #[test]
  fn sandbox_roundtrip() {
    roundtrip(&Message::Sandbox {
      enabled: true,
      workdir: "/home/user/proj".into(),
      write: vec![
        "/home/user/.cargo".into(),
        "/home/user/proj".into(),
      ],
      read: vec!["/usr/include".into()],
      net: vec!["crates.io".into(), "github.com".into()],
    });
  }

  #[test]
  fn sandbox_config_roundtrip() {
    let config = SandboxConfig {
      enabled: true,
      workdir: "/home/user/proj".into(),
      write: vec![
        "/home/user/.cargo".into(),
        "/home/user/proj".into(),
      ],
      read: vec!["/usr/include".into()],
      net: vec!["crates.io".into(), "github.com".into()],
    };
    let encoded = config.encode();
    let decoded =
      SandboxConfig::decode(&encoded).expect("decode failed");
    assert_eq!(config.encode(), decoded.encode());
  }

  #[test]
  fn sandbox_config_no_net_roundtrip() {
    let config = SandboxConfig {
      enabled: false,
      workdir: "/tmp".into(),
      write: vec![],
      read: vec![],
      net: vec![],
    };
    let encoded = config.encode();
    let decoded =
      SandboxConfig::decode(&encoded).expect("decode failed");
    assert_eq!(config.encode(), decoded.encode());
  }

  #[test]
  fn sandbox_no_net_roundtrip() {
    roundtrip(&Message::Sandbox {
      enabled: false,
      workdir: "/tmp".into(),
      write: vec![],
      read: vec![],
      net: vec![],
    });
  }

  #[test]
  fn run_roundtrip() {
    roundtrip(&Message::Run {
      command: "cd /tmp && cargo build".into(),
    });
  }

  #[test]
  fn error_roundtrip() {
    roundtrip(&Message::Error("something went wrong".into()));
  }

  #[test]
  fn ok_roundtrip() {
    roundtrip(&Message::Ok);
  }

  #[test]
  fn wire_handshake() {
    wire_roundtrip(&Message::Handshake {
      version: "0.1.0".into(),
      os: "macos".into(),
      arch: "aarch64".into(),
    });
  }

  #[test]
  fn wire_data() {
    wire_roundtrip(&Message::Data {
      path: "file.rs".into(),
      data: "aG9sb2E=".into(),
    });
  }

  #[test]
  fn decode_error_on_bad_type() {
    assert!(Message::decode("BOGUS foo").is_err());
  }

  #[test]
  fn decode_error_on_bad_handshake() {
    assert!(Message::decode("HANDSHAKE only_one_arg").is_err());
  }

  #[test]
  fn decode_error_on_bad_file_size() {
    assert!(Message::decode("FILE a not_a_number 123").is_err());
  }
}
