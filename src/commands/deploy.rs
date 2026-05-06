use std::collections::BTreeMap;
use std::path::Path;
use std::process;
use std::time::Duration;

use color_eyre::eyre::{self, WrapErr};
use openssh::{KnownHosts, Session, SessionBuilder};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use tokio::io::AsyncBufReadExt;

use crate::config;
use crate::output::Output;

#[derive(Serialize)]
struct InitResult {
    tailscale_hostname: String,
    tailscale_ip: String,
    podman_version: String,
    target_recorded: bool,
}

#[derive(Serialize)]
struct PushResult {
    app_name: String,
    target: String,
    image: String,
    port: u16,
}

#[derive(Serialize)]
struct DomainResult {
    domain: String,
    target: String,
}

const BASE_PORT: u16 = 8080;
const REGISTRY_PATH: &str = "/var/lib/perc/apps.toml";
const LOCK_PATH: &str = "/var/lib/perc/deploy.lock";
const RESTATE_INGRESS_PORT: u16 = 9080;

#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq, Eq)]
struct Registry {
    #[serde(default)]
    apps: BTreeMap<String, AppEntry>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
struct DbCredentials {
    user: String,
    password: String,
    name: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
struct RestateEntry {
    worker_port: u16,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
struct AppEntry {
    port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    domain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    db: Option<DbCredentials>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    env: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restate: Option<RestateEntry>,
}

fn used_ports(registry: &Registry) -> std::collections::HashSet<u16> {
    let mut used = std::collections::HashSet::new();
    for entry in registry.apps.values() {
        used.insert(entry.port);
        if let Some(ref restate) = entry.restate {
            used.insert(restate.worker_port);
        }
    }
    used
}

fn next_available_port(used: &std::collections::HashSet<u16>) -> u16 {
    let mut port = BASE_PORT;
    while used.contains(&port) {
        port += 1;
    }
    port
}

fn allocate_port(registry: &Registry, app_name: &str) -> u16 {
    if let Some(entry) = registry.apps.get(app_name) {
        return entry.port;
    }
    next_available_port(&used_ports(registry))
}

fn allocate_worker_port(registry: &Registry, app_name: &str, app_port: u16) -> u16 {
    if let Some(entry) = registry.apps.get(app_name)
        && let Some(ref restate) = entry.restate
    {
        return restate.worker_port;
    }
    let mut used = used_ports(registry);
    used.insert(app_port);
    next_available_port(&used)
}

fn is_valid_app_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let first = name.as_bytes()[0];
    if first.is_ascii_digit() {
        return false;
    }
    name.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn is_valid_domain(domain: &str) -> bool {
    if domain.is_empty() || domain.len() > 253 {
        return false;
    }
    domain.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    })
}

fn is_valid_env_key(key: &str) -> bool {
    if key.is_empty() {
        return false;
    }
    let first = key.as_bytes()[0];
    if !first.is_ascii_alphabetic() && first != b'_' {
        return false;
    }
    key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn systemd_escape_env_value(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '$' => escaped.push_str("$$"),
            '%' => escaped.push_str("%%"),
            _ => escaped.push(c),
        }
    }
    escaped
}

fn mask_secret(value: &str) -> String {
    let len = value.len();
    if len <= 8 {
        "*".repeat(len)
    } else {
        let visible = &value[..4];
        format!("{visible}{}", "*".repeat(len - 4))
    }
}

fn generate_caddyfile(registry: &Registry) -> String {
    let mut blocks = Vec::new();
    let mut domainless: Vec<(&str, u16)> = Vec::new();

    for (name, entry) in &registry.apps {
        if let Some(domain) = &entry.domain {
            blocks.push(format!(
                "{domain} {{\n\treverse_proxy localhost:{}\n}}",
                entry.port
            ));
        } else {
            domainless.push((name, entry.port));
        }
    }

    if domainless.len() == 1 {
        let (_, port) = domainless[0];
        blocks.insert(0, format!(":80 {{\n\treverse_proxy localhost:{port}\n}}"));
    }

    if blocks.is_empty() {
        String::new()
    } else {
        blocks.join("\n\n") + "\n"
    }
}

pub async fn run_init(output: &Output, host: &str) -> color_eyre::Result<()> {
    let authkey = match config::resolve_tailscale_authkey() {
        Ok(key) => key,
        Err(e) => {
            output.error("missing_authkey", &format!("{e}"));
            process::exit(1);
        }
    };

    let connect_host = if let Some(ts_host) = lookup_tailscale_host(host) {
        output.step(
            "connect",
            &format!("host already initialized — connecting via tailscale ({ts_host})"),
        );
        ts_host
    } else {
        output.step("connect", &format!("connecting to {host}"));
        host.to_string()
    };

    let session = match SessionBuilder::default()
        .known_hosts_check(KnownHosts::Add)
        .connect_timeout(Duration::from_secs(30))
        .connect(format!("root@{connect_host}"))
        .await
    {
        Ok(s) => s,
        Err(e) => {
            output.error(
                "ssh_connect",
                &format!("failed to connect to {connect_host}: {e}"),
            );
            process::exit(1);
        }
    };

    output.step("update", "updating system packages");
    ssh_run(
        &session,
        "system update",
        "export DEBIAN_FRONTEND=noninteractive && apt-get update -y && apt-get upgrade -y && apt-get install -y unattended-upgrades",
    )
    .await?;

    let rebooted_session = reboot_if_required(output, &session, &connect_host).await?;
    let session = if let Some(s) = rebooted_session {
        drop(session);
        s
    } else {
        session
    };

    output.step("tailscale", "installing tailscale");
    ssh_run(
        &session,
        "install tailscale",
        "curl -fsSL https://tailscale.com/install.sh | sh",
    )
    .await?;

    output.step("tailscale", "joining tailnet");
    ssh_run(
        &session,
        "tailscale up",
        &format!("export TS_AUTH_KEY={authkey} && tailscale up --ssh --auth-key=$TS_AUTH_KEY"),
    )
    .await?;

    let ts_status = ssh_run(&session, "tailscale status", "tailscale status --json").await?;
    let (ts_hostname, ts_ip) = parse_tailscale_status(&ts_status)?;

    output.step("podman", "installing podman");
    install_podman(&session).await?;

    let podman_version = ssh_run(&session, "podman version", "podman --version").await?;
    let podman_version = podman_version.trim().to_string();

    output.step("caddy", "installing caddy");
    install_caddy(&session).await?;

    output.step("lockdown", "securing SSH and configuring firewall");
    lockdown_ssh(&session).await?;

    output.step("user", "creating perc deploy user");
    create_perc_user(&session).await?;

    // The session may have been killed by UFW — close gracefully
    let _ = session.close().await;

    output.step(
        "verify",
        &format!("verifying tailscale connectivity to {ts_hostname}"),
    );
    let ts_session = connect_via_tailscale(&ts_hostname).await?;
    ssh_run(&ts_session, "verify", "tailscale status --self").await?;
    let _ = ts_session.close().await;

    let target_recorded = record_target(host, &ts_hostname, &ts_ip);
    if let Err(e) = &target_recorded {
        output.step(
            "warning",
            &format!("could not record target in perc.toml: {e}"),
        );
    }

    output.success(&InitResult {
        tailscale_hostname: ts_hostname,
        tailscale_ip: ts_ip,
        podman_version,
        target_recorded: target_recorded.is_ok(),
    });

    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "orchestration function with sequential deploy steps"
)]
pub async fn run_push(output: &Output, target: &str, force: bool) -> color_eyre::Result<()> {
    let project = read_project_config(output);
    let app_name = &project.app_name;
    let host = resolve_target(output, target, &project.targets);
    let domain = read_target_domain(target, &project.targets);
    if let Some(ref d) = domain
        && !is_valid_domain(d)
    {
        output.error(
            "invalid_domain",
            &format!("{d:?} is not a valid domain name — fix the domain in perc.toml"),
        );
        process::exit(1);
    }

    check_tool("cargo-zigbuild", &["cargo-zigbuild", "--version"])?;
    check_no_openssl_dep(output);

    output.step("build", &format!("cross-compiling {app_name} for linux"));
    let status = std::process::Command::new("cargo")
        .args([
            "zigbuild",
            "--release",
            "--target",
            "x86_64-unknown-linux-musl",
        ])
        .status()
        .wrap_err("failed to run cargo zigbuild")?;
    if !status.success() {
        output.error("build_failed", "cargo zigbuild failed");
        process::exit(1);
    }

    let binary_path = format!("target/x86_64-unknown-linux-musl/release/{app_name}");
    if !Path::new(&binary_path).exists() {
        output.error(
            "binary_not_found",
            &format!("expected binary at {binary_path}"),
        );
        process::exit(1);
    }

    let worker_binary_path = project.restate.as_ref().map(|r| {
        let path = format!("target/x86_64-unknown-linux-musl/release/{}", r.worker);
        if !Path::new(&path).exists() {
            output.error(
                "binary_not_found",
                &format!(
                    "expected worker binary at {path} — ensure Cargo.toml declares [[bin]] name = {:?}",
                    r.worker
                ),
            );
            process::exit(1);
        }
        path
    });

    let image_tag = format!("localhost/{app_name}:latest");
    output.step("image", &format!("building OCI image for {app_name}"));
    let image_archive = build_oci_tarball(&binary_path, &image_tag, &project.include)?;

    let worker_image = worker_binary_path
        .as_ref()
        .map(|wp| {
            let tag = format!("localhost/{app_name}-worker:latest");
            output.step(
                "image",
                &format!("building OCI image for {app_name}-worker"),
            );
            build_oci_tarball(wp, &tag, &project.include).map(|archive| (tag, archive))
        })
        .transpose()?;

    output.step("connect", &format!("connecting to {host}"));
    let session = connect(&host).await?;

    output.step("push", &format!("pushing image to {host}"));
    push_image_archive(&session, &image_archive).await?;
    if let Some((_, ref archive)) = worker_image {
        output.step("push", &format!("pushing worker image to {host}"));
        push_image_archive(&session, archive).await?;
    }

    output.step("lock", "acquiring deploy lock");
    if !try_acquire_deploy_lock(&session, force).await? {
        output.error(
            "deploy_locked",
            "another deploy is already in progress on this server — try again later, or use --force to clear the lock",
        );
        process::exit(1);
    }

    output.step("registry", "reading app registry");
    let mut registry = read_registry(&session).await?;
    let port = allocate_port(&registry, app_name);

    let existing_entry = registry.apps.get(app_name.as_str());
    let existing_db = existing_entry.and_then(|e| e.db.clone());
    let existing_env = existing_entry.map(|e| e.env.clone()).unwrap_or_default();
    let existing_restate = existing_entry.and_then(|e| e.restate.clone());

    let db_creds = if project.database {
        output.step("database", "ensuring PostgreSQL is installed");
        ensure_postgresql(&session).await?;

        output.step("database", &format!("provisioning database for {app_name}"));
        Some(ensure_database(&session, app_name, existing_db.as_ref()).await?)
    } else {
        existing_db
    };

    let restate_entry = if project.restate.is_some() {
        output.step("restate", "ensuring Restate is installed");
        ensure_restate(&session).await?;
        let worker_port = allocate_worker_port(&registry, app_name, port);
        Some(RestateEntry { worker_port })
    } else {
        existing_restate
    };

    let has_restate = restate_entry.is_some();
    let extra_env = build_extra_env(&project.env, &existing_env, db_creds.as_ref(), has_restate);
    let host_network = db_creds.is_some() || has_restate;

    registry.apps.insert(
        app_name.clone(),
        AppEntry {
            port,
            domain: domain.clone(),
            db: db_creds,
            env: existing_env,
            restate: restate_entry.clone(),
        },
    );

    output.step("deploy", &format!("deploying container on port {port}"));
    write_caddyfile(&session, &registry).await?;
    install_app_container(
        &session,
        app_name,
        &image_tag,
        port,
        &extra_env,
        host_network,
    )
    .await?;

    write_registry(&session, &registry).await?;

    output.step("verify", "waiting for app to be reachable");
    verify_app(&session, port).await?;

    if let (Some((worker_tag, _)), Some(restate)) = (&worker_image, &restate_entry) {
        let worker_name = format!("{app_name}-worker");
        output.step(
            "deploy",
            &format!("deploying worker container on port {}", restate.worker_port),
        );
        install_app_container(
            &session,
            &worker_name,
            worker_tag,
            restate.worker_port,
            &extra_env,
            true,
        )
        .await?;

        output.step("restate", "registering worker with Restate");
        register_restate_deployment(&session, restate.worker_port).await?;
    }

    release_deploy_lock(&session).await;
    let _ = session.close().await;

    output.success(&PushResult {
        app_name: app_name.clone(),
        target: target.to_string(),
        image: image_tag,
        port,
    });

    Ok(())
}

pub async fn run_domain(
    output: &Output,
    target: &str,
    domain: &str,
    force: bool,
) -> color_eyre::Result<()> {
    let project = read_project_config(output);
    let app_name = &project.app_name;
    let host = resolve_target(output, target, &project.targets);
    let target_name = resolve_target_name(output, target, &project.targets);

    if !is_valid_domain(domain) {
        output.error(
            "invalid_domain",
            &format!(
                "{domain:?} is not a valid domain name \
                 (use letters, digits, hyphens, dots only)"
            ),
        );
        process::exit(1);
    }

    if let Err(e) = save_domain(&target_name, domain) {
        output.error(
            "config_write",
            &format!("failed to save domain to perc.toml: {e}"),
        );
        process::exit(1);
    }
    output.step(
        "config",
        &format!("saved domain {domain} for target {target_name}"),
    );

    output.step("connect", &format!("connecting to {host}"));
    let session = connect(&host).await?;

    output.step("lock", "acquiring deploy lock");
    if !try_acquire_deploy_lock(&session, force).await? {
        output.error(
            "deploy_locked",
            "another deploy is already in progress on this server — try again later, or use --force to clear the lock",
        );
        process::exit(1);
    }

    output.step("registry", "reading app registry");
    let mut registry = read_registry(&session).await?;

    let Some(entry) = registry.apps.get_mut(app_name.as_str()) else {
        release_deploy_lock(&session).await;
        let _ = session.close().await;
        output.error(
            "not_deployed",
            &format!("{app_name} has not been deployed yet — run `perc deploy push` first"),
        );
        process::exit(1);
    };
    entry.domain = Some(domain.to_string());
    write_registry(&session, &registry).await?;

    output.step("caddy", &format!("configuring Caddy for {domain}"));
    write_caddyfile(&session, &registry).await?;

    release_deploy_lock(&session).await;
    let _ = session.close().await;

    output.success(&DomainResult {
        domain: domain.to_string(),
        target: target_name,
    });

    Ok(())
}

#[derive(Serialize)]
struct AddResult {
    tailscale_hostname: String,
    tailscale_ip: String,
}

pub async fn run_add(output: &Output, host: &str) -> color_eyre::Result<()> {
    output.step("connect", &format!("connecting to {host}"));
    let session = connect(host).await?;

    let ts_status = ssh_run(&session, "tailscale status", "tailscale status --json").await?;
    let (ts_hostname, ts_ip) = parse_tailscale_status(&ts_status)?;

    let _ = session.close().await;

    if let Err(e) = record_target(host, &ts_hostname, &ts_ip) {
        output.error(
            "config_write",
            &format!("failed to record target in perc.toml: {e}"),
        );
        process::exit(1);
    }

    output.success(&AddResult {
        tailscale_hostname: ts_hostname,
        tailscale_ip: ts_ip,
    });

    Ok(())
}

#[derive(Serialize)]
struct StatusResult {
    target: String,
    apps: Vec<StatusApp>,
}

#[derive(Serialize)]
struct StatusApp {
    name: String,
    port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    domain: Option<String>,
    database: bool,
    restate: bool,
}

pub async fn run_status(output: &Output, target: &str) -> color_eyre::Result<()> {
    let project = read_project_config(output);
    let host = resolve_target(output, target, &project.targets);

    output.step("connect", &format!("connecting to {host}"));
    let session = connect(&host).await?;

    let registry = read_registry(&session).await?;
    let _ = session.close().await;

    let apps: Vec<StatusApp> = registry
        .apps
        .into_iter()
        .map(|(name, entry)| StatusApp {
            name,
            port: entry.port,
            domain: entry.domain,
            database: entry.db.is_some(),
            restate: entry.restate.is_some(),
        })
        .collect();

    output.success(&StatusResult {
        target: target.to_string(),
        apps,
    });

    Ok(())
}

#[derive(Serialize)]
struct RemoveResult {
    removed: String,
    target: String,
}

pub async fn run_remove(
    output: &Output,
    target: &str,
    name: Option<&str>,
    force: bool,
) -> color_eyre::Result<()> {
    let project = read_project_config(output);
    let app_name = name.unwrap_or(&project.app_name);
    let host = resolve_target(output, target, &project.targets);

    output.step("connect", &format!("connecting to {host}"));
    let session = connect(&host).await?;

    output.step("lock", "acquiring deploy lock");
    if !try_acquire_deploy_lock(&session, force).await? {
        output.error(
            "deploy_locked",
            "another deploy is already in progress on this server — try again later, or use --force to clear the lock",
        );
        process::exit(1);
    }

    output.step("registry", "reading app registry");
    let mut registry = read_registry(&session).await?;

    let removed = registry.apps.remove(app_name);
    let Some(entry) = removed else {
        release_deploy_lock(&session).await;
        let _ = session.close().await;
        output.error(
            "not_found",
            &format!("app {app_name:?} not found in registry"),
        );
        process::exit(1);
    };

    if let Some(ref db) = entry.db {
        output.step("database", &format!("dropping database {}", db.name));
        drop_database(&session, &db.name, &db.user).await?;
    }

    write_registry(&session, &registry).await?;

    output.step("caddy", "regenerating Caddyfile");
    write_caddyfile(&session, &registry).await?;

    if entry.restate.is_some() {
        let worker_name = format!("{app_name}-worker");
        output.step("remove", &format!("stopping and removing {worker_name}"));
        sudo_ssh_run(
            &session,
            "stop worker",
            &format!("systemctl stop {worker_name} 2>/dev/null || true"),
        )
        .await?;
        sudo_ssh_run(
            &session,
            "remove worker quadlet",
            &format!("rm -f /etc/containers/systemd/{worker_name}.container"),
        )
        .await?;
    }

    output.step("remove", &format!("stopping and removing {app_name}"));
    sudo_ssh_run(
        &session,
        "stop app",
        &format!("systemctl stop {app_name} 2>/dev/null || true"),
    )
    .await?;
    sudo_ssh_run(
        &session,
        "remove quadlet",
        &format!("rm -f /etc/containers/systemd/{app_name}.container"),
    )
    .await?;
    sudo_ssh_run(&session, "reload systemd", "systemctl daemon-reload").await?;

    release_deploy_lock(&session).await;
    let _ = session.close().await;

    output.success(&RemoveResult {
        removed: app_name.to_string(),
        target: target.to_string(),
    });

    Ok(())
}

#[derive(Serialize)]
struct DbResult {
    app_name: String,
    target: String,
    database: String,
    user: String,
}

pub async fn run_db(output: &Output, target: &str, force: bool) -> color_eyre::Result<()> {
    let project = read_project_config(output);
    let app_name = &project.app_name;
    let host = resolve_target(output, target, &project.targets);

    output.step("connect", &format!("connecting to {host}"));
    let session = connect(&host).await?;

    output.step("lock", "acquiring deploy lock");
    if !try_acquire_deploy_lock(&session, force).await? {
        output.error(
            "deploy_locked",
            "another deploy is already in progress on this server — try again later, or use --force to clear the lock",
        );
        process::exit(1);
    }

    output.step("registry", "reading app registry");
    let mut registry = read_registry(&session).await?;

    let Some(entry) = registry.apps.get(app_name.as_str()) else {
        release_deploy_lock(&session).await;
        let _ = session.close().await;
        output.error(
            "not_deployed",
            &format!("{app_name} has not been deployed yet — run `perc deploy push` first"),
        );
        process::exit(1);
    };
    let port = entry.port;
    let domain = entry.domain.clone();
    let existing_db = entry.db.clone();
    let existing_env = entry.env.clone();
    let existing_restate = entry.restate.clone();

    output.step("database", "ensuring PostgreSQL is installed");
    ensure_postgresql(&session).await?;

    output.step("database", &format!("provisioning database for {app_name}"));
    let creds = ensure_database(&session, app_name, existing_db.as_ref()).await?;

    let db_name = creds.name.clone();
    let db_user = creds.user.clone();

    let has_restate = existing_restate.is_some();
    let extra_env = build_extra_env(&project.env, &existing_env, Some(&creds), has_restate);

    registry.apps.insert(
        app_name.clone(),
        AppEntry {
            port,
            domain,
            db: Some(creds),
            env: existing_env,
            restate: existing_restate,
        },
    );

    output.step("deploy", "updating container with DATABASE_URL");
    let image_tag = format!("localhost/{app_name}:latest");
    install_app_container(&session, app_name, &image_tag, port, &extra_env, true).await?;

    write_registry(&session, &registry).await?;

    output.step("verify", "waiting for app to be reachable");
    verify_app(&session, port).await?;

    if !project.database {
        if let Err(e) = add_database_to_perc_toml() {
            output.step(
                "warning",
                &format!("could not add [database] to perc.toml: {e}"),
            );
        } else {
            output.step("config", "added [database] section to perc.toml");
        }
    }

    release_deploy_lock(&session).await;
    let _ = session.close().await;

    output.success(&DbResult {
        app_name: app_name.clone(),
        target: target.to_string(),
        database: db_name,
        user: db_user,
    });

    Ok(())
}

#[derive(Serialize)]
struct SecretSetResult {
    app_name: String,
    target: String,
    set: Vec<String>,
}

pub async fn run_secret_set(
    output: &Output,
    target: &str,
    vars: &[String],
    force: bool,
) -> color_eyre::Result<()> {
    let project = read_project_config(output);
    let app_name = &project.app_name;
    let host = resolve_target(output, target, &project.targets);

    let mut pairs = Vec::new();
    for var in vars {
        let Some((key, value)) = var.split_once('=') else {
            output.error(
                "invalid_format",
                &format!("expected KEY=VALUE, got {var:?}"),
            );
            process::exit(1);
        };
        if key.is_empty() {
            output.error("invalid_format", "key cannot be empty");
            process::exit(1);
        }
        if !is_valid_env_key(key) {
            output.error(
                "invalid_format",
                &format!(
                    "{key:?} is not a valid environment variable name \
                     (use letters, digits, underscores; must start with a letter or underscore)"
                ),
            );
            process::exit(1);
        }
        pairs.push((key.to_string(), value.to_string()));
    }

    output.step("connect", &format!("connecting to {host}"));
    let session = connect(&host).await?;

    output.step("lock", "acquiring deploy lock");
    if !try_acquire_deploy_lock(&session, force).await? {
        output.error(
            "deploy_locked",
            "another deploy is already in progress on this server — try again later, or use --force to clear the lock",
        );
        process::exit(1);
    }

    output.step("registry", "reading app registry");
    let mut registry = read_registry(&session).await?;

    let Some(entry) = registry.apps.get_mut(app_name.as_str()) else {
        release_deploy_lock(&session).await;
        let _ = session.close().await;
        output.error(
            "not_deployed",
            &format!("{app_name} has not been deployed yet — run `perc deploy push` first"),
        );
        process::exit(1);
    };

    let keys_set: Vec<String> = pairs.iter().map(|(k, _)| k.clone()).collect();
    for (key, value) in pairs {
        output.step("secret", &format!("setting {key}"));
        entry.env.insert(key, value);
    }
    let port = entry.port;
    let has_db = entry.db.is_some();
    let has_restate = entry.restate.is_some();
    let restate_entry = entry.restate.clone();
    let extra_env = build_extra_env(&project.env, &entry.env, entry.db.as_ref(), has_restate);

    write_registry(&session, &registry).await?;

    let image_tag = format!("localhost/{app_name}:latest");
    output.step("deploy", "updating container environment");
    install_app_container(
        &session,
        app_name,
        &image_tag,
        port,
        &extra_env,
        has_db || has_restate,
    )
    .await?;

    if let Some(ref restate) = restate_entry {
        let worker_name = format!("{app_name}-worker");
        let worker_tag = format!("localhost/{worker_name}:latest");
        output.step("deploy", "updating worker container environment");
        install_app_container(
            &session,
            &worker_name,
            &worker_tag,
            restate.worker_port,
            &extra_env,
            true,
        )
        .await?;
    }

    output.step("verify", "waiting for app to be reachable");
    verify_app(&session, port).await?;

    release_deploy_lock(&session).await;
    let _ = session.close().await;

    output.success(&SecretSetResult {
        app_name: app_name.clone(),
        target: target.to_string(),
        set: keys_set,
    });

    Ok(())
}

#[derive(Serialize)]
struct SecretUnsetResult {
    app_name: String,
    target: String,
    unset: Vec<String>,
}

pub async fn run_secret_unset(
    output: &Output,
    target: &str,
    keys: &[String],
    force: bool,
) -> color_eyre::Result<()> {
    let project = read_project_config(output);
    let app_name = &project.app_name;
    let host = resolve_target(output, target, &project.targets);

    output.step("connect", &format!("connecting to {host}"));
    let session = connect(&host).await?;

    output.step("lock", "acquiring deploy lock");
    if !try_acquire_deploy_lock(&session, force).await? {
        output.error(
            "deploy_locked",
            "another deploy is already in progress on this server — try again later, or use --force to clear the lock",
        );
        process::exit(1);
    }

    output.step("registry", "reading app registry");
    let mut registry = read_registry(&session).await?;

    let Some(entry) = registry.apps.get_mut(app_name.as_str()) else {
        release_deploy_lock(&session).await;
        let _ = session.close().await;
        output.error(
            "not_deployed",
            &format!("{app_name} has not been deployed yet — run `perc deploy push` first"),
        );
        process::exit(1);
    };

    for key in keys {
        output.step("secret", &format!("unsetting {key}"));
        entry.env.remove(key);
    }
    let port = entry.port;
    let has_db = entry.db.is_some();
    let has_restate = entry.restate.is_some();
    let restate_entry = entry.restate.clone();
    let extra_env = build_extra_env(&project.env, &entry.env, entry.db.as_ref(), has_restate);

    write_registry(&session, &registry).await?;

    let image_tag = format!("localhost/{app_name}:latest");
    output.step("deploy", "updating container environment");
    install_app_container(
        &session,
        app_name,
        &image_tag,
        port,
        &extra_env,
        has_db || has_restate,
    )
    .await?;

    if let Some(ref restate) = restate_entry {
        let worker_name = format!("{app_name}-worker");
        let worker_tag = format!("localhost/{worker_name}:latest");
        output.step("deploy", "updating worker container environment");
        install_app_container(
            &session,
            &worker_name,
            &worker_tag,
            restate.worker_port,
            &extra_env,
            true,
        )
        .await?;
    }

    output.step("verify", "waiting for app to be reachable");
    verify_app(&session, port).await?;

    release_deploy_lock(&session).await;
    let _ = session.close().await;

    output.success(&SecretUnsetResult {
        app_name: app_name.clone(),
        target: target.to_string(),
        unset: keys.to_vec(),
    });

    Ok(())
}

#[derive(Serialize)]
struct SecretListResult {
    app_name: String,
    target: String,
    secrets: BTreeMap<String, String>,
}

pub async fn run_secret_list(
    output: &Output,
    target: &str,
    reveal: bool,
) -> color_eyre::Result<()> {
    let project = read_project_config(output);
    let app_name = &project.app_name;
    let host = resolve_target(output, target, &project.targets);

    output.step("connect", &format!("connecting to {host}"));
    let session = connect(&host).await?;

    let registry = read_registry(&session).await?;
    let _ = session.close().await;

    let Some(entry) = registry.apps.get(app_name.as_str()) else {
        output.error(
            "not_deployed",
            &format!("{app_name} has not been deployed yet — run `perc deploy push` first"),
        );
        process::exit(1);
    };

    let secrets = if reveal {
        entry.env.clone()
    } else {
        entry
            .env
            .iter()
            .map(|(k, v)| (k.clone(), mask_secret(v)))
            .collect()
    };

    output.success(&SecretListResult {
        app_name: app_name.clone(),
        target: target.to_string(),
        secrets,
    });

    Ok(())
}

pub async fn run_logs(
    output: &Output,
    target: &str,
    lines: u32,
    follow: bool,
) -> color_eyre::Result<()> {
    let project = read_project_config(output);
    let app_name = &project.app_name;
    let host = resolve_target(output, target, &project.targets);

    output.step("connect", &format!("connecting to {host}"));
    let session = connect(&host).await?;

    let units: Vec<String> = if project.restate.is_some() {
        vec![app_name.clone(), format!("{app_name}-worker")]
    } else {
        vec![app_name.clone()]
    };
    let unit_args: Vec<String> = units.iter().map(|u| format!("-u {u}")).collect();
    let unit_flags = unit_args.join(" ");

    if follow {
        output.step(
            "logs",
            &format!("streaming logs for {} (Ctrl+C to stop)", units.join(", ")),
        );
        let mut child = session
            .command("bash")
            .arg("-c")
            .arg(format!("journalctl {unit_flags} -f --no-pager"))
            .stdout(openssh::Stdio::piped())
            .stderr(openssh::Stdio::piped())
            .spawn()
            .await
            .wrap_err("failed to start journalctl")?;

        let stdout = child.stdout().take().unwrap();
        let mut reader = tokio::io::BufReader::new(stdout).lines();
        while let Some(line) = reader.next_line().await? {
            output.log_line(&line);
        }

        let _ = child.wait().await;
        let _ = session.close().await;
    } else {
        let cmd = format!("journalctl {unit_flags} -n {lines} --no-pager");
        let log_output = ssh_run(&session, "fetch logs", &cmd).await?;
        let _ = session.close().await;

        output.logs(&log_output);
    }

    Ok(())
}

fn add_database_to_perc_toml() -> eyre::Result<()> {
    let path = Path::new("perc.toml");
    let contents = std::fs::read_to_string(path).wrap_err("failed to read perc.toml")?;
    let mut doc: toml_edit::DocumentMut = contents.parse().wrap_err("failed to parse perc.toml")?;

    if doc.get("database").is_none() {
        doc["database"] = toml_edit::Item::Table(toml_edit::Table::new());
        std::fs::write(path, doc.to_string()).wrap_err("failed to write perc.toml")?;
    }
    Ok(())
}

struct RestateProjectConfig {
    worker: String,
}

struct ProjectConfig {
    app_name: String,
    targets: toml_edit::DocumentMut,
    database: bool,
    restate: Option<RestateProjectConfig>,
    env: BTreeMap<String, String>,
    include: Vec<String>,
}

fn read_project_config(output: &Output) -> ProjectConfig {
    let path = Path::new("perc.toml");
    if !path.exists() {
        output.error(
            "no_project",
            "perc.toml not found — run this from a perc project directory",
        );
        process::exit(1);
    }
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            output.error("config_read", &format!("failed to read perc.toml: {e}"));
            process::exit(1);
        }
    };
    let doc: toml_edit::DocumentMut = match contents.parse() {
        Ok(d) => d,
        Err(e) => {
            output.error("config_parse", &format!("failed to parse perc.toml: {e}"));
            process::exit(1);
        }
    };
    let app_name = doc
        .get("app")
        .and_then(|a| a.get("name"))
        .and_then(toml_edit::Item::as_str)
        .unwrap_or_default()
        .to_string();
    if app_name.is_empty() {
        output.error("config_invalid", "perc.toml missing app.name");
        process::exit(1);
    }
    if !is_valid_app_name(&app_name) {
        output.error(
            "config_invalid",
            &format!(
                "{app_name:?} is not a valid app name \
                 (use alphanumeric, hyphens, underscores; cannot start with a digit)"
            ),
        );
        process::exit(1);
    }
    let database = doc.get("database").is_some();
    let restate = doc.get("restate").map(|r| {
        let worker = r
            .get("worker")
            .and_then(toml_edit::Item::as_str)
            .filter(|s| !s.is_empty())
            .map_or_else(|| format!("{app_name}-worker"), String::from);
        RestateProjectConfig { worker }
    });
    let env: BTreeMap<String, String> = doc
        .get("env")
        .and_then(toml_edit::Item::as_table)
        .map(|t| {
            t.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.to_string(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    for key in env.keys() {
        if !is_valid_env_key(key) {
            output.error(
                "config_invalid",
                &format!(
                    "env key {key:?} is not a valid environment variable name \
                     (use letters, digits, underscores; must start with a letter or underscore)"
                ),
            );
            process::exit(1);
        }
    }
    let include = doc
        .get("app")
        .and_then(|a| a.get("include"))
        .and_then(toml_edit::Item::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    ProjectConfig {
        app_name,
        targets: doc,
        database,
        restate,
        env,
        include,
    }
}

fn resolve_target(output: &Output, target: &str, doc: &toml_edit::DocumentMut) -> String {
    let Some(targets) = doc.get("targets").and_then(toml_edit::Item::as_table) else {
        output.error(
            "no_targets",
            "no targets configured — run `perc deploy init <host>` first",
        );
        process::exit(1);
    };

    if target == "local" {
        if let Some((_, first)) = targets.iter().next()
            && let Some(host) = first.get("host").and_then(toml_edit::Item::as_str)
        {
            return host.to_string();
        }
        output.error(
            "no_targets",
            "no targets configured — run `perc deploy init <host>` first",
        );
        process::exit(1);
    }

    let Some(t) = targets.get(target) else {
        output.error("target_not_found", &format!("target {target:?} not found"));
        process::exit(1);
    };
    let Some(h) = t.get("host").and_then(toml_edit::Item::as_str) else {
        output.error(
            "target_invalid",
            &format!("target {target:?} has no host field"),
        );
        process::exit(1);
    };
    h.to_string()
}

fn resolve_target_name(output: &Output, target: &str, doc: &toml_edit::DocumentMut) -> String {
    let Some(targets) = doc.get("targets").and_then(toml_edit::Item::as_table) else {
        output.error(
            "no_targets",
            "no targets configured — run `perc deploy init <host>` first",
        );
        process::exit(1);
    };

    if target == "local" {
        if let Some((name, _)) = targets.iter().next() {
            return name.to_string();
        }
        output.error(
            "no_targets",
            "no targets configured — run `perc deploy init <host>` first",
        );
        process::exit(1);
    }

    if targets.contains_key(target) {
        return target.to_string();
    }

    output.error("target_not_found", &format!("target {target:?} not found"));
    process::exit(1);
}

fn save_domain(target_name: &str, domain: &str) -> eyre::Result<()> {
    let path = Path::new("perc.toml");
    let contents = std::fs::read_to_string(path).wrap_err("failed to read perc.toml")?;
    let mut doc: toml_edit::DocumentMut = contents.parse().wrap_err("failed to parse perc.toml")?;

    let target = doc
        .get_mut("targets")
        .and_then(toml_edit::Item::as_table_mut)
        .and_then(|t| t.get_mut(target_name))
        .and_then(toml_edit::Item::as_table_mut)
        .ok_or_else(|| eyre::eyre!("target {target_name:?} not found in perc.toml"))?;

    target.insert("domain", toml_edit::value(domain));

    std::fs::write(path, doc.to_string()).wrap_err("failed to write perc.toml")?;
    Ok(())
}

fn read_target_domain(target: &str, doc: &toml_edit::DocumentMut) -> Option<String> {
    let targets = doc.get("targets")?.as_table()?;
    let entry = if target == "local" {
        targets.iter().next().map(|(_, v)| v)?
    } else {
        targets.get(target)?
    };
    entry.get("domain")?.as_str().map(String::from)
}

fn check_tool(name: &str, cmd: &[&str]) -> eyre::Result<()> {
    let status = std::process::Command::new(cmd[0])
        .args(&cmd[1..])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => Ok(()),
        _ => {
            eyre::bail!(
                "{name} is required but not found — install it with: cargo install cargo-zigbuild"
            );
        }
    }
}

fn check_no_openssl_dep(output: &Output) {
    let result = std::process::Command::new("cargo")
        .args([
            "tree",
            "--prefix",
            "none",
            "--target",
            "x86_64-unknown-linux-musl",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output();
    let Ok(out) = result else { return };
    if !out.status.success() {
        return;
    }
    let tree = String::from_utf8_lossy(&out.stdout);
    if !tree.contains("openssl-sys") {
        return;
    }
    output.error(
        "openssl_dep",
        "this project depends on openssl-sys, which cannot be cross-compiled for musl\n\n\
         Switch to rustls — most Rust TLS libraries support it:\n  \
         rust-s3:  use features = [\"tokio-rustls-tls\"] (with default-features = false)\n  \
         sqlx:     use feature \"tls-rustls-ring\"\n  \
         reqwest:  use feature \"rustls-tls\"\n\n\
         Or add to [dependencies]: openssl = { version = \"0.10\", features = [\"vendored\"] }",
    );
    process::exit(1);
}

fn build_oci_tarball(binary_path: &str, tag: &str, include: &[String]) -> eyre::Result<Vec<u8>> {
    let binary_data = std::fs::read(binary_path).wrap_err("failed to read binary")?;

    let layer_tar = build_layer_tar(&binary_data, include, Path::new("."))?;
    let layer_hash = hex_sha256(&layer_tar);

    let config = serde_json::json!({
        "architecture": "amd64",
        "os": "linux",
        "config": {
            "Entrypoint": ["/app"],
        },
        "rootfs": {
            "type": "layers",
            "diff_ids": [format!("sha256:{layer_hash}")]
        }
    });
    let config_bytes = serde_json::to_vec(&config).unwrap();
    let config_hash = hex_sha256(&config_bytes);

    let manifest = serde_json::json!([{
        "Config": format!("{config_hash}.json"),
        "RepoTags": [tag],
        "Layers": [format!("{layer_hash}/layer.tar")]
    }]);
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();

    let mut image_archive = Vec::new();
    {
        let mut ar = tar::Builder::new(&mut image_archive);
        add_tar_entry(&mut ar, "manifest.json", &manifest_bytes)?;
        add_tar_entry(&mut ar, &format!("{config_hash}.json"), &config_bytes)?;
        add_tar_dir(&mut ar, &format!("{layer_hash}/"))?;
        add_tar_entry(&mut ar, &format!("{layer_hash}/layer.tar"), &layer_tar)?;
        ar.finish().wrap_err("failed to finalize image tar")?;
    }

    Ok(image_archive)
}

fn build_layer_tar(binary_data: &[u8], include: &[String], base: &Path) -> eyre::Result<Vec<u8>> {
    let mut layer = Vec::new();
    {
        let mut ar = tar::Builder::new(&mut layer);
        let mut header = tar::Header::new_gnu();
        header.set_path("app")?;
        header.set_size(binary_data.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        ar.append(&header, binary_data)?;

        for path_str in include {
            if Path::new(path_str).is_absolute() {
                return Err(eyre::eyre!("include path must be relative: {path_str}"));
            }
            let full_path = base.join(path_str);
            if !full_path.exists() {
                return Err(eyre::eyre!("include path does not exist: {path_str}"));
            }
            let canonical = full_path
                .canonicalize()
                .wrap_err_with(|| format!("failed to resolve include path: {path_str}"))?;
            let canonical_base = base
                .canonicalize()
                .wrap_err("failed to resolve project base directory")?;
            if !canonical.starts_with(&canonical_base) {
                return Err(eyre::eyre!(
                    "include path escapes project directory: {path_str}"
                ));
            }
            if full_path.is_dir() {
                append_dir_recursive(&mut ar, &full_path, path_str)?;
            } else {
                let data = std::fs::read(&full_path)
                    .wrap_err_with(|| format!("failed to read include file: {path_str}"))?;
                let mut h = tar::Header::new_gnu();
                h.set_path(path_str)?;
                h.set_size(data.len() as u64);
                h.set_mode(0o644);
                h.set_cksum();
                ar.append(&h, data.as_slice())?;
            }
        }

        ar.finish()?;
    }
    Ok(layer)
}

fn append_dir_recursive(
    ar: &mut tar::Builder<&mut Vec<u8>>,
    dir: &Path,
    prefix: &str,
) -> eyre::Result<()> {
    let mut h = tar::Header::new_gnu();
    h.set_path(format!("{prefix}/"))?;
    h.set_size(0);
    h.set_mode(0o755);
    h.set_entry_type(tar::EntryType::Directory);
    h.set_cksum();
    ar.append(&h, &[] as &[u8])?;

    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .wrap_err_with(|| format!("failed to read directory: {prefix}"))?
        .collect::<Result<Vec<_>, _>>()
        .wrap_err_with(|| format!("failed to read directory entry in: {prefix}"))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);

    for entry in entries {
        let entry_path = entry.path();
        let name = entry.file_name();
        let tar_path = format!("{prefix}/{}", name.to_string_lossy());

        if entry_path.is_dir() {
            append_dir_recursive(ar, &entry_path, &tar_path)?;
        } else {
            let data = std::fs::read(&entry_path)
                .wrap_err_with(|| format!("failed to read include file: {tar_path}"))?;
            let mut fh = tar::Header::new_gnu();
            fh.set_path(&tar_path)?;
            fh.set_size(data.len() as u64);
            fh.set_mode(0o644);
            fh.set_cksum();
            ar.append(&fh, data.as_slice())?;
        }
    }
    Ok(())
}

fn add_tar_entry(ar: &mut tar::Builder<&mut Vec<u8>>, path: &str, data: &[u8]) -> eyre::Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_path(path)?;
    header.set_size(data.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    ar.append(&header, data)?;
    Ok(())
}

fn add_tar_dir(ar: &mut tar::Builder<&mut Vec<u8>>, path: &str) -> eyre::Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_path(path)?;
    header.set_size(0);
    header.set_mode(0o755);
    header.set_entry_type(tar::EntryType::Directory);
    header.set_cksum();
    ar.append(&header, &[] as &[u8])?;
    Ok(())
}

fn hex_sha256(data: &[u8]) -> String {
    let hash = Sha256::digest(data);
    hash.iter().fold(String::new(), |mut acc, b| {
        use std::fmt::Write;
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

async fn push_image_archive(session: &Session, image_archive: &[u8]) -> eyre::Result<()> {
    let mut child = session
        .command("sudo")
        .arg("podman")
        .arg("load")
        .stdin(openssh::Stdio::piped())
        .stdout(openssh::Stdio::piped())
        .stderr(openssh::Stdio::piped())
        .spawn()
        .await
        .wrap_err("failed to start podman load")?;

    let mut stdin = child.stdin().take().unwrap();
    tokio::io::AsyncWriteExt::write_all(&mut stdin, image_archive)
        .await
        .wrap_err("failed to write image data")?;
    drop(stdin);

    let out = child
        .wait_with_output()
        .await
        .wrap_err("podman load failed")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        eyre::bail!("podman load failed: {stderr}");
    }
    Ok(())
}

async fn install_caddy(session: &Session) -> eyre::Result<()> {
    ssh_run(
        session,
        "install caddy",
        "command -v caddy >/dev/null 2>&1 || \
         (apt-get install -y debian-keyring debian-archive-keyring apt-transport-https curl && \
          curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg && \
          curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' | tee /etc/apt/sources.list.d/caddy-stable.list && \
          apt-get update && apt-get install -y caddy)",
    )
    .await?;

    ssh_run(session, "enable caddy", "systemctl enable caddy").await?;

    Ok(())
}

async fn write_caddyfile(session: &Session, registry: &Registry) -> eyre::Result<()> {
    let caddyfile = generate_caddyfile(registry);
    sudo_ssh_run(session, "create caddy dir", "mkdir -p /etc/caddy").await?;
    sudo_ssh_write_file(
        session,
        "write Caddyfile",
        "/etc/caddy/Caddyfile",
        &caddyfile,
    )
    .await?;
    sudo_ssh_run(session, "reload caddy", "systemctl reload caddy").await?;
    Ok(())
}

fn pg_identifier(app_name: &str) -> String {
    app_name.replace('-', "_")
}

fn database_url(creds: &DbCredentials) -> String {
    let user = utf8_percent_encode(&creds.user, NON_ALPHANUMERIC);
    let password = utf8_percent_encode(&creds.password, NON_ALPHANUMERIC);
    let name = utf8_percent_encode(&creds.name, NON_ALPHANUMERIC);
    format!("postgresql://{user}:{password}@localhost:5432/{name}")
}

fn build_extra_env(
    config_env: &BTreeMap<String, String>,
    registry_env: &BTreeMap<String, String>,
    db_creds: Option<&DbCredentials>,
    restate: bool,
) -> BTreeMap<String, String> {
    let mut env = config_env.clone();
    for (k, v) in registry_env {
        env.insert(k.clone(), v.clone());
    }
    if let Some(creds) = db_creds {
        env.insert("DATABASE_URL".to_string(), database_url(creds));
    }
    if restate {
        env.insert(
            "RESTATE_INGRESS_URL".to_string(),
            format!("http://localhost:{RESTATE_INGRESS_PORT}"),
        );
    }
    env
}

fn pg_tune_conf(total_ram_kb: u64) -> String {
    let pg_budget_kb = total_ram_kb / 4;
    let shared_buffers_mb = (pg_budget_kb / 4 / 1024).max(32);
    let effective_cache_size_mb = (pg_budget_kb * 3 / 4 / 1024).max(64);
    let maintenance_work_mem_mb = (pg_budget_kb / 8 / 1024).clamp(16, 2048);
    let work_mem_kb = (pg_budget_kb / (100 * 4)).max(1024);

    format!(
        "# Auto-tuned by perc ({pg_budget_kb}kB budget = 25% of RAM)\n\
         shared_buffers = {shared_buffers_mb}MB\n\
         effective_cache_size = {effective_cache_size_mb}MB\n\
         maintenance_work_mem = {maintenance_work_mem_mb}MB\n\
         work_mem = {work_mem_kb}kB\n",
    )
}

async fn ensure_postgresql(session: &Session) -> eyre::Result<()> {
    let has_psql = ssh_run(session, "check psql", "command -v psql").await;
    if has_psql.is_err() {
        sudo_ssh_run(session, "apt update", "apt-get update -qq").await?;
        sudo_ssh_run(
            session,
            "install postgresql",
            "apt-get install -y postgresql",
        )
        .await?;
        sudo_ssh_run(session, "enable postgresql", "systemctl enable postgresql").await?;
        sudo_ssh_run(session, "start postgresql", "systemctl start postgresql").await?;
    }

    let meminfo = ssh_run(
        session,
        "detect RAM",
        "awk '/^MemTotal:/ { print $2 }' /proc/meminfo",
    )
    .await?;
    let total_ram_kb: u64 = meminfo
        .trim()
        .parse()
        .wrap_err("failed to parse MemTotal from /proc/meminfo")?;

    let conf = pg_tune_conf(total_ram_kb);
    let conf_dir = ssh_run(
        session,
        "find pg conf.d",
        "find /etc/postgresql -name conf.d -type d | head -1",
    )
    .await?;
    let conf_dir = conf_dir.trim();
    if conf_dir.is_empty() {
        eyre::bail!("could not find PostgreSQL conf.d directory");
    }

    sudo_ssh_write_file(
        session,
        "write pg tuning",
        &format!("{conf_dir}/perc-tune.conf"),
        &conf,
    )
    .await?;

    sudo_ssh_run(session, "reload postgresql", "systemctl reload postgresql").await?;

    Ok(())
}

async fn ensure_database(
    session: &Session,
    app_name: &str,
    existing: Option<&DbCredentials>,
) -> eyre::Result<DbCredentials> {
    if let Some(creds) = existing {
        let db_name = &creds.name;
        let user = &creds.user;
        let password = &creds.password;
        ssh_run(
            session,
            "verify database",
            &format!(
                "sudo -u postgres psql -tc \
                 \"SELECT 1 FROM pg_database WHERE datname = '{db_name}'\" \
                 | grep -q 1 || \
                 sudo -u postgres psql -c \"CREATE DATABASE \\\"{db_name}\\\" OWNER \\\"{user}\\\"\""
            ),
        )
        .await?;
        ssh_run(
            session,
            "sync db password",
            &format!(
                "sudo -u postgres psql -c \"ALTER ROLE \\\"{user}\\\" WITH PASSWORD '{password}'\""
            ),
        )
        .await?;
        return Ok(creds.clone());
    }

    let pg_name = pg_identifier(app_name);
    let password = ssh_run(session, "generate password", "openssl rand -hex 32").await?;
    let password = password.trim().to_string();

    ssh_run(
        session,
        "create db user",
        &format!(
            "sudo -u postgres psql -tc \
             \"SELECT 1 FROM pg_roles WHERE rolname = '{pg_name}'\" \
             | grep -q 1 || \
             sudo -u postgres psql -c \"CREATE USER \\\"{pg_name}\\\"\""
        ),
    )
    .await?;

    ssh_run(
        session,
        "set db password",
        &format!(
            "sudo -u postgres psql -c \"ALTER ROLE \\\"{pg_name}\\\" WITH PASSWORD '{password}'\""
        ),
    )
    .await?;

    ssh_run(
        session,
        "create database",
        &format!(
            "sudo -u postgres psql -tc \
             \"SELECT 1 FROM pg_database WHERE datname = '{pg_name}'\" \
             | grep -q 1 || \
             sudo -u postgres psql -c \"CREATE DATABASE \\\"{pg_name}\\\" OWNER \\\"{pg_name}\\\"\""
        ),
    )
    .await?;

    Ok(DbCredentials {
        user: pg_name.clone(),
        password,
        name: pg_name,
    })
}

async fn drop_database(session: &Session, db_name: &str, db_user: &str) -> eyre::Result<()> {
    ssh_run(
        session,
        "drop database",
        &format!(
            "sudo -u postgres psql -c \"DROP DATABASE IF EXISTS \\\"{db_name}\\\"\" && \
             sudo -u postgres psql -c \"DROP USER IF EXISTS \\\"{db_user}\\\"\""
        ),
    )
    .await?;
    Ok(())
}

async fn ensure_restate(session: &Session) -> eyre::Result<()> {
    let has_restate = ssh_run(session, "check restate", "command -v restate-server").await;
    if has_restate.is_err() {
        let platform = "x86_64-unknown-linux-musl";
        ssh_run(
            session,
            "download restate",
            &format!(
                "cd /tmp && \
                 curl -fSL -o restate-server-{platform}.tar.xz \
                   https://restate.gateway.scarf.sh/latest/restate-server-{platform}.tar.xz && \
                 curl -fSL -o restate-cli-{platform}.tar.xz \
                   https://restate.gateway.scarf.sh/latest/restate-cli-{platform}.tar.xz && \
                 tar -xf restate-server-{platform}.tar.xz --strip-components=1 \
                   restate-server-{platform}/restate-server && \
                 tar -xf restate-cli-{platform}.tar.xz --strip-components=1 \
                   restate-cli-{platform}/restate && \
                 rm -f restate-server-{platform}.tar.xz restate-cli-{platform}.tar.xz"
            ),
        )
        .await?;
        sudo_ssh_run(
            session,
            "install restate binaries",
            "chmod +x /tmp/restate /tmp/restate-server",
        )
        .await?;
        sudo_ssh_run(
            session,
            "move restate binaries",
            "mv /tmp/restate /tmp/restate-server /usr/local/bin/",
        )
        .await?;
    }

    sudo_ssh_run(
        session,
        "create restate data dir",
        "mkdir -p /var/lib/restate",
    )
    .await?;

    let unit = format!(
        "[Unit]\n\
         Description=Restate Server\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart=/usr/local/bin/restate-server\n\
         Environment=RESTATE_INGRESS__BIND_ADDRESS=0.0.0.0:{RESTATE_INGRESS_PORT}\n\
         WorkingDirectory=/var/lib/restate\n\
         Restart=always\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n"
    );

    sudo_ssh_write_file(
        session,
        "write restate unit",
        "/etc/systemd/system/restate.service",
        &unit,
    )
    .await?;

    sudo_ssh_run(session, "reload systemd", "systemctl daemon-reload").await?;
    sudo_ssh_run(session, "enable restate", "systemctl enable --now restate").await?;

    Ok(())
}

async fn register_restate_deployment(session: &Session, worker_port: u16) -> eyre::Result<()> {
    for i in 0..5 {
        if i > 0 {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        let result = ssh_run(
            session,
            "register deployment",
            &format!("restate deployments register --force --yes http://localhost:{worker_port}"),
        )
        .await;
        if result.is_ok() {
            return Ok(());
        }
    }
    eyre::bail!("failed to register worker with Restate after 5 attempts")
}

async fn install_app_container(
    session: &Session,
    app_name: &str,
    image_tag: &str,
    port: u16,
    extra_env: &BTreeMap<String, String>,
    host_network: bool,
) -> eyre::Result<()> {
    let mut env_lines = format!("Environment=PORT={port}\n");
    for (key, value) in extra_env {
        use std::fmt::Write;
        let escaped = systemd_escape_env_value(value);
        let _ = writeln!(env_lines, "Environment=\"{key}={escaped}\"");
    }

    let network_line = if host_network {
        "Network=host\n".to_string()
    } else {
        format!("PublishPort=127.0.0.1:{port}:{port}\n")
    };

    let quadlet = format!(
        "[Unit]\n\
         Description={app_name}\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Container]\n\
         Image={image_tag}\n\
         {network_line}\
         {env_lines}\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    );

    sudo_ssh_run(
        session,
        "create quadlet dir",
        "mkdir -p /etc/containers/systemd",
    )
    .await?;
    sudo_ssh_write_file(
        session,
        "write quadlet",
        &format!("/etc/containers/systemd/{app_name}.container"),
        &quadlet,
    )
    .await?;

    sudo_ssh_run(session, "reload systemd", "systemctl daemon-reload").await?;
    sudo_ssh_run(
        session,
        "start app",
        &format!("systemctl restart {app_name}"),
    )
    .await?;

    Ok(())
}

async fn verify_app(session: &Session, port: u16) -> eyre::Result<()> {
    for i in 0..5 {
        if i > 0 {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        let result = ssh_run(
            session,
            "health check",
            &format!("curl -sf http://127.0.0.1:{port}/ || true"),
        )
        .await;
        if let Ok(body) = result
            && !body.trim().is_empty()
        {
            return Ok(());
        }
    }
    eyre::bail!("app did not respond on port {port} after 10 seconds");
}

async fn connect(host: &str) -> eyre::Result<Session> {
    SessionBuilder::default()
        .known_hosts_check(KnownHosts::Add)
        .connect_timeout(Duration::from_secs(30))
        .connect(format!("perc@{host}"))
        .await
        .wrap_err_with(|| format!("failed to connect to {host}"))
}

async fn read_registry(session: &Session) -> eyre::Result<Registry> {
    let raw = ssh_run(
        session,
        "read registry",
        &format!("cat {REGISTRY_PATH} 2>/dev/null || echo ''"),
    )
    .await?;
    let raw = raw.trim();
    if raw.is_empty() {
        return migrate_from_quadlets(session).await;
    }
    toml_edit::de::from_str(raw).wrap_err("failed to parse VPS registry")
}

async fn migrate_from_quadlets(session: &Session) -> eyre::Result<Registry> {
    let listing = ssh_run(
        session,
        "scan quadlets",
        "ls /etc/containers/systemd/*.container 2>/dev/null || echo ''",
    )
    .await?;
    let mut registry = Registry::default();
    for line in listing.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some(filename) = line.rsplit('/').next() else {
            continue;
        };
        let Some(app_name) = filename.strip_suffix(".container") else {
            continue;
        };
        let content = ssh_run(session, "read quadlet", &format!("cat {line}"))
            .await
            .unwrap_or_default();
        let port = parse_quadlet_port(&content).unwrap_or(BASE_PORT);
        registry.apps.insert(
            app_name.to_string(),
            AppEntry {
                port,
                domain: None,
                db: None,
                env: BTreeMap::new(),
                restate: None,
            },
        );
    }
    if !registry.apps.is_empty() {
        write_registry(session, &registry).await?;
    }
    Ok(registry)
}

fn parse_quadlet_port(content: &str) -> Option<u16> {
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("PublishPort=") {
            let parts: Vec<&str> = rest.split(':').collect();
            if parts.len() >= 2 {
                return parts[1].parse().ok();
            }
        }
    }
    None
}

async fn write_registry(session: &Session, registry: &Registry) -> eyre::Result<()> {
    let content = toml_edit::ser::to_string(registry).wrap_err("failed to serialize registry")?;
    ssh_write_file(
        session,
        "write registry",
        &format!("{REGISTRY_PATH}.tmp"),
        &content,
    )
    .await?;
    ssh_run(
        session,
        "finalize registry",
        &format!("chmod 600 {REGISTRY_PATH}.tmp && mv {REGISTRY_PATH}.tmp {REGISTRY_PATH}"),
    )
    .await?;
    Ok(())
}

async fn try_acquire_deploy_lock(session: &Session, force: bool) -> eyre::Result<bool> {
    if force {
        let _ = ssh_run(session, "clear deploy lock", &format!("rm -rf {LOCK_PATH}")).await;
    }
    let out = session
        .command("bash")
        .arg("-c")
        .arg(format!(
            "if mkdir {LOCK_PATH} 2>/dev/null; then exit 0; fi; \
             if [ -d {LOCK_PATH} ] && \
                find {LOCK_PATH} -maxdepth 0 -mmin +30 2>/dev/null | grep -q .; then \
               rm -rf {LOCK_PATH} && mkdir {LOCK_PATH} 2>/dev/null && exit 0; \
             fi; \
             exit 1"
        ))
        .output()
        .await
        .wrap_err("failed to check deploy lock")?;
    Ok(out.status.success())
}

async fn release_deploy_lock(session: &Session) {
    let _ = ssh_run(
        session,
        "release deploy lock",
        &format!("rm -rf {LOCK_PATH}"),
    )
    .await;
}

async fn reboot_if_required(
    output: &Output,
    session: &Session,
    connect_host: &str,
) -> eyre::Result<Option<Session>> {
    let needs_reboot = ssh_run(
        session,
        "check reboot required",
        "test -f /var/run/reboot-required && echo yes || echo no",
    )
    .await?;

    if needs_reboot.trim() != "yes" {
        return Ok(None);
    }

    output.step("reboot", "rebooting to apply system updates");
    ssh_run(session, "reboot", "reboot &").await.ok();

    tokio::time::sleep(Duration::from_secs(15)).await;

    for attempt in 1..=20 {
        if let Ok(s) = SessionBuilder::default()
            .known_hosts_check(KnownHosts::Add)
            .connect_timeout(Duration::from_secs(10))
            .connect(format!("root@{connect_host}"))
            .await
        {
            return Ok(Some(s));
        }
        output.step(
            "reboot",
            &format!("waiting for VPS to come back (attempt {attempt}/20)"),
        );
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    eyre::bail!("VPS did not come back after reboot");
}

async fn ssh_run(session: &Session, description: &str, cmd: &str) -> eyre::Result<String> {
    let out = session
        .command("bash")
        .arg("-c")
        .arg(cmd)
        .output()
        .await
        .wrap_err_with(|| format!("{description}: failed to execute"))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        eyre::bail!("{description} failed (exit {}): {stderr}", out.status);
    }

    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

async fn ssh_write_file(
    session: &Session,
    description: &str,
    path: &str,
    content: &str,
) -> eyre::Result<()> {
    let mut child = session
        .command("bash")
        .arg("-c")
        .arg(format!("cat > {path}"))
        .stdin(openssh::Stdio::piped())
        .stdout(openssh::Stdio::piped())
        .stderr(openssh::Stdio::piped())
        .spawn()
        .await
        .wrap_err_with(|| format!("{description}: failed to execute"))?;

    let mut stdin = child.stdin().take().unwrap();
    tokio::io::AsyncWriteExt::write_all(&mut stdin, content.as_bytes())
        .await
        .wrap_err_with(|| format!("{description}: failed to write content"))?;
    drop(stdin);

    let out = child
        .wait_with_output()
        .await
        .wrap_err_with(|| format!("{description}: wait failed"))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        eyre::bail!("{description} failed (exit {}): {stderr}", out.status);
    }

    Ok(())
}

async fn sudo_ssh_run(session: &Session, description: &str, cmd: &str) -> eyre::Result<String> {
    ssh_run(session, description, &format!("sudo {cmd}")).await
}

async fn sudo_ssh_write_file(
    session: &Session,
    description: &str,
    path: &str,
    content: &str,
) -> eyre::Result<()> {
    let mut child = session
        .command("bash")
        .arg("-c")
        .arg(format!("sudo tee {path} > /dev/null"))
        .stdin(openssh::Stdio::piped())
        .stdout(openssh::Stdio::piped())
        .stderr(openssh::Stdio::piped())
        .spawn()
        .await
        .wrap_err_with(|| format!("{description}: failed to execute"))?;

    let mut stdin = child.stdin().take().unwrap();
    tokio::io::AsyncWriteExt::write_all(&mut stdin, content.as_bytes())
        .await
        .wrap_err_with(|| format!("{description}: failed to write content"))?;
    drop(stdin);

    let out = child
        .wait_with_output()
        .await
        .wrap_err_with(|| format!("{description}: wait failed"))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        eyre::bail!("{description} failed (exit {}): {stderr}", out.status);
    }

    Ok(())
}

fn sudoers_content() -> &'static str {
    "# perc deployment tool — least-privilege sudoers policy\n\
     Cmnd_Alias PERC_SYSTEMCTL = \\\n\
     \x20   /usr/bin/systemctl daemon-reload, \\\n\
     \x20   /usr/bin/systemctl stop *, \\\n\
     \x20   /usr/bin/systemctl start *, \\\n\
     \x20   /usr/bin/systemctl restart *, \\\n\
     \x20   /usr/bin/systemctl reload *, \\\n\
     \x20   /usr/bin/systemctl enable *\n\
     Cmnd_Alias PERC_PODMAN = /usr/bin/podman load\n\
     Cmnd_Alias PERC_APT = \\\n\
     \x20   /usr/bin/apt-get update, \\\n\
     \x20   /usr/bin/apt-get update *, \\\n\
     \x20   /usr/bin/apt-get install *\n\
     Cmnd_Alias PERC_TEE = \\\n\
     \x20   /usr/bin/tee /etc/caddy/*, \\\n\
     \x20   /usr/bin/tee /etc/containers/systemd/*, \\\n\
     \x20   /usr/bin/tee /etc/systemd/system/*, \\\n\
     \x20   /usr/bin/tee /etc/postgresql/*\n\
     Cmnd_Alias PERC_MKDIR = \\\n\
     \x20   /bin/mkdir -p /etc/caddy, \\\n\
     \x20   /bin/mkdir -p /etc/containers/systemd, \\\n\
     \x20   /bin/mkdir -p /var/lib/restate\n\
     Cmnd_Alias PERC_RM = /bin/rm -f /etc/containers/systemd/*\n\
     Cmnd_Alias PERC_MV = /bin/mv /tmp/restate /tmp/restate-server /usr/local/bin/\n\
     Cmnd_Alias PERC_CHMOD = /bin/chmod +x /tmp/restate /tmp/restate-server\n\
     perc ALL=(ALL) NOPASSWD: PERC_SYSTEMCTL, PERC_PODMAN, PERC_APT, PERC_TEE, PERC_MKDIR, PERC_RM, PERC_MV, PERC_CHMOD\n\
     perc ALL=(postgres) NOPASSWD: /usr/bin/psql\n"
}

fn parse_tailscale_status(json: &str) -> eyre::Result<(String, String)> {
    let v: serde_json::Value =
        serde_json::from_str(json).wrap_err("failed to parse tailscale status JSON")?;

    let dns_name = v
        .pointer("/Self/DNSName")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| eyre::eyre!("tailscale status missing Self.DNSName"))?;

    let hostname = dns_name.trim_end_matches('.');

    let ip = v
        .pointer("/Self/TailscaleIPs/0")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| eyre::eyre!("tailscale status missing Self.TailscaleIPs"))?;

    Ok((hostname.to_string(), ip.to_string()))
}

async fn install_podman(session: &Session) -> eyre::Result<()> {
    ssh_run(
        session,
        "install podman",
        "export DEBIAN_FRONTEND=noninteractive && apt-get install -y podman",
    )
    .await?;

    let version_output = ssh_run(session, "verify podman", "podman --version").await?;
    let version_str = version_output.trim();
    let version_num = version_str.split_whitespace().last().unwrap_or(version_str);
    let (major, minor) = parse_version(version_num);
    if major < 4 || (major == 4 && minor < 4) {
        eyre::bail!(
            "podman {version_num} is too old — version 4.4+ is required (for Quadlet support)"
        );
    }

    Ok(())
}

fn parse_version(version: &str) -> (u32, u32) {
    let mut parts = version.split('.');
    let major = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    (major, minor)
}

async fn lockdown_ssh(session: &Session) -> eyre::Result<()> {
    ssh_run(
        session,
        "disable password auth",
        r"sed -i 's/^#*PasswordAuthentication.*/PasswordAuthentication no/' /etc/ssh/sshd_config 2>/dev/null; \
sed -i 's/^#*ChallengeResponseAuthentication.*/ChallengeResponseAuthentication no/' /etc/ssh/sshd_config 2>/dev/null; \
systemctl reload ssh 2>/dev/null || systemctl reload sshd 2>/dev/null || true",
    )
    .await?;

    ssh_run(
        session,
        "configure firewall",
        r"ufw default deny incoming && \
ufw default allow outgoing && \
ufw allow in on tailscale0 to any port 22 && \
ufw allow 80/tcp && \
ufw allow 443/tcp && \
ufw --force enable",
    )
    .await?;

    Ok(())
}

async fn create_perc_user(session: &Session) -> eyre::Result<()> {
    ssh_run(
        session,
        "create perc user",
        "id -u perc >/dev/null 2>&1 || \
         useradd --system --shell /bin/bash --home-dir /var/lib/perc --create-home perc",
    )
    .await?;

    ssh_run(
        session,
        "add journal group",
        "usermod -aG systemd-journal perc",
    )
    .await?;

    ssh_write_file(
        session,
        "write sudoers",
        "/etc/sudoers.d/perc",
        sudoers_content(),
    )
    .await?;

    ssh_run(
        session,
        "validate sudoers",
        "chmod 440 /etc/sudoers.d/perc && visudo -cf /etc/sudoers.d/perc",
    )
    .await?;

    ssh_run(
        session,
        "set state dir ownership",
        "mkdir -p /var/lib/perc && chown perc:perc /var/lib/perc && chmod 700 /var/lib/perc",
    )
    .await?;

    Ok(())
}

async fn connect_via_tailscale(hostname: &str) -> eyre::Result<Session> {
    let delays = [5, 10, 20];
    let mut last_err = None;

    for (i, delay) in delays.iter().enumerate() {
        tokio::time::sleep(Duration::from_secs(*delay)).await;

        match SessionBuilder::default()
            .known_hosts_check(KnownHosts::Add)
            .connect_timeout(Duration::from_secs(15))
            .connect(format!("perc@{hostname}"))
            .await
        {
            Ok(session) => return Ok(session),
            Err(e) => {
                tracing::info!(attempt = i + 1, "tailscale SSH retry: {e}");
                last_err = Some(e);
            }
        }
    }

    Err(last_err.map_or_else(
        || eyre::eyre!("could not connect via tailscale"),
        |e| eyre::eyre!("could not connect via tailscale after 3 attempts: {e}"),
    ))
}

fn lookup_tailscale_host(host: &str) -> Option<String> {
    let contents = std::fs::read_to_string("perc.toml").ok()?;
    let doc: toml_edit::DocumentMut = contents.parse().ok()?;
    let targets = doc.get("targets")?.as_table()?;
    for (_, target) in targets {
        let table = target.as_table()?;
        if table.get("original_host")?.as_str()? == host {
            return table.get("host")?.as_str().map(String::from);
        }
    }
    None
}

fn record_target(original_host: &str, ts_hostname: &str, ts_ip: &str) -> eyre::Result<()> {
    let path = Path::new("perc.toml");
    let mut doc = if path.exists() {
        let contents = std::fs::read_to_string(path).wrap_err("failed to read perc.toml")?;
        contents
            .parse::<toml_edit::DocumentMut>()
            .wrap_err("failed to parse perc.toml")?
    } else {
        toml_edit::DocumentMut::new()
    };

    let target_name = ts_hostname.split('.').next().unwrap_or(ts_hostname);

    if !doc.contains_key("targets") {
        doc["targets"] = toml_edit::Item::Table(toml_edit::Table::new());
    }

    let targets = doc["targets"]
        .as_table_mut()
        .ok_or_else(|| eyre::eyre!("targets is not a table in perc.toml"))?;

    if !targets.contains_key(target_name) {
        targets[target_name] = toml_edit::Item::Table(toml_edit::Table::new());
    }

    let target = targets[target_name]
        .as_table_mut()
        .ok_or_else(|| eyre::eyre!("target entry is not a table"))?;

    target.insert("host", toml_edit::value(ts_hostname));
    target.insert("ip", toml_edit::value(ts_ip));
    target.insert("original_host", toml_edit::value(original_host));

    std::fs::write(path, doc.to_string()).wrap_err("failed to write perc.toml")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry_with(entries: &[(&str, u16, Option<&str>)]) -> Registry {
        let mut apps = BTreeMap::new();
        for (name, port, domain) in entries {
            apps.insert(
                (*name).to_string(),
                AppEntry {
                    port: *port,
                    domain: domain.map(|d| d.to_string()),
                    db: None,
                    env: BTreeMap::new(),
                    restate: None,
                },
            );
        }
        Registry { apps }
    }

    #[test]
    fn allocate_port_empty_registry() {
        let reg = Registry::default();
        assert_eq!(allocate_port(&reg, "myapp"), 8080);
    }

    #[test]
    fn allocate_port_existing_app_returns_same() {
        let reg = registry_with(&[("myapp", 8080, None)]);
        assert_eq!(allocate_port(&reg, "myapp"), 8080);
    }

    #[test]
    fn allocate_port_next_available() {
        let reg = registry_with(&[("app1", 8080, None)]);
        assert_eq!(allocate_port(&reg, "app2"), 8081);
    }

    #[test]
    fn allocate_port_fills_gaps() {
        let reg = registry_with(&[("app1", 8080, None), ("app3", 8082, None)]);
        assert_eq!(allocate_port(&reg, "app2"), 8081);
    }

    #[test]
    fn allocate_port_skips_multiple_used() {
        let reg = registry_with(&[("a", 8080, None), ("b", 8081, None), ("c", 8082, None)]);
        assert_eq!(allocate_port(&reg, "d"), 8083);
    }

    #[test]
    fn caddyfile_empty_registry() {
        let reg = Registry::default();
        assert_eq!(generate_caddyfile(&reg), "");
    }

    #[test]
    fn caddyfile_single_app_with_domain() {
        let reg = registry_with(&[("myapp", 8080, Some("example.com"))]);
        let cf = generate_caddyfile(&reg);
        assert_eq!(cf, "example.com {\n\treverse_proxy localhost:8080\n}\n");
    }

    #[test]
    fn caddyfile_single_app_no_domain_gets_port_80() {
        let reg = registry_with(&[("myapp", 8080, None)]);
        let cf = generate_caddyfile(&reg);
        assert_eq!(cf, ":80 {\n\treverse_proxy localhost:8080\n}\n");
    }

    #[test]
    fn caddyfile_multiple_apps_with_domains() {
        let reg = registry_with(&[
            ("api", 8081, Some("api.example.com")),
            ("web", 8080, Some("example.com")),
        ]);
        let cf = generate_caddyfile(&reg);
        assert!(cf.contains("api.example.com {\n\treverse_proxy localhost:8081\n}"));
        assert!(cf.contains("example.com {\n\treverse_proxy localhost:8080\n}"));
    }

    #[test]
    fn caddyfile_one_domainless_among_domained() {
        let reg = registry_with(&[("api", 8081, Some("api.example.com")), ("dev", 8082, None)]);
        let cf = generate_caddyfile(&reg);
        assert!(cf.contains(":80 {\n\treverse_proxy localhost:8082\n}"));
        assert!(cf.contains("api.example.com {\n\treverse_proxy localhost:8081\n}"));
    }

    #[test]
    fn caddyfile_multiple_domainless_no_port_80() {
        let reg = registry_with(&[("a", 8080, None), ("b", 8081, None)]);
        let cf = generate_caddyfile(&reg);
        assert_eq!(cf, "");
    }

    #[test]
    fn registry_serde_roundtrip() {
        let reg = registry_with(&[("api", 8081, Some("api.example.com")), ("web", 8080, None)]);
        let serialized = toml_edit::ser::to_string(&reg).unwrap();
        let deserialized: Registry = toml_edit::de::from_str(&serialized).unwrap();
        assert_eq!(reg, deserialized);
    }

    #[test]
    fn registry_deserialize_empty_string() {
        let reg: Registry = toml_edit::de::from_str("").unwrap();
        assert_eq!(reg, Registry::default());
    }

    #[test]
    fn parse_quadlet_port_standard() {
        let content = "[Container]\nImage=localhost/myapp:latest\nPublishPort=127.0.0.1:8080:8080\nEnvironment=PORT=8080\n";
        assert_eq!(parse_quadlet_port(content), Some(8080));
    }

    #[test]
    fn parse_quadlet_port_non_default() {
        let content = "[Container]\nPublishPort=127.0.0.1:8083:8083\n";
        assert_eq!(parse_quadlet_port(content), Some(8083));
    }

    #[test]
    fn parse_quadlet_port_missing() {
        let content = "[Container]\nImage=localhost/myapp:latest\n";
        assert_eq!(parse_quadlet_port(content), None);
    }

    #[test]
    fn pg_identifier_replaces_hyphens() {
        assert_eq!(pg_identifier("my-cool-app"), "my_cool_app");
    }

    #[test]
    fn pg_identifier_no_hyphens_unchanged() {
        assert_eq!(pg_identifier("myapp"), "myapp");
    }

    #[test]
    fn database_url_format() {
        let creds = DbCredentials {
            user: "myapp".to_string(),
            password: "abc123def456".to_string(),
            name: "myapp".to_string(),
        };
        assert_eq!(
            database_url(&creds),
            "postgresql://myapp:abc123def456@localhost:5432/myapp"
        );
    }

    #[test]
    fn registry_serde_roundtrip_with_db() {
        let mut reg = registry_with(&[("api", 8081, Some("api.example.com"))]);
        reg.apps.get_mut("api").unwrap().db = Some(DbCredentials {
            user: "api".to_string(),
            password: "secret123".to_string(),
            name: "api".to_string(),
        });
        let serialized = toml_edit::ser::to_string(&reg).unwrap();
        let deserialized: Registry = toml_edit::de::from_str(&serialized).unwrap();
        assert_eq!(reg, deserialized);
    }

    #[test]
    fn registry_with_db_skips_none() {
        let reg = registry_with(&[("web", 8080, None)]);
        let serialized = toml_edit::ser::to_string(&reg).unwrap();
        assert!(!serialized.contains("db"));
    }

    #[test]
    fn pg_tune_4gb_vps() {
        let total_kb = 4 * 1024 * 1024; // 4GB
        let conf = pg_tune_conf(total_kb);
        assert!(conf.contains("shared_buffers = 256MB"));
        assert!(conf.contains("effective_cache_size = 768MB"));
        assert!(conf.contains("maintenance_work_mem = 128MB"));
    }

    #[test]
    fn pg_tune_1gb_vps() {
        let total_kb = 1024 * 1024; // 1GB
        let conf = pg_tune_conf(total_kb);
        assert!(conf.contains("shared_buffers = 64MB"));
        assert!(conf.contains("effective_cache_size = 192MB"));
    }

    #[test]
    fn pg_tune_16gb_vps() {
        let total_kb = 16 * 1024 * 1024; // 16GB
        let conf = pg_tune_conf(total_kb);
        assert!(conf.contains("shared_buffers = 1024MB"));
        assert!(conf.contains("effective_cache_size = 3072MB"));
        assert!(conf.contains("maintenance_work_mem = 512MB"));
    }

    #[test]
    fn pg_tune_enforces_minimums() {
        let total_kb = 256 * 1024; // 256MB
        let conf = pg_tune_conf(total_kb);
        assert!(conf.contains("shared_buffers = 32MB"));
        assert!(conf.contains("effective_cache_size = 64MB"));
        assert!(conf.contains("maintenance_work_mem = 16MB"));
        assert!(conf.contains("work_mem = 1024kB"));
    }

    #[test]
    fn build_extra_env_merges_config_and_registry() {
        let config: BTreeMap<String, String> =
            [("S3_REGION", "us-east-1"), ("S3_BUCKET", "mybucket")]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
        let registry: BTreeMap<String, String> = [("S3_ACCESS_KEY", "secret123")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let env = build_extra_env(&config, &registry, None, false);
        assert_eq!(env.len(), 3);
        assert_eq!(env["S3_REGION"], "us-east-1");
        assert_eq!(env["S3_BUCKET"], "mybucket");
        assert_eq!(env["S3_ACCESS_KEY"], "secret123");
    }

    #[test]
    fn build_extra_env_registry_overrides_config() {
        let config: BTreeMap<String, String> = [("S3_ENDPOINT", "http://localhost:9000")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let registry: BTreeMap<String, String> = [("S3_ENDPOINT", "https://s3.amazonaws.com")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let env = build_extra_env(&config, &registry, None, false);
        assert_eq!(env["S3_ENDPOINT"], "https://s3.amazonaws.com");
    }

    #[test]
    fn build_extra_env_adds_database_url() {
        let config = BTreeMap::new();
        let registry = BTreeMap::new();
        let creds = DbCredentials {
            user: "myapp".to_string(),
            password: "secret".to_string(),
            name: "myapp".to_string(),
        };
        let env = build_extra_env(&config, &registry, Some(&creds), false);
        assert_eq!(
            env["DATABASE_URL"],
            "postgresql://myapp:secret@localhost:5432/myapp"
        );
    }

    #[test]
    fn build_extra_env_empty_inputs() {
        let env = build_extra_env(&BTreeMap::new(), &BTreeMap::new(), None, false);
        assert!(env.is_empty());
    }

    #[test]
    fn registry_serde_roundtrip_with_env() {
        let mut reg = registry_with(&[("api", 8081, Some("api.example.com"))]);
        reg.apps.get_mut("api").unwrap().env = [
            ("S3_KEY".to_string(), "secret".to_string()),
            ("S3_REGION".to_string(), "us-east-1".to_string()),
        ]
        .into_iter()
        .collect();
        let serialized = toml_edit::ser::to_string(&reg).unwrap();
        let deserialized: Registry = toml_edit::de::from_str(&serialized).unwrap();
        assert_eq!(reg, deserialized);
    }

    #[test]
    fn registry_with_empty_env_omits_field() {
        let reg = registry_with(&[("web", 8080, None)]);
        let serialized = toml_edit::ser::to_string(&reg).unwrap();
        assert!(!serialized.contains("[apps.web.env]"));
    }

    #[test]
    fn allocate_port_skips_worker_ports() {
        let mut reg = registry_with(&[("app1", 8080, None)]);
        reg.apps.get_mut("app1").unwrap().restate = Some(RestateEntry { worker_port: 8081 });
        assert_eq!(allocate_port(&reg, "app2"), 8082);
    }

    #[test]
    fn allocate_worker_port_returns_existing() {
        let mut reg = registry_with(&[("app1", 8080, None)]);
        reg.apps.get_mut("app1").unwrap().restate = Some(RestateEntry { worker_port: 8081 });
        assert_eq!(allocate_worker_port(&reg, "app1", 8080), 8081);
    }

    #[test]
    fn allocate_worker_port_skips_app_port() {
        let reg = registry_with(&[("app1", 8080, None)]);
        assert_eq!(allocate_worker_port(&reg, "app1", 8080), 8081);
    }

    #[test]
    fn allocate_worker_port_skips_all_used() {
        let mut reg = registry_with(&[("app1", 8080, None), ("app2", 8082, None)]);
        reg.apps.get_mut("app1").unwrap().restate = Some(RestateEntry { worker_port: 8081 });
        assert_eq!(allocate_worker_port(&reg, "app2", 8082), 8083);
    }

    #[test]
    fn used_ports_includes_worker_ports() {
        let mut reg = registry_with(&[("app1", 8080, None), ("app2", 8082, None)]);
        reg.apps.get_mut("app1").unwrap().restate = Some(RestateEntry { worker_port: 8081 });
        let ports = used_ports(&reg);
        assert!(ports.contains(&8080));
        assert!(ports.contains(&8081));
        assert!(ports.contains(&8082));
        assert_eq!(ports.len(), 3);
    }

    #[test]
    fn build_extra_env_adds_restate_ingress_url() {
        let env = build_extra_env(&BTreeMap::new(), &BTreeMap::new(), None, true);
        assert_eq!(env.len(), 1);
        assert_eq!(
            env["RESTATE_INGRESS_URL"],
            format!("http://localhost:{RESTATE_INGRESS_PORT}")
        );
    }

    #[test]
    fn build_extra_env_no_restate_url_when_false() {
        let env = build_extra_env(&BTreeMap::new(), &BTreeMap::new(), None, false);
        assert!(!env.contains_key("RESTATE_INGRESS_URL"));
    }

    #[test]
    fn build_extra_env_restate_with_database() {
        let creds = DbCredentials {
            user: "myapp".to_string(),
            password: "secret".to_string(),
            name: "myapp".to_string(),
        };
        let env = build_extra_env(&BTreeMap::new(), &BTreeMap::new(), Some(&creds), true);
        assert_eq!(env.len(), 2);
        assert!(env.contains_key("DATABASE_URL"));
        assert!(env.contains_key("RESTATE_INGRESS_URL"));
    }

    #[test]
    fn registry_serde_roundtrip_with_restate() {
        let mut reg = registry_with(&[("api", 8080, Some("api.example.com"))]);
        reg.apps.get_mut("api").unwrap().restate = Some(RestateEntry { worker_port: 8081 });
        let serialized = toml_edit::ser::to_string(&reg).unwrap();
        let deserialized: Registry = toml_edit::de::from_str(&serialized).unwrap();
        assert_eq!(reg, deserialized);
    }

    #[test]
    fn registry_without_restate_omits_field() {
        let reg = registry_with(&[("web", 8080, None)]);
        let serialized = toml_edit::ser::to_string(&reg).unwrap();
        assert!(!serialized.contains("restate"));
    }

    #[test]
    fn registry_serde_roundtrip_with_all_features() {
        let mut reg = registry_with(&[("api", 8080, Some("api.example.com"))]);
        let entry = reg.apps.get_mut("api").unwrap();
        entry.db = Some(DbCredentials {
            user: "api".to_string(),
            password: "secret".to_string(),
            name: "api".to_string(),
        });
        entry.restate = Some(RestateEntry { worker_port: 8081 });
        entry.env = [("S3_KEY".to_string(), "abc".to_string())]
            .into_iter()
            .collect();
        let serialized = toml_edit::ser::to_string(&reg).unwrap();
        let deserialized: Registry = toml_edit::de::from_str(&serialized).unwrap();
        assert_eq!(reg, deserialized);
    }

    #[test]
    fn build_layer_tar_no_includes() {
        let binary = b"hello";
        let layer = build_layer_tar(binary, &[], Path::new(".")).unwrap();
        let mut ar = tar::Archive::new(layer.as_slice());
        let entries: Vec<_> = ar
            .entries()
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn build_layer_tar_includes_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.txt"), "some config").unwrap();

        let include = vec!["config.txt".to_string()];
        let layer = build_layer_tar(b"binary", &include, dir.path()).unwrap();

        let mut ar = tar::Archive::new(layer.as_slice());
        let names: Vec<_> = ar
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names.len(), 2);
        assert_eq!(names[0], "app");
        assert_eq!(names[1], "config.txt");
    }

    #[test]
    fn build_layer_tar_includes_directory() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("prompts");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("a.md"), "prompt a").unwrap();
        std::fs::write(sub.join("b.md"), "prompt b").unwrap();

        let include = vec!["prompts".to_string()];
        let layer = build_layer_tar(b"binary", &include, dir.path()).unwrap();

        let mut ar = tar::Archive::new(layer.as_slice());
        let names: Vec<_> = ar
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names.len(), 4);
        assert_eq!(names[0], "app");
        assert_eq!(names[1], "prompts/");
        assert_eq!(names[2], "prompts/a.md");
        assert_eq!(names[3], "prompts/b.md");
    }

    #[test]
    fn build_layer_tar_missing_include_errors() {
        let dir = tempfile::tempdir().unwrap();
        let result = build_layer_tar(b"bin", &["nonexistent/path".to_string()], dir.path());
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("include path does not exist")
        );
    }

    #[test]
    fn build_layer_tar_rejects_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let result = build_layer_tar(b"bin", &["/etc/passwd".to_string()], dir.path());
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("include path must be relative")
        );
    }

    #[test]
    fn build_layer_tar_rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().parent().unwrap();
        std::fs::write(parent.join("secret.txt"), "secret").ok();
        let result = build_layer_tar(b"bin", &["../secret.txt".to_string()], dir.path());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("include path escapes project directory")
                || err.contains("include path does not exist")
        );
    }

    #[test]
    fn valid_app_names() {
        assert!(is_valid_app_name("myapp"));
        assert!(is_valid_app_name("my-app"));
        assert!(is_valid_app_name("my_app"));
        assert!(is_valid_app_name("MyApp123"));
        assert!(is_valid_app_name("a"));
    }

    #[test]
    fn invalid_app_names() {
        assert!(!is_valid_app_name(""));
        assert!(!is_valid_app_name("123app"));
        assert!(!is_valid_app_name("my app"));
        assert!(!is_valid_app_name("my;app"));
        assert!(!is_valid_app_name("$(whoami)"));
        assert!(!is_valid_app_name("foo`id`"));
        assert!(!is_valid_app_name("app\nname"));
    }

    #[test]
    fn valid_domains() {
        assert!(is_valid_domain("example.com"));
        assert!(is_valid_domain("sub.example.com"));
        assert!(is_valid_domain("my-site.co.uk"));
        assert!(is_valid_domain("localhost"));
        assert!(is_valid_domain("a.b.c.d"));
    }

    #[test]
    fn invalid_domains() {
        assert!(!is_valid_domain(""));
        assert!(!is_valid_domain("example.com { }"));
        assert!(!is_valid_domain("example.com\n:80"));
        assert!(!is_valid_domain("-example.com"));
        assert!(!is_valid_domain("example-.com"));
        assert!(!is_valid_domain("exam ple.com"));
        assert!(!is_valid_domain("example..com"));
    }

    #[test]
    fn valid_env_keys() {
        assert!(is_valid_env_key("HOME"));
        assert!(is_valid_env_key("DATABASE_URL"));
        assert!(is_valid_env_key("_PRIVATE"));
        assert!(is_valid_env_key("s3Key1"));
    }

    #[test]
    fn invalid_env_keys() {
        assert!(!is_valid_env_key(""));
        assert!(!is_valid_env_key("1BAD"));
        assert!(!is_valid_env_key("KEY=VALUE"));
        assert!(!is_valid_env_key("MY-KEY"));
        assert!(!is_valid_env_key("KEY NAME"));
    }

    #[test]
    fn systemd_escape_basic_values() {
        assert_eq!(systemd_escape_env_value("simple"), "simple");
        assert_eq!(systemd_escape_env_value("hello world"), "hello world");
    }

    #[test]
    fn systemd_escape_special_chars() {
        assert_eq!(systemd_escape_env_value("a\"b"), "a\\\"b");
        assert_eq!(systemd_escape_env_value("a\\b"), "a\\\\b");
        assert_eq!(systemd_escape_env_value("a$b"), "a$$b");
        assert_eq!(systemd_escape_env_value("100%"), "100%%");
        assert_eq!(systemd_escape_env_value("line1\nline2"), "line1\\nline2");
    }

    #[test]
    fn systemd_escape_injection_attempt() {
        let malicious = "foo\n[Service]\nExecStartPost=/bin/sh -c 'curl evil'";
        let escaped = systemd_escape_env_value(malicious);
        assert!(!escaped.contains('\n'));
    }

    #[test]
    fn mask_secret_short_value() {
        assert_eq!(mask_secret("abc"), "***");
        assert_eq!(mask_secret("12345678"), "********");
    }

    #[test]
    fn mask_secret_long_value() {
        assert_eq!(mask_secret("sk-abc123xyz"), "sk-a********");
        assert_eq!(mask_secret("AKIAIOSFODNN7EXAMPLE"), "AKIA****************");
    }

    #[test]
    fn mask_secret_empty() {
        assert_eq!(mask_secret(""), "");
    }

    #[test]
    fn database_url_encodes_special_chars() {
        let creds = DbCredentials {
            user: "my-app".to_string(),
            password: "p@ss:word/123".to_string(),
            name: "my-app".to_string(),
        };
        let url = database_url(&creds);
        assert_eq!(
            url,
            "postgresql://my%2Dapp:p%40ss%3Aword%2F123@localhost:5432/my%2Dapp"
        );
    }

    #[test]
    fn sudoers_least_privilege() {
        let content = sudoers_content();
        assert!(
            !content.contains("/bin/bash"),
            "sudoers must not allow bash"
        );
        assert!(!content.contains("/bin/sh"), "sudoers must not allow sh");
        assert!(
            !content.contains("curl"),
            "curl removed — only used during init as root"
        );
        assert!(
            !content.contains("gpg"),
            "gpg removed — only used during init as root"
        );
        assert!(
            !content.contains("/usr/bin/tar"),
            "tar removed — only used as perc user without sudo"
        );
        assert!(
            !content.contains("ufw"),
            "ufw removed — only used during init as root"
        );
        assert!(content.contains("NOPASSWD:"));
        assert!(content.contains("/usr/bin/systemctl"));
        assert!(content.contains("/usr/bin/podman"));
        assert!(content.contains("(postgres)"));
        assert!(content.contains("/usr/bin/psql"));
        assert!(
            content.contains("Cmnd_Alias"),
            "should use Cmnd_Alias for grouping"
        );
        assert!(
            content.contains("podman load"),
            "podman must be restricted to load"
        );
        assert!(
            content.contains("systemctl daemon-reload"),
            "systemctl must have argument restrictions"
        );
    }
}
