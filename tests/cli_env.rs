use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;

fn perc() -> Command {
    Command::cargo_bin("perc").unwrap()
}

#[test]
fn help_shows_env_subcommand() {
    perc()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("env"))
        .stdout(predicate::str::contains("environment variables"));
}

#[test]
fn help_env_shows_subcommands() {
    perc()
        .args(["help", "env"])
        .assert()
        .success()
        .stdout(predicate::str::contains("set"))
        .stdout(predicate::str::contains("unset"))
        .stdout(predicate::str::contains("list"));
}

#[test]
fn env_set_no_args_exits_2() {
    perc().args(["env", "set"]).assert().code(2);
}

#[test]
fn env_unset_no_args_exits_2() {
    perc().args(["env", "unset"]).assert().code(2);
}

#[test]
fn env_set_no_perc_toml_exits_1() {
    let dir = tempfile::tempdir().unwrap();

    perc()
        .args(["env", "set", "FOO=bar"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("perc.toml not found"));
}

#[test]
fn env_unset_no_perc_toml_exits_1() {
    let dir = tempfile::tempdir().unwrap();

    perc()
        .args(["env", "unset", "FOO"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("perc.toml not found"));
}

#[test]
fn env_list_no_perc_toml_exits_1() {
    let dir = tempfile::tempdir().unwrap();

    perc()
        .args(["env", "list"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("perc.toml not found"));
}

#[test]
fn env_set_writes_to_perc_toml() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "[app]\nname = \"myapp\"\n").unwrap();

    perc()
        .args(["env", "set", "S3_REGION=us-east-1"])
        .current_dir(&dir)
        .assert()
        .success();

    let contents = fs::read_to_string(dir.path().join("perc.toml")).unwrap();
    assert!(contents.contains("S3_REGION"));
    assert!(contents.contains("us-east-1"));
}

#[test]
fn env_set_multiple_vars() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "[app]\nname = \"myapp\"\n").unwrap();

    perc()
        .args(["env", "set", "A=one", "B=two"])
        .current_dir(&dir)
        .assert()
        .success();

    let contents = fs::read_to_string(dir.path().join("perc.toml")).unwrap();
    assert!(contents.contains(r#"A = "one""#));
    assert!(contents.contains(r#"B = "two""#));
}

#[test]
fn env_set_updates_existing_value() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("perc.toml"),
        "[app]\nname = \"myapp\"\n\n[env]\nFOO = \"old\"\n",
    )
    .unwrap();

    perc()
        .args(["env", "set", "FOO=new"])
        .current_dir(&dir)
        .assert()
        .success();

    let contents = fs::read_to_string(dir.path().join("perc.toml")).unwrap();
    assert!(contents.contains(r#"FOO = "new""#));
    assert!(!contents.contains("old"));
}

#[test]
fn env_set_preserves_other_config() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("perc.toml"),
        "[app]\nname = \"myapp\"\n\n[database]\n",
    )
    .unwrap();

    perc()
        .args(["env", "set", "FOO=bar"])
        .current_dir(&dir)
        .assert()
        .success();

    let contents = fs::read_to_string(dir.path().join("perc.toml")).unwrap();
    assert!(contents.contains("[app]"));
    assert!(contents.contains("name = \"myapp\""));
    assert!(contents.contains("[database]"));
    assert!(contents.contains(r#"FOO = "bar""#));
}

#[test]
fn env_set_invalid_format_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "[app]\nname = \"myapp\"\n").unwrap();

    perc()
        .args(["env", "set", "NOEQUALS"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("expected KEY=VALUE"));
}

#[test]
fn env_set_empty_key_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "[app]\nname = \"myapp\"\n").unwrap();

    perc()
        .args(["env", "set", "=value"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("key cannot be empty"));
}

#[test]
fn env_set_value_with_equals() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "[app]\nname = \"myapp\"\n").unwrap();

    perc()
        .args(["env", "set", "URL=http://example.com?a=1&b=2"])
        .current_dir(&dir)
        .assert()
        .success();

    let contents = fs::read_to_string(dir.path().join("perc.toml")).unwrap();
    assert!(contents.contains("http://example.com?a=1&b=2"));
}

#[test]
fn env_unset_removes_key() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("perc.toml"),
        "[app]\nname = \"myapp\"\n\n[env]\nFOO = \"bar\"\nBAZ = \"qux\"\n",
    )
    .unwrap();

    perc()
        .args(["env", "unset", "FOO"])
        .current_dir(&dir)
        .assert()
        .success();

    let contents = fs::read_to_string(dir.path().join("perc.toml")).unwrap();
    assert!(!contents.contains("FOO"));
    assert!(contents.contains("BAZ"));
}

#[test]
fn env_unset_nonexistent_key_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "[app]\nname = \"myapp\"\n").unwrap();

    perc()
        .args(["env", "unset", "DOESNTEXIST"])
        .current_dir(&dir)
        .assert()
        .success();
}

#[test]
fn env_list_shows_vars() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("perc.toml"),
        "[app]\nname = \"myapp\"\n\n[env]\nFOO = \"bar\"\n",
    )
    .unwrap();

    perc()
        .args(["env", "list"])
        .current_dir(&dir)
        .assert()
        .success()
        .stdout(predicate::str::contains("FOO"))
        .stdout(predicate::str::contains("bar"));
}

#[test]
fn env_list_empty_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "[app]\nname = \"myapp\"\n").unwrap();

    perc()
        .args(["env", "list"])
        .current_dir(&dir)
        .assert()
        .success();
}

#[test]
fn env_set_json_output() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "[app]\nname = \"myapp\"\n").unwrap();

    let output = perc()
        .args(["--json", "env", "set", "FOO=bar"])
        .current_dir(&dir)
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(v["set"][0], "FOO");
}

#[test]
fn env_list_json_output() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("perc.toml"),
        "[app]\nname = \"myapp\"\n\n[env]\nFOO = \"bar\"\n",
    )
    .unwrap();

    let output = perc()
        .args(["--json", "env", "list"])
        .current_dir(&dir)
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(v["env"]["FOO"], "bar");
}

#[test]
fn env_list_json_no_perc_toml_exits_1_with_json() {
    let dir = tempfile::tempdir().unwrap();

    let output = perc()
        .args(["--json", "env", "list"])
        .current_dir(&dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    let v: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap();
    assert_eq!(v["code"], "no_project");
}
