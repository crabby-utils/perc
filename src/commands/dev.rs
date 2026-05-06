use std::collections::BTreeMap;
use std::path::Path;
use std::process;
use std::time::Duration;

use color_eyre::eyre::{self, WrapErr};
use serde::Serialize;

use crate::output::Output;

const POSTGRES_PORT: u16 = 5432;
const RUSTFS_S3_PORT: u16 = 9000;
const RUSTFS_CONSOLE_PORT: u16 = 9001;
const RESTATE_INGRESS_PORT: u16 = 8080;
const RESTATE_ADMIN_PORT: u16 = 9070;

const RUSTFS_ACCESS_KEY: &str = "percdev";
const RUSTFS_SECRET_KEY: &str = "percdevsecret";

const POSTGRES_USER: &str = "perc";
const POSTGRES_PASSWORD: &str = "perc";

// ── Config ──────────────────────────────────────────────────────────────────

struct StorageConfig {
    bucket: String,
}

struct RestateDevConfig {
    worker: String,
}

struct DevConfig {
    app_name: String,
    database: bool,
    storage: Option<StorageConfig>,
    restate: Option<RestateDevConfig>,
    env: BTreeMap<String, String>,
    watch: Vec<String>,
}

fn read_dev_config(output: &Output) -> DevConfig {
    let path = Path::new("perc.toml");
    if !path.exists() {
        output.error(
            "no_project",
            "perc.toml not found \u{2014} run this from a perc project directory",
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
    let database = doc.get("database").is_some();
    let storage = doc.get("storage").map(|s| {
        let bucket = s
            .get("bucket")
            .and_then(toml_edit::Item::as_str)
            .filter(|b| !b.is_empty())
            .unwrap_or("dev-bucket")
            .to_string();
        StorageConfig { bucket }
    });
    let restate = doc.get("restate").map(|r| {
        let worker = r
            .get("worker")
            .and_then(toml_edit::Item::as_str)
            .filter(|s| !s.is_empty())
            .map_or_else(|| format!("{app_name}-worker"), String::from);
        RestateDevConfig { worker }
    });
    let env = doc
        .get("env")
        .and_then(toml_edit::Item::as_table)
        .map(|t| {
            t.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.to_string(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let watch = doc
        .get("app")
        .and_then(|a| a.get("watch"))
        .and_then(toml_edit::Item::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    DevConfig {
        app_name,
        database,
        storage,
        restate,
        env,
        watch,
    }
}

fn resolve_watch_paths(output: &Output, config: &DevConfig) -> Vec<std::path::PathBuf> {
    if !config.watch.is_empty() {
        return config.watch.iter().map(std::path::PathBuf::from).collect();
    }
    let src = Path::new("src");
    if src.is_dir() {
        return vec![src.to_path_buf()];
    }
    if let Some(members) = read_workspace_members()
        && !members.is_empty()
    {
        return members;
    }
    output.error(
        "no_watch_paths",
        "no src/ directory found and no [workspace] members in Cargo.toml \u{2014} set app.watch in perc.toml",
    );
    process::exit(1);
}

fn read_workspace_members() -> Option<Vec<std::path::PathBuf>> {
    let contents = std::fs::read_to_string("Cargo.toml").ok()?;
    let doc: toml_edit::DocumentMut = contents.parse().ok()?;
    let members = doc.get("workspace")?.get("members")?.as_array()?;
    let paths: Vec<std::path::PathBuf> = members
        .iter()
        .filter_map(|v| v.as_str())
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_dir())
        .collect();
    Some(paths)
}

fn needs_runtime(config: &DevConfig) -> bool {
    config.database || config.storage.is_some() || config.restate.is_some()
}

// ── Container runtime ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
enum ContainerRuntime {
    Docker,
    Podman,
}

impl ContainerRuntime {
    fn cmd(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Podman => "podman",
        }
    }

    fn host_gateway(self) -> &'static str {
        match self {
            Self::Docker => "host.docker.internal",
            Self::Podman => "host.containers.internal",
        }
    }
}

fn detect_runtime() -> Option<ContainerRuntime> {
    if std::process::Command::new("docker")
        .arg("info")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
    {
        return Some(ContainerRuntime::Docker);
    }
    if std::process::Command::new("podman")
        .arg("info")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
    {
        return Some(ContainerRuntime::Podman);
    }
    None
}

fn require_runtime(output: &Output) -> ContainerRuntime {
    if let Some(r) = detect_runtime() {
        r
    } else {
        output.error(
            "no_runtime",
            "docker or podman is required but neither was found (or the daemon is not running)",
        );
        process::exit(1);
    }
}

// ── Container lifecycle ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContainerState {
    Running,
    Stopped,
    Absent,
}

impl ContainerState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Absent => "absent",
        }
    }
}

async fn inspect_container(runtime: ContainerRuntime, name: &str) -> ContainerState {
    let output = tokio::process::Command::new(runtime.cmd())
        .args(["inspect", "--format", "{{.State.Running}}", name])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .await;
    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            if stdout.trim() == "true" {
                ContainerState::Running
            } else {
                ContainerState::Stopped
            }
        }
        _ => ContainerState::Absent,
    }
}

async fn ensure_container(
    output: &Output,
    runtime: ContainerRuntime,
    name: &str,
    run_args: &[String],
) -> color_eyre::Result<()> {
    match inspect_container(runtime, name).await {
        ContainerState::Running => {
            output.step("skip", &format!("{name} already running"));
            return Ok(());
        }
        ContainerState::Stopped => {
            output.step("start", &format!("starting {name}"));
            let status = tokio::process::Command::new(runtime.cmd())
                .args(["start", name])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .status()
                .await
                .wrap_err_with(|| format!("failed to start container {name}"))?;
            if !status.success() {
                eyre::bail!("failed to start container {name}");
            }
            return Ok(());
        }
        ContainerState::Absent => {
            output.step("create", &format!("creating {name}"));
        }
    }
    let mut args: Vec<&str> = vec!["run"];
    let str_args: Vec<&str> = run_args.iter().map(String::as_str).collect();
    args.extend_from_slice(&str_args);
    let cmd_output = tokio::process::Command::new(runtime.cmd())
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .wrap_err_with(|| format!("failed to create container {name}"))?;
    if !cmd_output.status.success() {
        let stderr = String::from_utf8_lossy(&cmd_output.stderr);
        eyre::bail!("failed to create container {name}: {stderr}");
    }
    Ok(())
}

// ── Service definitions ─────────────────────────────────────────────────────

fn container_name(app_name: &str, service: &str) -> String {
    format!("perc-{app_name}-{service}")
}

fn volume_name(app_name: &str, service: &str) -> String {
    format!("perc-{app_name}-{service}")
}

fn postgres_run_args(app_name: &str) -> Vec<String> {
    let name = container_name(app_name, "postgres");
    let volume = volume_name(app_name, "postgres");
    vec![
        "-d".into(),
        "--name".into(),
        name,
        "-v".into(),
        format!("{volume}:/var/lib/postgresql/data"),
        "-e".into(),
        format!("POSTGRES_USER={POSTGRES_USER}"),
        "-e".into(),
        format!("POSTGRES_PASSWORD={POSTGRES_PASSWORD}"),
        "-p".into(),
        format!("{POSTGRES_PORT}:{POSTGRES_PORT}"),
        "postgres:18".into(),
    ]
}

fn rustfs_run_args(app_name: &str) -> Vec<String> {
    let name = container_name(app_name, "storage");
    let volume = volume_name(app_name, "storage");
    vec![
        "-d".into(),
        "--name".into(),
        name,
        "-v".into(),
        format!("{volume}:/data"),
        "-e".into(),
        format!("RUSTFS_ACCESS_KEY={RUSTFS_ACCESS_KEY}"),
        "-e".into(),
        format!("RUSTFS_SECRET_KEY={RUSTFS_SECRET_KEY}"),
        "-p".into(),
        format!("{RUSTFS_S3_PORT}:{RUSTFS_S3_PORT}"),
        "-p".into(),
        format!("{RUSTFS_CONSOLE_PORT}:{RUSTFS_CONSOLE_PORT}"),
        "rustfs/rustfs:latest".into(),
        "server".into(),
        "/data".into(),
        "--console-address".into(),
        ":9001".into(),
    ]
}

fn restate_run_args(app_name: &str) -> Vec<String> {
    let name = container_name(app_name, "restate");
    let volume = volume_name(app_name, "restate");
    vec![
        "-d".into(),
        "--name".into(),
        name,
        "-v".into(),
        format!("{volume}:/restate-data"),
        "-p".into(),
        format!("{RESTATE_INGRESS_PORT}:{RESTATE_INGRESS_PORT}"),
        "-p".into(),
        format!("{RESTATE_ADMIN_PORT}:{RESTATE_ADMIN_PORT}"),
        "docker.restate.dev/restatedev/restate:latest".into(),
    ]
}

fn service_container_names(config: &DevConfig) -> Vec<String> {
    let mut names = Vec::new();
    if config.database {
        names.push(container_name(&config.app_name, "postgres"));
    }
    if config.storage.is_some() {
        names.push(container_name(&config.app_name, "storage"));
    }
    if config.restate.is_some() {
        names.push(container_name(&config.app_name, "restate"));
    }
    names
}

fn service_volume_names(config: &DevConfig) -> Vec<String> {
    let mut names = Vec::new();
    if config.database {
        names.push(volume_name(&config.app_name, "postgres"));
    }
    if config.storage.is_some() {
        names.push(volume_name(&config.app_name, "storage"));
    }
    if config.restate.is_some() {
        names.push(volume_name(&config.app_name, "restate"));
    }
    names
}

// ── Health checks ───────────────────────────────────────────────────────────

async fn wait_for_postgres(runtime: ContainerRuntime, name: &str) -> color_eyre::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let result = tokio::process::Command::new(runtime.cmd())
            .args(["exec", name, "pg_isready", "-U", POSTGRES_USER])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
        if result.is_ok_and(|s| s.success()) {
            return Ok(());
        }
        if tokio::time::Instant::now() > deadline {
            eyre::bail!("postgres did not become ready within 30 seconds");
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn wait_for_tcp(addr: &str) -> color_eyre::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            return Ok(());
        }
        if tokio::time::Instant::now() > deadline {
            eyre::bail!("service at {addr} did not become ready within 30 seconds");
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

// ── Post-start setup ────────────────────────────────────────────────────────

async fn create_database(
    runtime: ContainerRuntime,
    container_name: &str,
    db_name: &str,
) -> color_eyre::Result<()> {
    let result = tokio::process::Command::new(runtime.cmd())
        .args([
            "exec",
            container_name,
            "psql",
            "-U",
            POSTGRES_USER,
            "-tc",
            &format!("SELECT 1 FROM pg_database WHERE datname = '{db_name}'"),
        ])
        .output()
        .await
        .wrap_err("failed to check database existence")?;
    let stdout = String::from_utf8_lossy(&result.stdout);
    if !stdout.contains('1') {
        let status = tokio::process::Command::new(runtime.cmd())
            .args([
                "exec",
                container_name,
                "createdb",
                "-U",
                POSTGRES_USER,
                db_name,
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .status()
            .await
            .wrap_err("failed to create database")?;
        if !status.success() {
            eyre::bail!("failed to create database {db_name}");
        }
    }
    Ok(())
}

async fn create_s3_bucket(runtime: ContainerRuntime, bucket: &str) -> color_eyre::Result<()> {
    let cmd = format!(
        "mc alias set local http://localhost:{RUSTFS_S3_PORT} {RUSTFS_ACCESS_KEY} {RUSTFS_SECRET_KEY} && \
         mc mb --ignore-existing local/{bucket}"
    );
    let result = tokio::process::Command::new(runtime.cmd())
        .args([
            "run",
            "--rm",
            "--network=host",
            "--entrypoint",
            "sh",
            "minio/mc:latest",
            "-c",
            &cmd,
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .wrap_err("failed to create S3 bucket")?;
    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        eyre::bail!("failed to create S3 bucket {bucket}: {stderr}");
    }
    Ok(())
}

async fn register_restate_worker(
    runtime: ContainerRuntime,
    worker_port: u16,
) -> color_eyre::Result<()> {
    let host = runtime.host_gateway();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let result = tokio::process::Command::new("curl")
            .args([
                "-sf",
                "-X",
                "POST",
                &format!("http://localhost:{RESTATE_ADMIN_PORT}/deployments"),
                "-H",
                "Content-Type: application/json",
                "-d",
                &format!(r#"{{"uri":"http://{host}:{worker_port}","force":true}}"#),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
        if result.is_ok_and(|s| s.success()) {
            return Ok(());
        }
        if tokio::time::Instant::now() > deadline {
            eyre::bail!("failed to register worker with Restate after 30 seconds");
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

// ── Port allocation ─────────────────────────────────────────────────────────

fn find_available_port() -> color_eyre::Result<u16> {
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").wrap_err("failed to find an available port")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

// ── Environment ─────────────────────────────────────────────────────────────

fn build_dev_env(
    config: &DevConfig,
    app_port: u16,
    worker_port: Option<u16>,
) -> BTreeMap<String, String> {
    let mut env = config.env.clone();
    env.insert("PORT".into(), app_port.to_string());
    if config.database {
        let db_name = config.app_name.replace('-', "_");
        env.insert(
            "DATABASE_URL".into(),
            format!("postgresql://{POSTGRES_USER}:{POSTGRES_PASSWORD}@localhost:{POSTGRES_PORT}/{db_name}"),
        );
    }
    if let Some(ref storage) = config.storage {
        env.insert(
            "S3_ENDPOINT".into(),
            format!("http://localhost:{RUSTFS_S3_PORT}"),
        );
        env.insert("S3_ACCESS_KEY".into(), RUSTFS_ACCESS_KEY.into());
        env.insert("S3_SECRET_KEY".into(), RUSTFS_SECRET_KEY.into());
        env.insert("S3_BUCKET".into(), storage.bucket.clone());
    }
    if config.restate.is_some() {
        env.insert(
            "RESTATE_INGRESS_URL".into(),
            format!("http://localhost:{RESTATE_INGRESS_PORT}"),
        );
    }
    if let Some(wp) = worker_port {
        env.insert("WORKER_PORT".into(), wp.to_string());
    }
    env
}

// ── Process management ──────────────────────────────────────────────────────

fn spawn_cargo(
    bin_name: &str,
    env_vars: &BTreeMap<String, String>,
) -> color_eyre::Result<std::process::Child> {
    let mut cmd = std::process::Command::new("cargo");
    cmd.args(["run", "--bin", bin_name]);
    for (k, v) in env_vars {
        cmd.env(k, v);
    }
    cmd.stdout(std::process::Stdio::inherit());
    cmd.stderr(std::process::Stdio::inherit());
    let child = cmd
        .spawn()
        .wrap_err_with(|| format!("failed to start cargo run --bin {bin_name}"))?;
    Ok(child)
}

fn kill_child(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

// ── File watching ───────────────────────────────────────────────────────────

async fn debounced_recv(rx: &mut tokio::sync::mpsc::Receiver<()>) {
    let _ = rx.recv().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    while rx.try_recv().is_ok() {}
}

// ── Output types ────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct DevStopResult {
    stopped: Vec<String>,
}

#[derive(Serialize)]
struct DevResetResult {
    removed_containers: Vec<String>,
    removed_volumes: Vec<String>,
}

#[derive(Serialize)]
struct ServiceStatus {
    name: String,
    state: String,
    ports: Vec<String>,
}

#[derive(Serialize)]
struct DevStatusResult {
    services: Vec<ServiceStatus>,
}

// ── Commands ────────────────────────────────────────────────────────────────

#[expect(
    clippy::too_many_lines,
    reason = "orchestration function with sequential steps"
)]
pub async fn run_up(output: &Output) -> color_eyre::Result<()> {
    let config = read_dev_config(output);

    let runtime = if needs_runtime(&config) {
        Some(require_runtime(output))
    } else {
        None
    };

    // Start service containers
    let mut started_services: Vec<String> = Vec::new();

    if config.database {
        let rt = runtime.expect("runtime required for database");
        let name = container_name(&config.app_name, "postgres");
        let args = postgres_run_args(&config.app_name);
        ensure_container(output, rt, &name, &args).await?;
        output.step("health", "waiting for postgres");
        wait_for_postgres(rt, &name).await?;
        let db_name = config.app_name.replace('-', "_");
        output.step("setup", &format!("ensuring database {db_name} exists"));
        create_database(rt, &name, &db_name).await?;
        started_services.push("postgres".into());
    }

    if config.storage.is_some() {
        let rt = runtime.expect("runtime required for storage");
        let name = container_name(&config.app_name, "storage");
        let args = rustfs_run_args(&config.app_name);
        ensure_container(output, rt, &name, &args).await?;
        output.step("health", "waiting for storage");
        wait_for_tcp(&format!("127.0.0.1:{RUSTFS_S3_PORT}")).await?;
    }

    if config.restate.is_some() {
        let rt = runtime.expect("runtime required for restate");
        let name = container_name(&config.app_name, "restate");
        let args = restate_run_args(&config.app_name);
        ensure_container(output, rt, &name, &args).await?;
        output.step("health", "waiting for restate");
        wait_for_tcp(&format!("127.0.0.1:{RESTATE_ADMIN_PORT}")).await?;
        started_services.push("restate".into());
    }

    // Create S3 bucket after storage is healthy
    if let Some(ref storage) = config.storage {
        let rt = runtime.expect("runtime required for storage");
        output.step(
            "setup",
            &format!("ensuring bucket {} exists", storage.bucket),
        );
        create_s3_bucket(rt, &storage.bucket).await?;
        started_services.push("storage".into());
    }

    // Allocate ports
    let app_port = find_available_port()?;
    let worker_port = if config.restate.is_some() {
        Some(find_available_port()?)
    } else {
        None
    };

    // Build env vars
    let app_env = build_dev_env(&config, app_port, worker_port);
    let worker_env = worker_port.map(|wp| {
        let mut env = app_env.clone();
        env.insert("PORT".into(), wp.to_string());
        env
    });

    // Print service URLs
    eprintln!();
    eprintln!("  app:              http://localhost:{app_port}");
    if let Some(wp) = worker_port {
        eprintln!("  worker:           http://localhost:{wp}");
    }
    if config.database {
        eprintln!("  postgres:         localhost:{POSTGRES_PORT}");
    }
    if config.storage.is_some() {
        eprintln!("  storage (S3):     http://localhost:{RUSTFS_S3_PORT}");
        eprintln!("  storage console:  http://localhost:{RUSTFS_CONSOLE_PORT}");
    }
    if config.restate.is_some() {
        eprintln!("  restate ingress:  http://localhost:{RESTATE_INGRESS_PORT}");
        eprintln!("  restate admin:    http://localhost:{RESTATE_ADMIN_PORT}");
    }
    eprintln!();

    // Set up file watcher
    let watch_paths = resolve_watch_paths(output, &config);
    let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(16);
    let mut watcher = notify::recommended_watcher(move |res: Result<notify::Event, _>| {
        if let Ok(event) = res
            && (event.kind.is_modify() || event.kind.is_create() || event.kind.is_remove())
        {
            let _ = tx.blocking_send(());
        }
    })
    .wrap_err("failed to create file watcher")?;
    for path in &watch_paths {
        notify::Watcher::watch(&mut watcher, path, notify::RecursiveMode::Recursive)
            .wrap_err_with(|| format!("failed to watch {}", path.display()))?;
    }
    let watch_display: Vec<_> = watch_paths
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    output.step(
        "watch",
        &format!("watching {} for changes", watch_display.join(", ")),
    );

    // Start initial processes
    output.step(
        "cargo",
        &format!("running {} on port {app_port}", config.app_name),
    );
    let mut app_child = spawn_cargo(&config.app_name, &app_env)?;

    let mut worker_child = if let Some(ref restate_cfg) = config.restate {
        let wp = worker_port.expect("worker port allocated");
        output.step(
            "cargo",
            &format!("running {} on port {wp}", restate_cfg.worker),
        );
        let wenv = worker_env.as_ref().expect("worker env built");
        let child = spawn_cargo(&restate_cfg.worker, wenv)?;
        let rt = runtime.expect("runtime required for restate");
        output.step("restate", "registering worker deployment");
        register_restate_worker(rt, wp).await?;
        Some(child)
    } else {
        None
    };

    // Main event loop
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                eprintln!();
                output.step("shutdown", "stopping app processes");
                kill_child(&mut app_child);
                if let Some(ref mut w) = worker_child {
                    kill_child(w);
                }
                output.step("shutdown", "containers left running \u{2014} use `perc dev stop` to stop them");
                return Ok(());
            }
            () = debounced_recv(&mut rx) => {
                output.step("reload", "file change detected, restarting");
                kill_child(&mut app_child);
                if let Some(ref mut w) = worker_child {
                    kill_child(w);
                }
                app_child = spawn_cargo(&config.app_name, &app_env)?;
                if let Some(ref restate_cfg) = config.restate {
                    let rt = runtime.expect("runtime required for restate");
                    let wp = worker_port.expect("worker port allocated");
                    let wenv = worker_env.as_ref().expect("worker env built");
                    worker_child = Some(spawn_cargo(&restate_cfg.worker, wenv)?);
                    output.step("restate", "re-registering worker deployment");
                    register_restate_worker(rt, wp).await?;
                }
            }
        }
    }
}

pub async fn run_stop(output: &Output) -> color_eyre::Result<()> {
    let config = read_dev_config(output);
    if !needs_runtime(&config) {
        output.success(&DevStopResult {
            stopped: Vec::new(),
        });
        return Ok(());
    }
    let runtime = require_runtime(output);
    let containers = service_container_names(&config);
    let mut stopped = Vec::new();

    for name in &containers {
        match inspect_container(runtime, name).await {
            ContainerState::Running => {
                output.step("stop", &format!("stopping {name}"));
                let _ = tokio::process::Command::new(runtime.cmd())
                    .args(["stop", name])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .await;
                stopped.push(name.clone());
            }
            ContainerState::Stopped => {
                output.step("skip", &format!("{name} already stopped"));
            }
            ContainerState::Absent => {
                output.step("skip", &format!("{name} not found"));
            }
        }
    }

    output.success(&DevStopResult { stopped });
    Ok(())
}

pub async fn run_reset(output: &Output) -> color_eyre::Result<()> {
    let config = read_dev_config(output);
    if !needs_runtime(&config) {
        output.success(&DevResetResult {
            removed_containers: Vec::new(),
            removed_volumes: Vec::new(),
        });
        return Ok(());
    }
    let runtime = require_runtime(output);
    let containers = service_container_names(&config);
    let volumes = service_volume_names(&config);

    for name in &containers {
        if inspect_container(runtime, name).await != ContainerState::Absent {
            output.step("remove", &format!("removing container {name}"));
            let _ = tokio::process::Command::new(runtime.cmd())
                .args(["stop", name])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .await;
            let _ = tokio::process::Command::new(runtime.cmd())
                .args(["rm", "-f", name])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .await;
        }
    }
    for vol in &volumes {
        output.step("remove", &format!("removing volume {vol}"));
        let _ = tokio::process::Command::new(runtime.cmd())
            .args(["volume", "rm", "-f", vol])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
    }

    output.success(&DevResetResult {
        removed_containers: containers,
        removed_volumes: volumes,
    });
    Ok(())
}

pub async fn run_status(output: &Output) -> color_eyre::Result<()> {
    let config = read_dev_config(output);
    if !needs_runtime(&config) {
        output.success(&DevStatusResult {
            services: Vec::new(),
        });
        return Ok(());
    }
    let runtime = require_runtime(output);
    let mut services = Vec::new();

    if config.database {
        let name = container_name(&config.app_name, "postgres");
        let state = inspect_container(runtime, &name).await;
        services.push(ServiceStatus {
            name: "postgres".into(),
            state: state.as_str().into(),
            ports: vec![format!("{POSTGRES_PORT}")],
        });
    }
    if config.storage.is_some() {
        let name = container_name(&config.app_name, "storage");
        let state = inspect_container(runtime, &name).await;
        services.push(ServiceStatus {
            name: "storage".into(),
            state: state.as_str().into(),
            ports: vec![
                format!("{RUSTFS_S3_PORT} (S3)"),
                format!("{RUSTFS_CONSOLE_PORT} (console)"),
            ],
        });
    }
    if config.restate.is_some() {
        let name = container_name(&config.app_name, "restate");
        let state = inspect_container(runtime, &name).await;
        services.push(ServiceStatus {
            name: "restate".into(),
            state: state.as_str().into(),
            ports: vec![
                format!("{RESTATE_INGRESS_PORT} (ingress)"),
                format!("{RESTATE_ADMIN_PORT} (admin)"),
            ],
        });
    }

    output.success(&DevStatusResult { services });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_dev_env_includes_port() {
        let config = DevConfig {
            app_name: "myapp".into(),
            database: false,
            storage: None,
            restate: None,
            env: BTreeMap::new(),
            watch: Vec::new(),
        };
        let env = build_dev_env(&config, 3000, None);
        assert_eq!(env["PORT"], "3000");
    }

    #[test]
    fn build_dev_env_includes_database_url() {
        let config = DevConfig {
            app_name: "my-app".into(),
            database: true,
            storage: None,
            restate: None,
            env: BTreeMap::new(),
            watch: Vec::new(),
        };
        let env = build_dev_env(&config, 3000, None);
        assert_eq!(
            env["DATABASE_URL"],
            "postgresql://perc:perc@localhost:5432/my_app"
        );
    }

    #[test]
    fn build_dev_env_includes_storage_vars() {
        let config = DevConfig {
            app_name: "myapp".into(),
            database: false,
            storage: Some(StorageConfig {
                bucket: "test-bucket".into(),
            }),
            restate: None,
            env: BTreeMap::new(),
            watch: Vec::new(),
        };
        let env = build_dev_env(&config, 3000, None);
        assert_eq!(env["S3_ENDPOINT"], "http://localhost:9000");
        assert_eq!(env["S3_ACCESS_KEY"], "percdev");
        assert_eq!(env["S3_SECRET_KEY"], "percdevsecret");
        assert_eq!(env["S3_BUCKET"], "test-bucket");
    }

    #[test]
    fn build_dev_env_includes_restate_url() {
        let config = DevConfig {
            app_name: "myapp".into(),
            database: false,
            storage: None,
            restate: Some(RestateDevConfig {
                worker: "myapp-worker".into(),
            }),
            env: BTreeMap::new(),
            watch: Vec::new(),
        };
        let env = build_dev_env(&config, 3000, Some(3001));
        assert_eq!(env["RESTATE_INGRESS_URL"], "http://localhost:8080");
    }

    #[test]
    fn build_dev_env_includes_user_env() {
        let mut user_env = BTreeMap::new();
        user_env.insert("MY_VAR".into(), "my_val".into());
        let config = DevConfig {
            app_name: "myapp".into(),
            database: false,
            storage: None,
            restate: None,
            env: user_env,
            watch: Vec::new(),
        };
        let env = build_dev_env(&config, 3000, None);
        assert_eq!(env["MY_VAR"], "my_val");
    }

    #[test]
    fn build_dev_env_includes_worker_port() {
        let config = DevConfig {
            app_name: "myapp".into(),
            database: false,
            storage: None,
            restate: Some(RestateDevConfig {
                worker: "myapp-worker".into(),
            }),
            env: BTreeMap::new(),
            watch: Vec::new(),
        };
        let env = build_dev_env(&config, 3000, Some(3001));
        assert_eq!(env["WORKER_PORT"], "3001");
    }

    #[test]
    fn service_container_names_all_services() {
        let config = DevConfig {
            app_name: "myapp".into(),
            database: true,
            storage: Some(StorageConfig { bucket: "b".into() }),
            restate: Some(RestateDevConfig { worker: "w".into() }),
            env: BTreeMap::new(),
            watch: Vec::new(),
        };
        let names = service_container_names(&config);
        assert_eq!(
            names,
            vec![
                "perc-myapp-postgres",
                "perc-myapp-storage",
                "perc-myapp-restate",
            ]
        );
    }

    #[test]
    fn service_container_names_database_only() {
        let config = DevConfig {
            app_name: "myapp".into(),
            database: true,
            storage: None,
            restate: None,
            env: BTreeMap::new(),
            watch: Vec::new(),
        };
        let names = service_container_names(&config);
        assert_eq!(names, vec!["perc-myapp-postgres"]);
    }

    #[test]
    fn service_container_names_no_services() {
        let config = DevConfig {
            app_name: "myapp".into(),
            database: false,
            storage: None,
            restate: None,
            env: BTreeMap::new(),
            watch: Vec::new(),
        };
        let names = service_container_names(&config);
        assert!(names.is_empty());
    }

    #[test]
    fn database_name_replaces_hyphens() {
        let config = DevConfig {
            app_name: "my-cool-app".into(),
            database: true,
            storage: None,
            restate: None,
            env: BTreeMap::new(),
            watch: Vec::new(),
        };
        let env = build_dev_env(&config, 3000, None);
        assert!(env["DATABASE_URL"].contains("my_cool_app"));
    }

    #[test]
    fn volume_names_match_container_names() {
        let config = DevConfig {
            app_name: "myapp".into(),
            database: true,
            storage: Some(StorageConfig { bucket: "b".into() }),
            restate: Some(RestateDevConfig { worker: "w".into() }),
            env: BTreeMap::new(),
            watch: Vec::new(),
        };
        let containers = service_container_names(&config);
        let volumes = service_volume_names(&config);
        assert_eq!(containers, volumes);
    }

    #[test]
    fn find_available_port_succeeds() {
        let port = find_available_port().unwrap();
        assert!(port > 0);
    }
}
