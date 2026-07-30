# rcargo

Run cargo on a remote host and stream output back locally.

## Requirements

- `ssh` and `scp`
- On Linux: a kernel supporting Landlock LSM (5.13+) (only when sandbox is enabled)

## Setup

Create a `deploy.toml` (or `rcargo.toml`) in your project root:

```toml
target = "your-server"
remote_path = "/optional/path"  # Defaults to $HOME/build/{project_name}
```

`$HOME` in `remote_path` is resolved on the remote host.

### Sandbox

Remote builds run inside a sandbox by default. The sandbox provides:

- **Filesystem sandboxing**: Landlock (Linux) / Seatbelt (macOS) restricts access to whitelisted paths only.
- **Network sandboxing**: A local HTTP proxy enforces domain allowlisting, with kernel-level enforcement to prevent direct external connections.

The default allowed network domains are:

- `crates.io`
- `index.crates.io`
- `static.crates.io`
- `static.rust-lang.org`
- `github.com`

To disable the sandbox:

```toml
[sandbox]
enabled = false
```

#### Environment variables

Pass environment variables to the remote build (e.g. for `sqlx`):

```toml
[sandbox.env]
DATABASE_URL = "sqlite://db.sqlite3"
```

#### Additional allowed paths

```toml
[sandbox.allow]
write = ["/opt/build-cache"]
net = ["internal.registry.com"]
```

### Hooks

Shell commands that run on the remote host **outside the sandbox** before the build. Useful for database setup, migrations, etc.

```toml
[hooks]
prebuild = "sqlx database create && sqlx migrate run"
```

Or as a list:

```toml
[hooks]
prebuild = [
  "sqlx database create",
  "sqlx migrate run",
]
```

Hooks inherit the environment variables from `[sandbox.env]`.

## Architecture

See `docs/architecture.md` for an overview of the system design,
command flow, and key decisions (e.g. why check/clippy skip
sandboxing).

## Usage

Before any command runs, rcargo verifies SSH connectivity to the remote host.

Code is synced to the remote via the shim's delta sync protocol,
which excludes `.git` and respects `.gitignore` so build artifacts
and databases are untouched.

```
rcargo build          # Build on remote (sandboxed)
rcargo check          # Check code on remote (cargo check, no sandbox)
rcargo run            # Stop existing process, build, and launch on remote
rcargo stop           # Stop the running process on remote
rcargo test           # Run tests on remote (sandboxed)
rcargo test -- --skip foo  # Pass extra args to cargo test
```

### Flags

- `--target, -t` — Override the target host from config
- `--debug` — Enable debug output for any command
