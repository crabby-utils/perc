use assert_cmd::Command;

fn perc() -> Command {
    Command::cargo_bin("perc").unwrap()
}

#[test]
fn target_flag_accepted() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("perc.toml"), "[app]\nname = \"myapp\"\n").unwrap();

    perc()
        .args(["status", "--target", "prod"])
        .current_dir(&dir)
        .assert()
        .success();
}

#[test]
fn verbose_flag_accepted() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("perc.toml"), "[app]\nname = \"myapp\"\n").unwrap();

    perc()
        .args(["status", "-v"])
        .current_dir(&dir)
        .assert()
        .success();
}

#[test]
fn triple_verbose_accepted() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("perc.toml"), "[app]\nname = \"myapp\"\n").unwrap();

    perc()
        .args(["status", "-vvv"])
        .current_dir(&dir)
        .assert()
        .success();
}

#[test]
fn target_without_value_exits_2() {
    perc().args(["status", "--target"]).assert().code(2);
}
