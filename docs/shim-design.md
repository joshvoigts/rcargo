# Shim Design

Eliminate `rsync` and remote `nono` CLI dependencies by shipping a lightweight
shim binary that handles sync (via `fast_rsync`) and sandboxing (via
`landlock` / Seatbelt FFI) natively.

The shim is not a long-running process — it runs once per invocation and exits when the
command exits. It's a drop-in replacement for the `nono run ...` wrapper:
sync files, apply sandbox, exec command, done.

## Goals

- No `rsync` required on the local machine
- No `nono` CLI required on the remote machine
- Delta transfer for changed files (skip unchanged, patch changed)
- Same sandboxing guarantees as current `nono` approach
- Minimal added dependencies (~2–3 MB total)

## Dependencies (added)

| Crate | Size | Purpose | Platform |
|-------|------|---------|----------|
| `fast_rsync` 0.2.0 | 76 KB | Delta transfer algorithm | All |
| `landlock` 0.4.5 | 220 KB | Filesystem sandboxing | Linux |
| (Seatbelt FFI) | ~0 | Filesystem + network sandboxing | macOS |

No `nono` library — its ~113 MB of deps are sigstore/keyring/cert stuff
we don't need. We only need raw Landlock rulesets and Seatbelt profiles.

## Binary Distribution

The `rcargo` client binary embeds prebuilt shim binaries as base64 strings
for four targets. The shim is a separate cross-compiled binary — the client
extracts and deploys it to the remote host.

Target triples:

```
x86_64-unknown-linux-gnu
aarch64-unknown-linux-gnu
x86_64-apple-darwin
aarch64-apple-darwin
```

### Client Binary

- **Client mode** (default): current behavior, talks to remote via SSH
- **Shim mode** (`rcargo --shim`): reads sync protocol from stdin,
  writes to stdout, runs sandboxed commands. This mode is only used
  when the client is testing the shim locally.

Build process (CI or `just build-all`):

```
# Cross-compile shim for each target
cargo build --release --target x86_64-unknown-linux-gnu
cargo build --release --target aarch64-unknown-linux-gnu
cargo build --release --target x86_64-apple-darwin
cargo build --release --target aarch64-apple-darwin

# Embed as base64 constants
src/shim_embed.rs:
const SHIM_LINUX_X86_64: &str = "<base64>";
const SHIM_LINUX_AARCH64: &str = "<base64>";
const SHIM_MACOS_X86_64: &str = "<base64>";
const SHIM_MACOS_AARCH64: &str = "<base64>";
```

Cross-compilation for Linux targets on macOS requires `cross` or
`rustup target add` + appropriate musl toolchains. Darwin cross-compilation
is native (`rustup target add`).

The `ignore` crate is also added as a dependency for `.gitignore` pattern
matching during local file listing.

## Bootstrap Protocol

On every run, the client checks for the shim on the remote:

```
1. ssh host "uname -s && uname -m"
   → detect OS + arch, select embedded binary

2. ssh host "test -x $SHIM_PATH && $SHIM_PATH --version"
   → if succeeds and version matches, skip bootstrap

3. Bootstrap (base64 decode + chmod):
   ssh host "echo '<base64>' | base64 -d > $SHIM_PATH && chmod +x $SHIM_PATH"

4. Verify:
   ssh host "$SHIM_PATH --version"
```

`$SHIM_PATH` defaults to `$HOME/.rcargo/shim`.

If bootstrap fails, fall back to rsync (if available) or error.

## Sync Protocol

Length-prefixed binary messages over stdin/stdout between client and shim.

Wire format: `<4 bytes big-endian length><payload>`
(no trailing delimiter — the length field is authoritative)

All payloads are UTF-8 strings. Binary data (signatures, file contents,
deltas) is base64-encoded within the payload.

Conventions in the message descriptions below:

- `→` = shim writes to stdout (client reads from SSH pipe)
- `←` = client writes to shim's stdin (via SSH pipe)

### Message Types

```
# ─── Phase 1: Handshake ───

→ HANDSHAKE <version> <os> <arch>
  Shim identifies itself.

# ─── Phase 2: File Listing ───

← LIST
  Client requests the shim's file listing.

→ FILE <path> <size> <mtime>
  One per file in the remote working tree (relative paths).
  mtime is Unix timestamp in seconds.

→ END_LIST
  Shim signals end of its file listing.

# ─── Phase 3: Sync Actions ───

  The client computes the diff locally (remote_files vs local_files)
  and sends a stream of sync action messages:

← SKIP <path>
  File unchanged (same size + mtime). No action needed.

← UPLOAD <path> <size>
  File is new or changed. Client will send full file data next.

← DELETE <path>
  File exists on remote but not locally. Shim removes it.

← DELTA <path> <delta_bytes_base64>
  File changed. Client sends a precomputed fast_rsync delta.
  (The client fetches signatures via batched SIG messages — see below.)

← DATA <path> <data_bytes_base64>
  Client sends full file content (for UPLOAD).

← END_SYNC
  Client signals all sync actions sent.

→ ERROR <message>
  Shim reports an error (e.g. disk full, permission denied).
  Client should abort sync and report the error.

→ OK
  Shim confirms receipt of a message or completion of an action.

# ─── Phase 4: Command Execution ───

← RUN <command_string>
  Client sends the command to run (e.g. sandboxed cargo build).
  After this message, stdout carries raw command output (no protocol).
  The shim's exit code = the command's exit code.
```

### Signature Batching (Delta Transfer)

For changed files, the client needs the shim's `fast_rsync` signatures
before it can compute deltas. Rather than round-tripping per file, the
client first sends DELTA requests, then the shim responds with all
signatures at once:

```
# Client sends all delta requests up front:
← DELTA_REQUEST <path>
← DELTA_REQUEST <path>
...

# Shim responds with all signatures:
→ SIG <path> <sig_base64>
→ SIG <path> <sig_base64>
...

→ END_SIGS
  Shim signals all signatures sent.

# Client computes deltas locally and sends them:
← DELTA <path> <delta_base64>
← DELTA <path> <delta_base64>
...
```

This collapses N round trips into 3 (requests → signatures → deltas).

### Client-Side Sync Logic

```rust
let remote_files: HashMap<String, (u64, u64)> = // from FILE messages
let local_files: HashMap<String, (u64, u64)> = // from walkdir + metadata
let mut delta_requests: Vec<String> = Vec::new();

// Phase 3a: Send sync actions, collect delta requests
for (path, (size, mtime)) in &local_files {
    match remote_files.get(path.as_str()) {
        Some((r_size, r_mtime)) if r_size == size && r_mtime == mtime => {
            send(SKIP { path });
        }
        Some(_) => {
            // File changed — request signature for delta transfer
            delta_requests.push(path.clone());
        }
        None => {
            // New file — full upload
            send_upload(path, local_data(path));
        }
    }
}

// Files on remote but not local → delete
for path in remote_files.keys().filter(|p| !local_files.contains_key(p)) {
    if !should_exclude(path) {
        send(DELETE { path });
    }
}

// Send all delta requests, receive all signatures
for path in &delta_requests {
    send(DELTA_REQUEST { path });
}
send(END_SIGS_REQUEST);

let sigs = receive_signatures(); // Vec<(path, Signature)>
for (path, sig) in &sigs {
    let local_data = std::fs::read(&path)?;
    let mut delta = Vec::new();
    fast_rsync::diff(&sig, &local_data, &mut delta)?;
    send(DELTA { path, delta_base64: base64(&delta) });
}

send(END_SYNC);
```

### Exclusion Handling

Exclusions (`.git`, `target/`, `.gitignore` patterns) are evaluated
**locally** when building `local_files`. The shim's file list is used
to detect deletions, but deletions are also filtered — we never delete
paths that match exclusion patterns (so `target/` on the remote is
untouched even if it doesn't exist locally).

```rust
fn should_exclude(path: &str) -> bool {
    // Hard-coded exclusions
    if path.starts_with(".git") { return true; }
    if path.starts_with("target/") { return true; }

    // .gitignore patterns (via `ignore` crate)
    ignore::gitignore(path)
}
```

### Data Integrity

`fast_rsync` uses the legacy MD4 hash format for signatures and deltas.
While fine for delta computation, we should not trust that `apply`
produces correct output without verification. For each patched file,
the client computes a SHA-256 hash of the local file and includes it
in the `DELTA` message. The shim verifies after applying:

```
← DELTA <path> <delta_bytes_base64> <sha256_hex>
```

If verification fails, the shim falls back to requesting a full upload.

## Sandboxing

The shim applies sandboxing **just before** executing the command.
No `nono` CLI needed — direct kernel primitives.

### Linux: Landlock

```rust
use landlock::{
    path_beneath_rules, Access, ABI, CompatLevel, HandleAccess,
    RestrictSelfAttr, Ruleset, RulesetAttr,
};

fn apply_sandbox(paths: &[SandboxPath]) -> Result<()> {
    let mut ruleset = Ruleset::new()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(Access::FS_READ)?
        .handle_access(Access::FS_WRITE)?
        .create()?;

    for p in paths {
        let access = match p.access {
            Access::Read => Access::FS_READ,
            Access::ReadWrite => Access::FS_READ | Access::FS_WRITE,
        };
        let rule = path_beneath_rules(&[p.path.as_path()], access)?;
        ruleset.add_rules(rule)?;
    }

    // Restrict self + all future children. ABI version determines
    // which features are available (network filtering needs v4+).
    let status = ruleset.restrict_self(ABI::V3)?;
    // status.ruleset tells us what was actually enforced
    Ok(())
}
```

Network filtering requires Landlock ABI v4 (kernel 5.19+). On older
kernels, fall back to allow-all network (same as current nono behavior).
Note: Landlock network rules only restrict TCP bind/connect — UDP and
DNS resolution are not restricted.

### macOS: Seatbelt FFI

Seatbelt profiles use a Scheme-like DSL (not JSON/plist). The shim
generates a profile string dynamically from the same config that
currently drives `nono` CLI flags.

```rust
fn build_seatbelt_profile(
    paths: &[SandboxPath],
    domains: &[String],
) -> String {
    let mut rules = String::from(r#"(
  (version 1)
  (deny default)
"# );

    // Allow read access to specified paths
    for p in paths {
        if p.access == Access::Read || p.access == Access::ReadWrite {
            rules.push_str(&format!(
                "  (allow file-read* (subpath \"{}\"))\n",
                p.path.display()
            ));
        }
    }

    // Allow write access to read-write paths
    for p in paths {
        if p.access == Access::ReadWrite {
            rules.push_str(&format!(
                "  (allow file-write* (subpath \"{}\"))\n",
                p.path.display()
            ));
        }
    }

    // Network: allow TCP connect to specified port ranges
    if !domains.is_empty() {
        rules.push_str("  (allow network* (remote tcp))\n");
    }

    rules.push(')');
    rules
}

extern "C" {
    fn sandbox_init(
        profile: *const c_char,
        flags: u64,
        error: *mut *mut c_char,
    ) -> c_int;
}

fn apply_seatbelt(profile: &str) -> Result<()> {
    let c_profile = CString::new(profile)?;
    let mut err: *mut c_char = std::ptr::null_mut();
    let rc = unsafe { sandbox_init(c_profile.as_ptr(), 0, &mut err) };
    if rc != 0 {
        let msg = unsafe { CStr::from_ptr(err) };
        return Err(format!("sandbox_init failed: {}", msg.to_string_lossy()).into());
    }
    Ok(())
}
```

nono uses Apple's private `sandbox_init()` API rather than the newer
`sandbox_apply_container()`. While technically undocumented, this API
has been stable for over a decade and is widely used by third-party
tools (including `sandbox-exec`).

### Command Execution Flow

```rust
// In shim mode, after END_SYNC:
let cmd = receive_run_command()?;

if sandbox.enabled {
    apply_sandbox(&sandbox.paths)?;
}

// Spawn command as child process.
// Landlock/Seatbelt restrictions are inherited by child processes
// and cannot be removed — the command runs fully sandboxed.
let status = Command::new("bash")
    .args(["--norc", "--noprofile", "-c", &cmd])
    .status()?;

std::process::exit(status.code().unwrap_or(1));
```

Note: We use `Command::new().status()` (not `exec()`) so the shim
process remains the PID visible to the remote host. This matters for
the `stop_server` flow which tracks PIDs via pid files. Signal handling
is handled by the shim forwarding signals to its child process group.

## Output Streaming

After the `RUN` message, the protocol ends. The shim's stdout/stderr
become the command's stdout/stderr, flowing directly through the SSH
connection to the client.

The shim must flush stdout after each protocol message to avoid
buffering conflicts with raw command output. After the `RUN` message
is sent, the shim switches stdout to raw passthrough mode (no more
protocol framing).

The client uses the same `ssh -t` + SGR filtering as today:

```
// Client opens SSH to: $SHIM_PATH --shim
// Protocol messages flow through until RUN
// After RUN, raw command output flows through
// SSH exit code = command exit code = shim exit code
```

No change needed to the existing `filter_sgr` logic.

## Integration Points

### Current `git::sync_repo` → `shim_sync`

```rust
// Before:
git::sync_repo(&config.target, remote_path)?;

// After:
ensure_shim(&config.target)?;
shim_sync(&config.target, remote_path)?;
```

### Current `sandbox::build_cmd` → shim handles sandboxing

```rust
// Before:
let cmd = sandbox::build_cmd(config, remote_path, home, debug);
ssh::ssh_run(&config.target, &cmd)?;

// After: shim receives the command via RUN message,
// applies sandbox, and execs it.
// The client just sends the inner command (no nono wrapper).
```

The `sandbox::build_cmd` function still exists on the client side to
construct the command string, but it omits the `nono run ...` wrapper.
The shim applies sandboxing natively.

### Fallback

If the shim is unavailable or bootstrap fails:

1. Try rsync (if `which rsync` succeeds)
2. Try tar-over-ssh: `tar cz --exclude=.git . | ssh host 'cd path && tar xz'`
3. Error with clear message

The fallback path preserves backward compatibility during rollout.

## Rollout Plan

1. **Phase 0**: Shim binary scaffold — `--shim` mode, protocol framing,
   handshake, version check. No sync, no sandbox. Just validates the
   bootstrap and exec flow works end-to-end.
2. **Phase 1**: Full file upload sync (no delta). Shim writes received
   files to disk. Client sends DATA messages for all non-excluded files.
3. **Phase 2**: Add `fast_rsync` delta transfer. Batched signature
   exchange, client-side delta computation, SHA-256 verification.
4. **Phase 3**: Add Landlock sandboxing (Linux).
5. **Phase 4**: Add Seatbelt sandboxing (macOS).
6. **Phase 5**: Binary embedding + bootstrap. Cross-compile shim for
   4 targets, embed as base64 in client binary.
7. **Phase 6**: Fallback logic, keep rsync path as option.

## Risks

- **Cross-compilation**: Requires setting up build targets in CI. Straightforward
  but adds build complexity.
- **Remote rustc not needed**: Shim is a prebuilt binary, no compilation on remote.
- **Version drift**: If client and shim versions mismatch, protocol may break.
  Mitigated by version check in HANDSHAKE.
- **Seatbelt FFI**: Untested territory for rcargo. nono's macOS code can serve
  as reference. Small surface area (~200 lines).
- **fast_rsync MD4**: Uses legacy MD4 hashes. Not a security concern (we're not
  authenticating), but we verify patched output with SHA-256 to catch corruption.
- **Landlock network limits**: Landlock ABI v4+ network rules only restrict TCP
  bind/connect. UDP and DNS are unrestricted. On kernels < 5.19, no network
  filtering at all — same as current nono behavior.
- **SSH pipe buffering**: Protocol messages and raw command output share the same
  stdout pipe. The shim must flush after each protocol message to avoid interleaving.
  If SSH buffers aggressively, there could be startup latency for command output.
