use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;

fn perc() -> Command {
    Command::cargo_bin("perc").unwrap()
}

#[test]
fn new_creates_project_directory() {
    let dir = tempfile::tempdir().unwrap();
    perc()
        .args(["new", "myapp"])
        .current_dir(&dir)
        .assert()
        .success();

    assert!(dir.path().join("myapp").is_dir());
    assert!(dir.path().join("myapp/src").is_dir());
    assert!(dir.path().join("myapp/Cargo.toml").is_file());
    assert!(dir.path().join("myapp/src/main.rs").is_file());
    assert!(dir.path().join("myapp/perc.toml").is_file());
    assert!(dir.path().join("myapp/.gitignore").is_file());
}

#[test]
fn new_main_rs_contains_hello_with_app_name() {
    let dir = tempfile::tempdir().unwrap();
    perc()
        .args(["new", "myapp"])
        .current_dir(&dir)
        .assert()
        .success();

    let main_rs = fs::read_to_string(dir.path().join("myapp/src/main.rs")).unwrap();
    assert!(main_rs.contains("Hello world, from myapp"));
}

#[test]
fn new_perc_toml_has_app_name() {
    let dir = tempfile::tempdir().unwrap();
    perc()
        .args(["new", "myapp"])
        .current_dir(&dir)
        .assert()
        .success();

    let perc_toml = fs::read_to_string(dir.path().join("myapp/perc.toml")).unwrap();
    assert!(perc_toml.contains("name = \"myapp\""));
}

#[test]
fn new_cargo_toml_has_deps() {
    let dir = tempfile::tempdir().unwrap();
    perc()
        .args(["new", "myapp"])
        .current_dir(&dir)
        .assert()
        .success();

    let cargo_toml = fs::read_to_string(dir.path().join("myapp/Cargo.toml")).unwrap();
    assert!(cargo_toml.contains("axum"));
    assert!(cargo_toml.contains("tokio"));
    assert!(cargo_toml.contains("name = \"myapp\""));
}

#[test]
fn new_already_exists_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("myapp")).unwrap();

    perc()
        .args(["new", "myapp"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("already exists"));
}

#[test]
fn new_json_output() {
    let dir = tempfile::tempdir().unwrap();
    perc()
        .args(["--json", "new", "myapp"])
        .current_dir(&dir)
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""name":"myapp"#));
}

#[test]
fn new_invalid_name_starts_with_digit() {
    let dir = tempfile::tempdir().unwrap();
    perc()
        .args(["new", "1badname"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("not a valid crate name"));
}

#[test]
fn new_invalid_name_empty() {
    let dir = tempfile::tempdir().unwrap();
    perc()
        .args(["new", ""])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("not a valid crate name"));
}

#[test]
fn new_name_with_hyphens_and_underscores() {
    let dir = tempfile::tempdir().unwrap();
    perc()
        .args(["new", "my-cool_app"])
        .current_dir(&dir)
        .assert()
        .success();

    let main_rs = fs::read_to_string(dir.path().join("my-cool_app/src/main.rs")).unwrap();
    assert!(main_rs.contains("Hello world, from my-cool_app"));
}

#[test]
fn new_generated_project_compiles() {
    let dir = tempfile::tempdir().unwrap();
    perc()
        .args(["new", "compiletest"])
        .current_dir(&dir)
        .assert()
        .success();

    let status = std::process::Command::new("cargo")
        .args(["check"])
        .current_dir(dir.path().join("compiletest"))
        .status()
        .expect("cargo check failed to run");

    assert!(status.success(), "generated project did not compile");
}
