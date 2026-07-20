# rcargo

Run cargo on a remote host and stream output back locally.

## Requirements

- `rsync`
- `nono` — [github.com/nolabs-ai/nono](https://github.com/nolabs-ai/nono) (only when sandbox is enabled)
- On Linux: a kernel supporting Landlock LSM (5.13+) (only when sandbox is enabled)

## Setup

Create a `deploy.toml` (or `rcargo.toml`) in your project root:

```toml
target = "your-server"
remote_path = "/optional/path"  # Defaults to $HOME/build/{project_name}
```

`$HOME` in `remote_path` is resolved on the remote host.

### Sandbox

Remote builds run inside a [nono](https://github.com/nolabs-ai/nono) sandbox by default. nono uses Landlock (Linux) / Seatbelt (macOS) for kernel-level filesystem sandboxing — deny-all reads, then whitelist specific paths. Binary execution works because the filesystem is intact; the kernel just denies access to non-whitelisted paths.

Network is proxied with a domain allowlist. The default allowed domains are:

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

## Usage

Before any command runs, rcargo verifies SSH connectivity to the remote host.

Code is synced to the remote via `rsync`, which excludes `.git` and respects `.gitignore` so build artifacts and databases are untouched.

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
- `--branch, -b` — Override the branch (defaults to current branch)
- `--debug` — Enable debug output for any command
