# Architecture

## Overview

rcargo runs cargo commands on a remote host and streams output back
locally. The system has three components:

1. **Client** (`rcargo`) — CLI tool that orchestrates sync, hooks,
   and remote execution over SSH.
2. **Protocol** (`rcargo-protocol`) — Shared wire format for
   length-prefixed messages between client and shim.
3. **Shim** (`rcargo-shim`) — Lightweight binary deployed to the
   remote host. Handles file sync (via `fast_rsync` deltas) and
   sandboxing (via Landlock/Seatbelt).

## Command Flow

```
rcargo build
  → SSH: check shim version (bootstrap if needed)
     ssh host ~/.rcargo/shim <expected_version>
     exit 0 → version matches, skip deploy
     exit 1 → deploy new binary via scp
  → SSH+shim: sync files via delta protocol
  → SSH: run prebuild hooks (outside sandbox)
  → SSH+shim: sandbox setup + execute cargo build
  → stream output to local terminal
```

## Sync Protocol

The client and shim communicate over stdin/stdout using
length-prefixed UTF-8 messages. Binary data (signatures, file
contents, deltas) is base64-encoded within the payload.

Key phases:
1. Handshake — shim identifies version, OS, arch
2. File listing — shim sends FILE messages for each remote file
3. Sync actions — client sends SKIP/DELETE/DATA/DELTA per file
4. Run — client sends the command, shim executes it

See `docs/shim-design.md` for the full protocol specification.

## Sandboxing

Sandboxing is applied by the shim just before executing the command.
The client sends sandbox configuration (allowed paths, network
domains) via the SANDBOX protocol message.

### Build and Test (`rcargo build`, `rcargo test`)

These run inside the sandbox. The shim applies Landlock (Linux) or
Seatbelt (macOS) restrictions before executing cargo.

### Check and Clippy (`rcargo check`, `rcargo clippy`)

These do **not** run inside the sandbox. `cargo check` and
`cargo clippy` are read-only analysis passes that don't execute
target code, so sandboxing is unnecessary. They use delta sync
to transfer files, then execute via direct SSH.

## Hooks

Prebuild hooks run on the remote host **outside the sandbox**,
via a separate SSH session. They inherit environment variables
from `[sandbox.env]` in the configuration.

Hook ordering: for all commands, files are synced first, then
hooks run, then the cargo command executes (sandboxed for
build/test, unsandboxed for check/clippy).

## Deploy

`rcargo deploy` sets up a systemd user service on the remote host.
The deploy flow:

1. Stop any existing process (systemd service or PID file)
2. Sync files via shim
3. Run prebuild hooks
4. Build via shim (sandboxed)
5. Configure and start systemd service

The binary is built in `target/release/` and the systemd service
points to `{remote_path}/target/release/{bin_name}`.
