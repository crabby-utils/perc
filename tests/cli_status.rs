use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;

fn perc() -> Command {
    Command::cargo_bin("perc").unwrap()
}

#[test]
fn status_no_config_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    perc()
        .arg("status")
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("not a perc project"));
}

#[test]
fn status_valid_config_shows_app_name() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "[app]\nname = \"myapp\"\n").unwrap();

    perc()
        .arg("status")
        .current_dir(&dir)
        .assert()
        .success()
        .stdout(predicate::str::contains("myapp"));
}

#[test]
fn status_valid_config_with_targets_lists_them() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("perc.toml"),
        "[app]\nname = \"myapp\"\n\n[targets.staging]\nhost = \"s.example.com\"\n\n[targets.production]\nhost = \"p.example.com\"\n",
    )
    .unwrap();

    perc()
        .arg("status")
        .current_dir(&dir)
        .assert()
        .success()
        .stdout(predicate::str::contains("production"))
        .stdout(predicate::str::contains("staging"));
}

#[test]
fn status_malformed_config_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "not valid {{[ toml").unwrap();

    perc()
        .arg("status")
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("parse"));
}
