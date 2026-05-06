# perc

Scaffold, run, and deploy Rust web apps to your own VPS.

> **Early preview.** Perc is in active development and has rough edges. APIs will change, features will break, and there are gaps in what's implemented. Don't rely on it for anything serious yet. See the [website](https://perc.daz.is) for more context.

## Install

```
cargo install perc
```

Or build from source:

```
git clone https://github.com/crabby-utils/perc.git
cd perc
cargo install --path .
```

## Usage

```
perc new <name>      # scaffold a new Rust+Axum project
perc status          # show project status
perc --help          # see all commands and flags
```

### Create a project

```
perc new myapp
cd myapp
perc dev
```

Creates a directory with a ready-to-run Rust+Axum app, `perc.toml` config, and `.gitignore`. The app reads the `PORT` environment variable (default `8080`) and serves a hello-world route.

### Local development

Start the development environment with file watching:

```
perc dev
```

This reads `perc.toml` and:

1. Starts any configured service containers (PostgreSQL, RustFS, Restate) via Docker or Podman
2. Finds available ports and runs `cargo run` with service connection environment variables
3. Watches `src/` for changes and restarts the app automatically
4. Ctrl+C stops the app but leaves service containers running for fast restart

Services are only started when declared in `perc.toml`:

- `[database]` — PostgreSQL 18 on port 5432
- `[storage]` — RustFS (S3-compatible) on ports 9000 (S3 API) and 9001 (console)
- `[restate]` — Restate on ports 8080 (ingress) and 9070 (admin)

If `[restate]` is configured, both the main app and the worker binary run simultaneously on separate auto-allocated ports. The worker is registered with Restate automatically (and re-registered after each restart).

Manage service containers:

```
perc dev status    # show running services and ports
perc dev stop      # stop containers (data preserved)
perc dev reset     # stop and remove containers and volumes
```

**Requires Docker or Podman** with the daemon running.

#### Storage (S3-compatible)

Add S3-compatible object storage for local development:

```toml
[storage]
bucket = "my-bucket"
```

This starts a RustFS container and auto-creates the bucket. The app receives:

| Variable | Value |
|---|---|
| `S3_ENDPOINT` | `http://localhost:9000` |
| `S3_ACCESS_KEY` | `percdev` |
| `S3_SECRET_KEY` | `percdevsecret` |
| `S3_BUCKET` | bucket name from config |

The RustFS web console is available at `http://localhost:9001` (login: percdev / percdevsecret).

#### Environment variables in dev

| Variable | Source | When |
|---|---|---|
| `PORT` | Auto-allocated | Always |
| `DATABASE_URL` | Auto-generated | `[database]` present |
| `S3_ENDPOINT`, `S3_ACCESS_KEY`, `S3_SECRET_KEY`, `S3_BUCKET` | Fixed dev values | `[storage]` present |
| `RESTATE_INGRESS_URL` | `http://localhost:8080` | `[restate]` present |
| All `[env]` values | `perc.toml` | Always |

### Operator configuration

Store credentials and settings in `~/.config/perc/credentials.toml` (0600 permissions):

```
perc config set tailscale.authkey tskey-auth-...
perc config get tailscale.authkey
```

Environment variables override file values — `TAILSCALE_AUTHKEY` overrides `tailscale.authkey`. Use `PERC_CONFIG_DIR` to override the config directory location.

### Deploy

Bootstrap a fresh Ubuntu VPS for deployment:

```
perc deploy init <host>
```

This connects to `<host>` as root over SSH, then:

1. Updates system packages
2. Installs Tailscale and joins your tailnet (requires auth key — see above)
3. Installs Podman 5.0+
4. Locks down SSH (password auth disabled, port 22 restricted to Tailscale interface)
5. Configures UFW firewall (allows 80/443 for web traffic)
6. Creates a dedicated `perc` deploy user with a scoped sudoers policy
7. Verifies connectivity over Tailscale as the `perc` user
8. Records the target in `perc.toml`

After init, the host is only reachable via Tailscale. All subsequent commands connect as the `perc` user (not root), with sudo restricted to specific binaries only.

### Add an existing target

To deploy a second app to an already-initialized VPS, add it as a target in the new project:

```
perc deploy add my-vps  # replace with your Tailscale machine name
```

This connects via Tailscale SSH, verifies connectivity, and records the target in `perc.toml`. Use this instead of `init` when the host is already bootstrapped.

### Push an app

Build, ship, and start the app on a target:

```
perc deploy push
```

This:

1. Cross-compiles the Rust app for Linux (`x86_64-unknown-linux-musl`) using `cargo-zigbuild`
2. Builds a minimal OCI image in pure Rust (no local container runtime needed) — just the static binary, nothing else
3. Pipes the image to the target via `ssh podman load` (no registry needed)
4. Registers the app in the VPS-side registry and allocates a port
5. Generates a Caddyfile with reverse proxy blocks for all deployed apps
6. Deploys the app as a Podman Quadlet (systemd-managed container)
7. Verifies the app responds on the target

**Prerequisites:**

- [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild): `cargo install cargo-zigbuild`
- A target already bootstrapped with `perc deploy init` (or added with `perc deploy add`)

Use `--target <name>` to push to a specific target. Without it, pushes to the first configured target.

Deploy commands acquire a server-side lock to prevent concurrent deploys from corrupting state. If a previous deploy crashed and left a stale lock (auto-expires after 30 minutes), use `--force` to clear it:

```
perc deploy --force push
```

### Multiple apps on one VPS

Each perc project has its own `perc.toml` and deploys independently. When you push multiple apps to the same VPS, perc automatically:

- Assigns each app a unique port (starting from 8080)
- Generates a shared Caddyfile with one block per app
- Routes traffic by domain (each app needs its own domain for HTTPS)

A single app without a domain gets a `:80` fallback. Once you have multiple apps, assign domains to disambiguate.

Use `perc deploy status` from any project targeting the VPS to see all deployed apps.

### Set a domain

Associate a domain with a target for automatic HTTPS:

```
perc deploy domain example.com
```

This:

1. Saves the domain in `perc.toml` under the target
2. Updates the app's domain in the VPS registry
3. Regenerates the Caddyfile for all apps on the target
4. Reloads Caddy, which automatically provisions a Let's Encrypt TLS certificate

Make sure the domain's DNS A record points to the server's public IP before running this command. The app must have been pushed at least once before setting a domain.

Use `--target <name>` to set the domain for a specific target.

### Database

Add a PostgreSQL database to a deployed app:

```
perc deploy db
```

This:

1. Installs PostgreSQL on the VPS if not already present and auto-tunes it for available RAM (25% budget)
2. Creates a dedicated database and user for the app
3. Injects `DATABASE_URL` into the container environment (sqlx-compatible format)
4. Restarts the container

After running, the app can connect using the `DATABASE_URL` environment variable. Removing an app with `perc deploy remove` also drops its database and user.

#### Migrations

Perc provisions the database but does not run migrations — that's your app's responsibility. The recommended approach is to run migrations at startup:

```rust
sqlx::migrate!().run(&pool).await?;
```

This ensures the schema is always up to date after each deploy, with no extra commands or tooling required.

To have the database provisioned automatically on every push, add a `[database]` section to `perc.toml`:

```toml
[app]
name = "myapp"

[database]
```

With this section present, `perc deploy push` ensures PostgreSQL is installed and the database exists before deploying. Credentials are stored in the VPS registry, not in `perc.toml`.

Multiple apps on the same VPS share a single PostgreSQL instance but each gets its own database and user with a unique password.

### Restate (durable execution)

Add Restate support for durable workflows by adding a `[restate]` section to `perc.toml`:

```toml
[app]
name = "myapp"

[restate]
worker = "myapp-worker"
```

The `worker` field names the Cargo binary that serves Restate endpoints. If omitted, it defaults to `{app_name}-worker`. Your Cargo project should declare a second binary:

```toml
[[bin]]
name = "myapp-worker"
path = "src/worker.rs"
```

When `[restate]` is present, `perc deploy push`:

1. Cross-compiles both the main server and worker binaries
2. Builds separate OCI images for each
3. Installs Restate server on the VPS if not already present (shared across apps, runs as a systemd service)
4. Deploys the worker as a separate container with its own allocated port
5. Registers the worker with Restate (`restate deployments register --force`)
6. Injects `RESTATE_INGRESS_URL` into both containers so they can invoke Restate handlers

The Restate server uses port 9080 for ingress (instead of the default 8080, to avoid conflicts with app ports) and 9070 for admin.

Both the main app and worker containers use `Network=host` so they can communicate with Restate on localhost. The worker is also restarted when secrets change via `perc deploy secret set/unset`.

Removing an app with `perc deploy remove` also stops and removes the worker container.

### Include files

Bundle extra files or directories into the container alongside the binary:

```toml
[app]
name = "myapp"
include = ["prompts", "static/config.json"]
```

Each entry is copied into the container at the same relative path. Directories are included recursively. The binary runs with `/` as its working directory, so `prompts/expand-base.md` in your project becomes `/prompts/expand-base.md` in the container.

### Environment variables

Manage non-secret environment variables in `perc.toml` with `perc env`:

```
perc env set S3_REGION=us-east-1 S3_BUCKET=mybucket
perc env unset S3_REGION
perc env list
```

This updates the `[env]` table in `perc.toml`:

```toml
[env]
S3_REGION = "us-east-1"
S3_BUCKET = "mybucket"
S3_ENDPOINT = "https://s3.amazonaws.com"
```

These are injected as `Environment=` directives in the container on every push. Safe to commit to version control.

### Secrets

For secrets (API keys, passwords), use `perc deploy secret` to store them on the VPS:

```
perc deploy secret set S3_ACCESS_KEY=AKIAIOSFODNN7EXAMPLE S3_SECRET_KEY=wJalrXUtnFEMI/K7MDENG
perc deploy secret unset S3_SECRET_KEY
perc deploy secret list
perc deploy secret list --reveal
```

`secret set` and `secret unset` update the VPS registry and restart the container immediately. Secrets persist across pushes — they are stored in the VPS registry (`/var/lib/perc/apps.toml`), never in `perc.toml` or version control.

`secret list` masks values by default to prevent accidental exposure in CI logs or screen sharing. Use `--reveal` to show full values.

When both sources define the same key, the VPS secret takes precedence over the `perc.toml` value. `DATABASE_URL` (managed by `perc deploy db`) takes precedence over both.

The app must be deployed before setting secrets. Use `--target <name>` to manage secrets for a specific target.

### Show deployed apps

```
perc deploy status
```

Connects to the target and displays all deployed apps, their ports, and domains. Use `--target <name>` to query a specific target.

### View logs

Show recent logs for the deployed app:

```
perc deploy logs
```

By default, shows the last 50 lines. Use `--lines` / `-n` to control how many:

```
perc deploy logs -n 200
```

Stream logs in real time (like `tail -f`):

```
perc deploy logs --follow
perc deploy logs -f
```

Use `--target <name>` to view logs from a specific target. Press Ctrl+C to stop following.

### Remove an app

```
perc deploy remove [name]
```

Removes an app from the target: unregisters it, regenerates the Caddyfile, stops the container, and removes the Quadlet unit. Defaults to the current project's app name if no name is given. Use `--target <name>` to remove from a specific target.

### Global flags

- `--target <name>` — select deploy target
- `--json` — machine-readable JSON output
- `-v` / `-vv` / `-vvv` — increase log verbosity

## Project config

Create a `perc.toml` in your project root:

```toml
[app]
name = "myapp"
include = ["prompts", "static/config.json"]  # optional — files/dirs bundled into the container

[env]  # optional — non-secret environment variables injected into the container
S3_REGION = "us-east-1"
S3_BUCKET = "mybucket"

[database]  # optional — provisions a PostgreSQL database on push

[storage]  # optional — S3-compatible storage via RustFS (local dev only)
bucket = "my-bucket"

[restate]  # optional — installs Restate and deploys a worker binary
worker = "myapp-worker"  # defaults to "{app_name}-worker" if omitted

[targets.production]
host = "example.com"
domain = "myapp.example.com"
```

## Development

To contribute to perc itself:

```
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```
