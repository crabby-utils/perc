use assert_cmd::Command;
use predicates::prelude::*;

fn perc() -> Command {
    Command::cargo_bin("perc").unwrap()
}

#[test]
fn help_exits_0_and_contains_perc_and_status() {
    perc()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("perc"))
        .stdout(predicate::str::contains("status"));
}

#[test]
fn version_exits_0_and_contains_version() {
    let version = env!("CARGO_PKG_VERSION");
    perc()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(version));
}

#[test]
fn help_status_exits_0() {
    perc()
        .args(["help", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("status"));
}

#[test]
fn unknown_subcommand_exits_2() {
    perc().arg("nonexistent").assert().code(2);
}

#[test]
fn unknown_flag_exits_2() {
    perc().arg("--nonsense").assert().code(2);
}
