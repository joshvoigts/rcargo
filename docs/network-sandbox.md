# Network Sandboxing

rcargo enforces domain-based network filtering using a local HTTP
proxy combined with kernel-level sandboxing to force all traffic
through the proxy.

## Architecture

```
┌─────────────────────────────────────────┐
│           Sandbox Process               │
│  (cargo build, cargo test, etc.)        │
│                                         │
│  HTTP_PROXY=http://127.0.0.1:<port>     │
│  HTTPS_PROXY=http://127.0.0.1:<port>    │
└────────────────────┬────────────────────┘
                     │
                     ▼
┌─────────────────────────────────────────┐
│        Network Proxy (localhost)        │
│                                         │
│  - HTTP CONNECT tunneling (HTTPS)       │
│  - HTTP request forwarding              │
│  - Domain allowlist enforcement         │
│  - Blocks denied domains                │
│  - Forces Connection: close             │
└────────────────────┬────────────────────┘
                     │
                     ▼
┌─────────────────────────────────────────┐
│         Kernel Sandbox                  │
│                                         │
│  Linux: Landlock ABI V4 (5.19+)        │
│  - TCP connect to proxy port only       │
│  - UDP unrestricted (DNS works)         │
│  - All other TCP blocked                │
│                                         │
│  macOS: Seatbelt                        │
│  - TCP to localhost:proxy_port          │
│  - UDP unrestricted (DNS works)         │
│  - All other TCP blocked                │
└─────────────────────────────────────────┘
```

## How it works

1. **Proxy Start**: Before applying the sandbox, the shim starts a
   local HTTP proxy on a random localhost port.

2. **Environment Setup**: The proxy URL is set via `HTTP_PROXY`,
   `HTTPS_PROXY`, and `ALL_PROXY` environment variables for the
   sandboxed command. `Connection: close` is enforced to prevent
   keep-alive connections from bypassing the proxy.

3. **Kernel Enforcement**: The sandbox restricts TCP network access
   to only the proxy port, preventing direct external TCP
   connections. UDP is unrestricted, allowing DNS resolution to
   work through the system resolver.

4. **Domain Filtering**: The proxy checks each connection against
   the allowlist in `sandbox.allow.net` and blocks unauthorized
   domains.

## Configuration

In `rcargo.toml`:

```toml
[sandbox]
enabled = true

[sandbox.allow]
net = [
  "crates.io",
  "index.crates.io",
  "static.crates.io",
  "static.rust-lang.org",
  "github.com",
]
```

## Default allowed domains

When `sandbox.allow.net` is empty but network access is enabled,
the following domains are allowed by default for cargo operations:

- `crates.io`
- `index.crates.io`
- `static.crates.io`
- `static.rust-lang.org`
- `github.com`

## Platform support

| Platform | Kernel Sandbox | TCP Filtering | UDP/DNS |
|----------|----------------|---------------|---------|
| Linux    | Landlock ABI V4 (kernel 5.19+) | Proxy port only | Unrestricted |
| macOS    | Seatbelt | Proxy port only | Unrestricted |
| Other    | None | Proxy-only | Unrestricted |

## Limitations

- **UDP traffic**: Landlock ABI V4 and Seatbelt only restrict TCP.
  UDP traffic (including DNS) passes through the kernel sandbox
  unrestricted. The proxy only handles TCP (HTTP/HTTPS), so UDP
  is not filtered at the application layer either.

- **DNS leakage**: A malicious process could send arbitrary UDP
  packets (e.g., DNS queries to exfiltrate data) without going
  through the proxy. This is a minor risk for build sandboxing
  since DNS responses are limited in size and the primary threat
  is code execution, not data exfiltration.

- **No UDP proxy support**: The proxy handles HTTP/HTTPS (TCP
  only). Any application that requires UDP beyond DNS (e.g., NTP)
  will work but is not filtered.
