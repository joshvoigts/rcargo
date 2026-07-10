# rdeploy

Build and deploy rust projects on remote servers via SSH.

## Requirements

- `rsync`
- [zerobox](https://github.com/nicholasgasior/zerobox) — `cargo install zerobox`

## Setup

Create a `deploy.toml` in your project root:

```toml
target = "your-server"
remote_path = "/optional/path"  # Defaults to $HOME/build/{project_name}
```

### Sandbox

Remote builds run inside a [zerobox](https://github.com/afshinm/zerobox) sandbox by default. Reads are restricted to cargo/rustup dirs and the project. Writes and network are further restricted to only what's needed.

> **Note:** On Linux, zerobox may block binary execution from user paths.
> If builds fail with "Operation not permitted", disable the sandbox:
> ```toml
> [sandbox]
> enabled = false
> ```

To customize allowed paths:

```toml
[sandbox.allow]
read = ["/opt/shared/libs"]
write = ["/tmp/build-cache"]
net = ["internal.registry.com"]

[sandbox.deny]
read = [".secrets"]
write = [".git"]
```

## Usage

```
rdeploy build   # Build on remote, copy binary back locally
rdeploy run     # Build and run on remote
rdeploy stop    # Stop the running process on remote
```

### Flags

- `--target, -t` — Override the target host from `deploy.toml`
- `--branch, -b` — Override the branch (defaults to current branch)
