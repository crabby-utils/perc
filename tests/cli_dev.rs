use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;

fn perc() -> Command {
    Command::cargo_bin("perc").unwrap()
}

// ── Help / CLI structure ────────────────────────────────────────────────────

#[test]
fn help_shows_dev_subcommand() {
    perc()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("dev"));
}

#[test]
fn help_dev_shows_subcommands() {
    perc()
        .args(["help", "dev"])
        .assert()
        .success()
        .stdout(predicate::str::contains("up"))
        .stdout(predicate::str::contains("stop"))
        .stdout(predicate::str::contains("reset"))
        .stdout(predicate::str::contains("status"));
}

// ── No perc.toml errors ────────────────────────────────────────────────────

#[test]
fn dev_no_perc_toml_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    perc()
        .arg("dev")
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("perc.toml not found"));
}

#[test]
fn dev_up_no_perc_toml_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    perc()
        .args(["dev", "up"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("perc.toml not found"));
}

#[test]
fn dev_stop_no_perc_toml_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    perc()
        .args(["dev", "stop"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("perc.toml not found"));
}

#[test]
fn dev_reset_no_perc_toml_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    perc()
        .args(["dev", "reset"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("perc.toml not found"));
}

#[test]
fn dev_status_no_perc_toml_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    perc()
        .args(["dev", "status"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("perc.toml not found"));
}

#[test]
fn dev_status_json_no_perc_toml_exits_1_with_json() {
    let dir = tempfile::tempdir().unwrap();
    let output = perc()
        .args(["--json", "dev", "status"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    let v: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap();
    assert_eq!(v["code"], "no_project");
}

// ── Missing app.name ────────────────────────────────────────────────────────

#[test]
fn dev_missing_app_name_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "[app]\n").unwrap();
    perc()
        .args(["dev", "status"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("missing app.name"));
}

// ── No services configured ─────────────────────────────────────────────────

#[test]
fn dev_stop_no_services_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "[app]\nname = \"myapp\"\n").unwrap();
    perc()
        .args(["dev", "stop"])
        .current_dir(&dir)
        .assert()
        .success();
}

#[test]
fn dev_reset_no_services_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "[app]\nname = \"myapp\"\n").unwrap();
    perc()
        .args(["dev", "reset"])
        .current_dir(&dir)
        .assert()
        .success();
}

#[test]
fn dev_status_no_services_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "[app]\nname = \"myapp\"\n").unwrap();
    perc()
        .args(["dev", "status"])
        .current_dir(&dir)
        .assert()
        .success();
}

#[test]
fn dev_status_no_services_json_output() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "[app]\nname = \"myapp\"\n").unwrap();
    let output = perc()
        .args(["--json", "dev", "status"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(v["services"], serde_json::json!([]));
}

#[test]
fn dev_stop_no_services_json_output() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "[app]\nname = \"myapp\"\n").unwrap();
    let output = perc()
        .args(["--json", "dev", "stop"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(v["stopped"], serde_json::json!([]));
}

#[test]
fn dev_reset_no_services_json_output() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "[app]\nname = \"myapp\"\n").unwrap();
    let output = perc()
        .args(["--json", "dev", "reset"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(v["removed_containers"], serde_json::json!([]));
    assert_eq!(v["removed_volumes"], serde_json::json!([]));
}

// ── Config parsing (storage section) ────────────────────────────────────────

#[test]
fn dev_parses_storage_section() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("perc.toml"),
        "[app]\nname = \"myapp\"\n\n[storage]\nbucket = \"my-bucket\"\n",
    )
    .unwrap();
    // status with storage configured will try to detect runtime
    // but even if runtime is missing, the config parsing itself should work
    // We test by checking that it doesn't fail with a config parse error
    let output = perc()
        .args(["--json", "dev", "status"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("config_parse"),
        "config should parse successfully"
    );
}

#[test]
fn dev_parses_all_sections() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("perc.toml"),
        "[app]\nname = \"myapp\"\n\n[database]\n\n[storage]\nbucket = \"b\"\n\n[restate]\nworker = \"myapp-worker\"\n",
    )
    .unwrap();
    let output = perc()
        .args(["--json", "dev", "status"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("config_parse"),
        "config should parse successfully"
    );
}

// ── Default subcommand ──────────────────────────────────────────────────────

#[test]
fn dev_default_and_up_produce_same_error() {
    let dir = tempfile::tempdir().unwrap();
    let output_bare = perc().arg("dev").current_dir(&dir).output().unwrap();
    let output_up = perc()
        .args(["dev", "up"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(output_bare.status.code(), output_up.status.code());
    let stderr_bare = String::from_utf8_lossy(&output_bare.stderr);
    let stderr_up = String::from_utf8_lossy(&output_up.stderr);
    assert_eq!(stderr_bare, stderr_up);
}
