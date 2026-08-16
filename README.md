# rcargo

Run cargo on a remote host and stream output back locally.

## Requirements

- `rsync`
- `nono` — [github.com/nolabs-ai/nono](https://github.com/nolabs-ai/nono) (only when sandbox is enabled)
- On Linux: a kernel supporting Landlock LSM (5.13+) (only when sandbox is enabled)

## Setup

Configuration lives in TOML files layered in two places:

- **Project** — `rcargo.toml` in the repo root (`deploy.toml` is still accepted for backwards compatibility)
- **Global** — `$XDG_CONFIG_HOME/rcargo/rcargo.toml` (default `~/.config/rcargo/rcargo.toml`), providing defaults shared across all projects on this machine

Project values **replace** global ones: whichever key the project defines wins, including entire sub-tables (no merging). Leave a key out of the project and the global value is used.

Minimal project config:

```toml
target = "your-server"
remote_path = "/optional/path"  # Full path, overrides everything below
remote_build_dir = "/home/james/build"  # Or give a base dir; repo lands at {dir}/{project_name}
```

If neither is set, the repo lands at `$HOME/build/{project_name}`. `remote_build_dir` joins the project name onto the base dir (e.g. `remote_build_dir = "/home/james/build"` with package `myapp` → `/home/james/build/myapp`). `remote_path` takes precedence over `remote_build_dir` and lets you pin an exact path. `$HOME` in either is resolved on the remote host.

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

### Prebuild

Shell commands that run on the remote host **outside the sandbox** before the build. Useful for database setup, migrations, etc.

```toml
prebuild = "sqlx database create && sqlx migrate run"
```

Or as a list:

```toml
prebuild = [
  "sqlx database create",
  "sqlx migrate run",
]
```

Prebuild commands inherit the environment variables from `[sandbox.env]`.

### Commands

Define named command sequences that run as a single remote session (one sync, one prebuild run, stopped on the first failure). A command maps a name to an ordered list of steps; each step is a built-in command optionally followed by its args.

```toml
[commands]
ci    = ["lint", "test --workspace -q"]
check = ["clippy", "test"]
```

Available step commands: `lint`, `clippy`, `check`, `test`, `build`.

Run a command by name:

```
rcargo ci
```

Global flags (like `--debug`) must come **before** the command name:

```
rcargo --debug ci
```

Command names must not collide with the built-in subcommands (`build`, `check`, `clippy`, `lint`, `run`, `stop`, `test`, `deploy`, `undeploy`, `status`) — those are dispatched to the built-in behavior, not to `[commands]`. A misspelled command (e.g. `rcargo buid`) also reports `no command 'buid' defined in [commands]`.

## Usage

Before any command runs, rcargo verifies SSH connectivity to the remote host.

Code is synced to the remote via `rsync`, which excludes `.git` and respects `.gitignore` so build artifacts and databases are untouched.

```
rcargo build          # Build on remote (sandboxed)
rcargo check          # Check code on remote (cargo check, sandboxed)
rcargo clippy         # Run clippy on remote (cargo clippy, sandboxed)
rcargo lint           # Run lint on remote (cargo lint, via lint xtask, sandboxed)
rcargo run            # Stop existing process, build, and launch on remote
rcargo stop           # Stop the running process on remote
rcargo test                  # Run tests on remote (sandboxed)
rcargo test -- --skip foo    # Pass extra args to cargo test
rcargo <command>             # Run a user-defined command from [commands] (e.g. `rcargo ci`)
```

### Flags

- `--target, -t` — Override the target host from config
- `--branch, -b` — Override the branch (defaults to current branch)
- `--package, -p` — Workspace member to install (overrides config)
- `--bin` — Binary name override (defaults to auto-detect from [[bin]] target)
- `--timeout` — Timeout in seconds for remote commands (default: 600)
- `--debug` — Enable debug output for any command
