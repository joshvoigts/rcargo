# rdeploy

Build and deploy rust projects on remote servers via SSH.

## Setup

Create a `deploy.toml` in your project root:

```toml
target = "your-server"
remote_path = "/optional/path"  # Defaults to $HOME/build/{project_name}
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
