use std::fs;
use std::os::unix::fs::PermissionsExt;

use assert_cmd::Command;
use predicates::prelude::*;

fn perc() -> Command {
    Command::cargo_bin("perc").unwrap()
}

#[test]
fn config_set_creates_file_with_value() {
    let dir = tempfile::tempdir().unwrap();

    perc()
        .args(["config", "set", "tailscale.authkey", "tskey-test-123"])
        .env("PERC_CONFIG_DIR", dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("tailscale.authkey"));

    let contents = fs::read_to_string(dir.path().join("credentials.toml")).unwrap();
    assert!(contents.contains("tskey-test-123"));
}

#[test]
fn config_set_creates_nested_toml_table() {
    let dir = tempfile::tempdir().unwrap();

    perc()
        .args(["config", "set", "tailscale.authkey", "tskey-test-456"])
        .env("PERC_CONFIG_DIR", dir.path())
        .assert()
        .success();

    let contents = fs::read_to_string(dir.path().join("credentials.toml")).unwrap();
    assert!(contents.contains("[tailscale]"));
    assert!(contents.contains(r#"authkey = "tskey-test-456""#));
}

#[test]
fn config_set_file_has_0600_permissions() {
    let dir = tempfile::tempdir().unwrap();

    perc()
        .args(["config", "set", "tailscale.authkey", "tskey-test"])
        .env("PERC_CONFIG_DIR", dir.path())
        .assert()
        .success();

    let metadata = fs::metadata(dir.path().join("credentials.toml")).unwrap();
    let mode = metadata.permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "credentials file should be 0600, got {mode:o}");
}

#[test]
fn config_set_preserves_existing_values() {
    let dir = tempfile::tempdir().unwrap();

    perc()
        .args(["config", "set", "tailscale.authkey", "tskey-first"])
        .env("PERC_CONFIG_DIR", dir.path())
        .assert()
        .success();

    perc()
        .args(["config", "set", "other.secret", "mysecret"])
        .env("PERC_CONFIG_DIR", dir.path())
        .assert()
        .success();

    let contents = fs::read_to_string(dir.path().join("credentials.toml")).unwrap();
    assert!(contents.contains("tskey-first"));
    assert!(contents.contains("mysecret"));
}

#[test]
fn config_get_reads_value_from_file() {
    let dir = tempfile::tempdir().unwrap();

    perc()
        .args(["config", "set", "tailscale.authkey", "tskey-read-test"])
        .env("PERC_CONFIG_DIR", dir.path())
        .assert()
        .success();

    perc()
        .args(["config", "get", "tailscale.authkey"])
        .env("PERC_CONFIG_DIR", dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("tskey-read-test"));
}

#[test]
fn config_get_env_var_overrides_file() {
    let dir = tempfile::tempdir().unwrap();

    perc()
        .args(["config", "set", "tailscale.authkey", "tskey-from-file"])
        .env("PERC_CONFIG_DIR", dir.path())
        .assert()
        .success();

    perc()
        .args(["config", "get", "tailscale.authkey"])
        .env("PERC_CONFIG_DIR", dir.path())
        .env("TAILSCALE_AUTHKEY", "tskey-from-env")
        .assert()
        .success()
        .stdout(predicate::str::contains("tskey-from-env"))
        .stdout(predicate::str::contains("env"));
}

#[test]
fn config_get_nonexistent_key_exits_1() {
    let dir = tempfile::tempdir().unwrap();

    perc()
        .args(["config", "get", "nonexistent.key"])
        .env("PERC_CONFIG_DIR", dir.path())
        .env_remove("NONEXISTENT_KEY")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("no value set"));
}

#[test]
fn config_get_bad_key_format_exits_1() {
    let dir = tempfile::tempdir().unwrap();

    perc()
        .args(["config", "get", "nodot"])
        .env("PERC_CONFIG_DIR", dir.path())
        .assert()
        .code(1)
        .stderr(predicate::str::contains("section.field"));
}

#[test]
fn config_set_no_args_exits_2() {
    perc().args(["config", "set"]).assert().code(2);
}

#[test]
fn config_get_no_args_exits_2() {
    perc().args(["config", "get"]).assert().code(2);
}

#[test]
fn config_set_json_produces_valid_json() {
    let dir = tempfile::tempdir().unwrap();

    let output = perc()
        .args(["config", "set", "tailscale.authkey", "tskey-json", "--json"])
        .env("PERC_CONFIG_DIR", dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(v["key"], "tailscale.authkey");
}

#[test]
fn config_get_json_produces_valid_json() {
    let dir = tempfile::tempdir().unwrap();

    perc()
        .args(["config", "set", "tailscale.authkey", "tskey-json-get"])
        .env("PERC_CONFIG_DIR", dir.path())
        .assert()
        .success();

    let output = perc()
        .args(["config", "get", "tailscale.authkey", "--json"])
        .env("PERC_CONFIG_DIR", dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(v["key"], "tailscale.authkey");
    assert_eq!(v["value"], "tskey-json-get");
    assert_eq!(v["source"], "file");
}

#[test]
fn config_set_updates_existing_value() {
    let dir = tempfile::tempdir().unwrap();

    perc()
        .args(["config", "set", "tailscale.authkey", "old-value"])
        .env("PERC_CONFIG_DIR", dir.path())
        .assert()
        .success();

    perc()
        .args(["config", "set", "tailscale.authkey", "new-value"])
        .env("PERC_CONFIG_DIR", dir.path())
        .assert()
        .success();

    perc()
        .args(["config", "get", "tailscale.authkey"])
        .env("PERC_CONFIG_DIR", dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("new-value"));
}
