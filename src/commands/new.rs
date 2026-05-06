use std::fs;
use std::path::Path;
use std::process;

use serde::Serialize;

use crate::output::Output;

#[derive(Serialize)]
struct NewOutput {
    name: String,
    path: String,
}

fn is_valid_crate_name(name: &str) -> bool {
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

pub fn run(output: &Output, name: &str) -> color_eyre::Result<()> {
    if !is_valid_crate_name(name) {
        output.error(
            "invalid_name",
            &format!("{name:?} is not a valid crate name (use alphanumeric, hyphens, underscores; cannot start with a digit)"),
        );
        process::exit(1);
    }

    let project_dir = Path::new(name);
    if project_dir.exists() {
        output.error(
            "already_exists",
            &format!("directory {name:?} already exists"),
        );
        process::exit(1);
    }

    output.step("create", &format!("creating project {name}"));

    let src_dir = project_dir.join("src");
    fs::create_dir_all(&src_dir)?;

    fs::write(
        project_dir.join("Cargo.toml"),
        format!(
            r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"

[dependencies]
axum = "0.8"
tokio = {{ version = "1", features = ["full"] }}
"#
        ),
    )?;

    fs::write(
        src_dir.join("main.rs"),
        format!(
            r#"use axum::{{Router, routing::get}};

#[tokio::main]
async fn main() {{
    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".to_string());
    let addr = format!("0.0.0.0:{{port}}");
    let app = Router::new().route("/", get(hello));
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}}

async fn hello() -> &'static str {{
    "Hello world, from {name}"
}}
"#
        ),
    )?;

    fs::write(
        project_dir.join("perc.toml"),
        format!(
            r#"[app]
name = "{name}"
"#
        ),
    )?;

    fs::write(project_dir.join(".gitignore"), "/target\n")?;

    let abs_path = fs::canonicalize(project_dir)?;
    output.success(&NewOutput {
        name: name.to_string(),
        path: abs_path.to_string_lossy().into_owned(),
    });
    Ok(())
}
