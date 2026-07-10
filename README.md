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

Remote builds run inside a [zerobox](https://github.com/nicholasgasior/zerobox) sandbox by default with access to cargo/rustup dirs and crate registries. To customize or disable:

```toml
[sandbox]
enabled = false  # Skip sandbox entirely

[sandbox.allow]
read = ["/opt/shared/libs"]
write = ["/tmp/build-cache"]
net = ["internal.registry.com"]

[sandbox.deny]
read = [".edwin"]
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
