use assert_cmd::Command;
use std::fs;

fn perc() -> Command {
    Command::cargo_bin("perc").unwrap()
}

#[test]
fn json_status_valid_project_is_valid_json_with_fields() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("perc.toml"),
        "[app]\nname = \"myapp\"\n\n[targets.prod]\nhost = \"example.com\"\n",
    )
    .unwrap();

    let output = perc()
        .args(["status", "--json"])
        .current_dir(&dir)
        .output()
        .unwrap();

    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(json.get("app_name").is_some());
    assert!(json.get("targets").is_some());
}

#[test]
fn json_status_no_project_exits_1_with_json_error() {
    let dir = tempfile::tempdir().unwrap();

    let output = perc()
        .args(["status", "--json"])
        .current_dir(&dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code().unwrap(), 1);
    let json: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(json.get("error").is_some());
    assert!(json.get("code").is_some());
}

#[test]
fn json_status_malformed_config_exits_1_with_json_error() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "not valid {{[ toml").unwrap();

    let output = perc()
        .args(["status", "--json"])
        .current_dir(&dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code().unwrap(), 1);
    let json: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(json.get("error").is_some());
}

#[test]
fn json_output_contains_no_ansi_escape_codes() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "[app]\nname = \"myapp\"\n").unwrap();

    let output = perc()
        .args(["status", "--json"])
        .current_dir(&dir)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("\x1b["),
        "JSON output should not contain ANSI escape codes"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("\x1b["),
        "JSON stderr should not contain ANSI escape codes"
    );
}
