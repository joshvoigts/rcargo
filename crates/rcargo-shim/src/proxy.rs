use std::collections::HashSet;
use tokio::io::{
  AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader,
};
use tokio::net::{TcpListener, TcpStream};

pub struct NetworkProxy {
  listener: TcpListener,
  allowed_domains: HashSet<String>,
}

impl NetworkProxy {
  pub async fn start(
    allowed_domains: Vec<String>,
  ) -> Result<Self, Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let allowed_domains: HashSet<String> =
      allowed_domains.into_iter().collect();
    Ok(Self {
      listener,
      allowed_domains,
    })
  }

  pub fn port(&self) -> u16 {
    self.listener.local_addr().unwrap().port()
  }

  pub async fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
    loop {
      let (stream, _addr) = self.listener.accept().await?;
      let allowed = self.allowed_domains.clone();
      tokio::spawn(async move {
        if let Err(e) = handle_connection(stream, &allowed).await {
          eprintln!("[proxy] connection error: {e}");
        }
      });
    }
  }
}

async fn handle_connection(
  mut client: TcpStream,
  allowed_domains: &HashSet<String>,
) -> Result<(), Box<dyn std::error::Error>> {
  let mut reader = BufReader::new(&mut client);
  let mut request_line = String::new();
  reader.read_line(&mut request_line).await?;

  let parts: Vec<&str> =
    request_line.trim().split_whitespace().collect();
  if parts.len() < 3 {
    return Err("invalid request".into());
  }

  let method = parts[0];
  let target = parts[1];

  match method {
    "CONNECT" => {
      // HTTPS tunneling: CONNECT host:port HTTP/1.1
      let host_port: Vec<&str> = target.split(':').collect();
      let host = host_port[0];
      let port: u16 =
        host_port.get(1).and_then(|p| p.parse().ok()).unwrap_or(443);

      if !is_domain_allowed(host, allowed_domains) {
        let response = "HTTP/1.1 403 Forbidden\r\n\r\n";
        client.write_all(response.as_bytes()).await?;
        return Ok(());
      }

      // Connect to the target
      let target_addr = format!("{host}:{port}");
      match TcpStream::connect(&target_addr).await {
        Ok(target_stream) => {
          let response =
            "HTTP/1.1 200 Connection Established\r\n\r\n";
          client.write_all(response.as_bytes()).await?;

          // Bidirectional copy
          let (mut client_read, mut client_write) =
            client.into_split();
          let (mut target_read, mut target_write) =
            target_stream.into_split();

          let client_to_target = async {
            tokio::io::copy(&mut client_read, &mut target_write).await
          };
          let target_to_client = async {
            tokio::io::copy(&mut target_read, &mut client_write).await
          };

          tokio::select! {
            _ = client_to_target => {}
            _ = target_to_client => {}
          }
        }
        Err(_) => {
          let response = "HTTP/1.1 502 Bad Gateway\r\n\r\n";
          client.write_all(response.as_bytes()).await?;
        }
      }
    }
    "GET" | "POST" | "PUT" | "DELETE" | "HEAD" | "OPTIONS" => {
      // HTTP request: parse the URL
      let url = target.parse::<url::Url>()?;
      let host = url.host_str().unwrap_or("");

      if !is_domain_allowed(host, allowed_domains) {
        let response = "HTTP/1.1 403 Forbidden\r\n\r\n";
        client.write_all(response.as_bytes()).await?;
        return Ok(());
      }

      let port = url.port().unwrap_or(80);
      let target_addr = format!("{host}:{port}");

      match TcpStream::connect(&target_addr).await {
        Ok(mut target_stream) => {
          // Reconstruct the request with relative path
          let path = if url.query().is_some() {
            format!("{}?{}", url.path(), url.query().unwrap())
          } else {
            url.path().to_string()
          };

          let mut request = format!("{method} {path} HTTP/1.1\r\n");
          // Read remaining headers from client
          let mut headers = String::new();
          let mut content_length: usize = 0;
          let mut has_host = false;
          loop {
            let mut line = String::new();
            reader.read_line(&mut line).await?;
            if line.trim().is_empty() {
              break;
            }
            // Skip proxy-specific headers
            let lower = line.to_lowercase();
            if lower.starts_with("proxy-") {
              continue;
            }
            if lower.starts_with("host:") {
              has_host = true;
            }
            // Capture Content-Length for body forwarding
            if lower.starts_with("content-length:") {
              if let Some(val) = line.split(':').nth(1) {
                content_length = val.trim().parse().map_err(
                  |e| -> Box<dyn std::error::Error> {
                    format!("bad Content-Length: {e}").into()
                  },
                )?;
              }
            }
            headers.push_str(&line);
          }

          // Add Host header from URL if client didn't send one
          if !has_host {
            let host_header = if let Some(port) = url.port() {
              format!("Host: {host}:{port}\r\n")
            } else {
              format!("Host: {host}\r\n")
            };
            request.push_str(&host_header);
          }

          request.push_str(&headers);
          request.push_str("Connection: close\r\n");
          request.push_str("\r\n");

          target_stream.write_all(request.as_bytes()).await?;

          // Forward request body if present
          if content_length > 0 {
            let mut body = vec![0u8; content_length];
            let mut total_read = 0;
            while total_read < content_length {
              let n = reader.read(&mut body[total_read..]).await?;
              if n == 0 {
                break;
              }
              total_read += n;
            }
            target_stream.write_all(&body[..total_read]).await?;
          }

          // Forward response
          tokio::io::copy(&mut target_stream, &mut client).await?;
        }
        Err(_) => {
          let response = "HTTP/1.1 502 Bad Gateway\r\n\r\n";
          client.write_all(response.as_bytes()).await?;
        }
      }
    }
    _ => {
      let response = "HTTP/1.1 501 Not Implemented\r\n\r\n";
      client.write_all(response.as_bytes()).await?;
    }
  }

  Ok(())
}

fn is_domain_allowed(host: &str, allowed: &HashSet<String>) -> bool {
  let host = host.to_lowercase();
  for domain in allowed {
    let domain = domain.to_lowercase();
    if host == domain || host.ends_with(&format!(".{domain}")) {
      return true;
    }
  }
  false
}

#[cfg(test)]
mod tests {
  use super::*;
  use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
  use tokio::net::TcpListener;

  #[tokio::test]
  async fn test_is_domain_allowed() {
    let mut allowed = HashSet::new();
    allowed.insert("crates.io".to_string());
    allowed.insert("github.com".to_string());

    assert!(is_domain_allowed("crates.io", &allowed));
    assert!(is_domain_allowed("CRATES.IO", &allowed));
    assert!(is_domain_allowed("index.crates.io", &allowed));
    assert!(is_domain_allowed("github.com", &allowed));
    assert!(is_domain_allowed("api.github.com", &allowed));
    assert!(!is_domain_allowed("evil.com", &allowed));
    assert!(!is_domain_allowed("crates.io.evil.com", &allowed));
  }

  #[tokio::test]
  async fn test_proxy_blocks_denied_domain() {
    let mut allowed = HashSet::new();
    allowed.insert("allowed.com".to_string());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
      loop {
        let (stream, _) = listener.accept().await.unwrap();
        let allowed = allowed.clone();
        tokio::spawn(async move {
          let _ = handle_connection(stream, &allowed).await;
        });
      }
    });

    // Try to connect to a denied domain
    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
      .await
      .unwrap();
    let request = "CONNECT denied.com:443 HTTP/1.1\r\nHost: denied.com:443\r\n\r\n";
    stream.write_all(request.as_bytes()).await.unwrap();

    let mut response = String::new();
    let mut reader = BufReader::new(&mut stream);
    reader.read_line(&mut response).await.unwrap();

    assert!(response.contains("403"));
  }

  #[tokio::test]
  async fn test_proxy_allows_permitted_domain() {
    let mut allowed = HashSet::new();
    allowed.insert("httpbin.org".to_string());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
      loop {
        let (stream, _) = listener.accept().await.unwrap();
        let allowed = allowed.clone();
        tokio::spawn(async move {
          let _ = handle_connection(stream, &allowed).await;
        });
      }
    });

    // Connect to an allowed domain (will fail to connect to target, but proxy should try)
    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
      .await
      .unwrap();
    let request = "CONNECT httpbin.org:443 HTTP/1.1\r\nHost: httpbin.org:443\r\n\r\n";
    stream.write_all(request.as_bytes()).await.unwrap();

    let mut response = String::new();
    let mut reader = BufReader::new(&mut stream);
    reader.read_line(&mut response).await.unwrap();

    // Should get 200 (connection established) or 502 (bad gateway if target unreachable)
    // but NOT 403
    assert!(!response.contains("403"));
  }
}
