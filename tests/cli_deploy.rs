use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;

fn perc() -> Command {
    Command::cargo_bin("perc").unwrap()
}

#[test]
fn deploy_init_no_host_exits_2() {
    perc().args(["deploy", "init"]).assert().code(2);
}

#[test]
fn deploy_init_no_authkey_exits_1() {
    let config_dir = tempfile::tempdir().unwrap();

    perc()
        .args(["deploy", "init", "192.168.1.100"])
        .env("PERC_CONFIG_DIR", config_dir.path())
        .env_remove("TAILSCALE_AUTHKEY")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("tailscale auth key not found"));
}

#[test]
fn deploy_init_no_authkey_json_exits_1_with_json() {
    let config_dir = tempfile::tempdir().unwrap();

    let output = perc()
        .args(["deploy", "init", "192.168.1.100", "--json"])
        .env("PERC_CONFIG_DIR", config_dir.path())
        .env_remove("TAILSCALE_AUTHKEY")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    let v: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap();
    assert_eq!(v["code"], "missing_authkey");
}

#[test]
fn help_deploy_shows_init_subcommand() {
    perc()
        .args(["help", "deploy"])
        .assert()
        .success()
        .stdout(predicate::str::contains("init"))
        .stdout(predicate::str::contains("Bootstrap"));
}

#[test]
fn help_deploy_init_shows_host_argument() {
    perc()
        .args(["help", "deploy", "init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("HOST"))
        .stdout(predicate::str::contains("Hostname or IP"));
}

#[test]
fn help_shows_config_and_deploy() {
    perc()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("config"))
        .stdout(predicate::str::contains("deploy"));
}

#[test]
fn help_deploy_shows_push_subcommand() {
    perc()
        .args(["help", "deploy"])
        .assert()
        .success()
        .stdout(predicate::str::contains("push"))
        .stdout(predicate::str::contains("Build and push"));
}

#[test]
fn deploy_push_no_perc_toml_exits_1() {
    let dir = tempfile::tempdir().unwrap();

    perc()
        .args(["deploy", "push"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("perc.toml not found"));
}

#[test]
fn deploy_push_missing_app_name_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "[targets]\n").unwrap();

    perc()
        .args(["deploy", "push"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("missing app.name"));
}

#[test]
fn deploy_push_no_targets_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "[app]\nname = \"myapp\"\n").unwrap();

    perc()
        .args(["deploy", "push"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("no targets configured"));
}

#[test]
fn deploy_push_named_target_not_found_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("perc.toml"),
        "[app]\nname = \"myapp\"\n\n[targets.prod]\nhost = \"example.com\"\n",
    )
    .unwrap();

    perc()
        .args(["--target", "staging", "deploy", "push"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn deploy_push_no_perc_toml_json_exits_1_with_json() {
    let dir = tempfile::tempdir().unwrap();

    let output = perc()
        .args(["--json", "deploy", "push"])
        .current_dir(&dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    let v: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap();
    assert_eq!(v["code"], "no_project");
}

#[test]
fn deploy_push_no_zigbuild_exits_with_error() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("perc.toml"),
        "[app]\nname = \"myapp\"\n\n[targets.prod]\nhost = \"example.com\"\n",
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"myapp\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();

    let output = perc()
        .args(["deploy", "push"])
        .current_dir(&dir)
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();

    assert!(!output.status.success());
}

#[test]
fn deploy_push_openssl_dep_exits_with_error() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("perc.toml"),
        "[app]\nname = \"myapp\"\n\n[targets.prod]\nhost = \"example.com\"\n",
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"myapp\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();

    let bin_dir = dir.path().join("bin");
    fs::create_dir(&bin_dir).unwrap();
    let fake_cargo = bin_dir.join("cargo");
    fs::write(
        &fake_cargo,
        "#!/bin/sh\necho 'myapp v0.1.0'\necho 'openssl-sys v0.9.100'\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake_cargo, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let fake_zigbuild = bin_dir.join("cargo-zigbuild");
    fs::write(&fake_zigbuild, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake_zigbuild, fs::Permissions::from_mode(0o755)).unwrap();
    }

    perc()
        .args(["deploy", "push"])
        .current_dir(&dir)
        .env("PATH", format!("{}:/usr/bin:/bin", bin_dir.display()))
        .assert()
        .code(1)
        .stderr(predicate::str::contains("openssl-sys"))
        .stderr(predicate::str::contains("rustls"));
}

#[test]
fn deploy_push_openssl_dep_json_exits_with_error_code() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("perc.toml"),
        "[app]\nname = \"myapp\"\n\n[targets.prod]\nhost = \"example.com\"\n",
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"myapp\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();

    let bin_dir = dir.path().join("bin");
    fs::create_dir(&bin_dir).unwrap();
    let fake_cargo = bin_dir.join("cargo");
    fs::write(
        &fake_cargo,
        "#!/bin/sh\necho 'myapp v0.1.0'\necho 'openssl-sys v0.9.100'\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake_cargo, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let fake_zigbuild = bin_dir.join("cargo-zigbuild");
    fs::write(&fake_zigbuild, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake_zigbuild, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let output = perc()
        .args(["--json", "deploy", "push"])
        .current_dir(&dir)
        .env("PATH", format!("{}:/usr/bin:/bin", bin_dir.display()))
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    let v: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap();
    assert_eq!(v["code"], "openssl_dep");
}

#[test]
fn help_deploy_shows_domain_subcommand() {
    perc()
        .args(["help", "deploy"])
        .assert()
        .success()
        .stdout(predicate::str::contains("domain"))
        .stdout(predicate::str::contains("Associate a domain"));
}

#[test]
fn help_deploy_domain_shows_name_argument() {
    perc()
        .args(["help", "deploy", "domain"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Domain name"));
}

#[test]
fn deploy_domain_no_name_exits_2() {
    perc().args(["deploy", "domain"]).assert().code(2);
}

#[test]
fn deploy_domain_no_perc_toml_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    perc()
        .args(["deploy", "domain", "example.com"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("perc.toml not found"));
}

#[test]
fn deploy_domain_no_targets_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "[app]\nname = \"myapp\"\n").unwrap();

    perc()
        .args(["deploy", "domain", "example.com"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("no targets configured"));
}

#[test]
fn deploy_domain_named_target_not_found_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("perc.toml"),
        "[app]\nname = \"myapp\"\n\n[targets.prod]\nhost = \"example.com\"\n",
    )
    .unwrap();

    perc()
        .args(["--target", "staging", "deploy", "domain", "example.com"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn deploy_domain_no_perc_toml_json_exits_1_with_json() {
    let dir = tempfile::tempdir().unwrap();

    let output = perc()
        .args(["--json", "deploy", "domain", "example.com"])
        .current_dir(&dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    let v: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap();
    assert_eq!(v["code"], "no_project");
}

#[test]
fn deploy_domain_saves_to_perc_toml_before_ssh() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("perc.toml"),
        "[app]\nname = \"myapp\"\n\n[targets.prod]\nhost = \"unreachable.invalid\"\n",
    )
    .unwrap();

    let _ = perc()
        .args(["deploy", "domain", "example.com"])
        .current_dir(&dir)
        .timeout(std::time::Duration::from_secs(35))
        .output();

    let contents = fs::read_to_string(dir.path().join("perc.toml")).unwrap();
    assert!(contents.contains(r#"domain = "example.com""#));
}

// --- deploy add ---

#[test]
fn help_deploy_shows_add_subcommand() {
    perc()
        .args(["help", "deploy"])
        .assert()
        .success()
        .stdout(predicate::str::contains("add"))
        .stdout(predicate::str::contains("Add an already-initialized host"));
}

#[test]
fn deploy_add_no_host_exits_2() {
    perc().args(["deploy", "add"]).assert().code(2);
}

#[test]
fn help_deploy_add_shows_host_argument() {
    perc()
        .args(["help", "deploy", "add"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Tailscale hostname"));
}

// --- deploy status ---

#[test]
fn help_deploy_shows_status_subcommand() {
    perc()
        .args(["help", "deploy"])
        .assert()
        .success()
        .stdout(predicate::str::contains("status"))
        .stdout(predicate::str::contains("Show deployed apps"));
}

#[test]
fn deploy_status_no_perc_toml_exits_1() {
    let dir = tempfile::tempdir().unwrap();

    perc()
        .args(["deploy", "status"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("perc.toml not found"));
}

#[test]
fn deploy_status_no_targets_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "[app]\nname = \"myapp\"\n").unwrap();

    perc()
        .args(["deploy", "status"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("no targets configured"));
}

#[test]
fn deploy_status_json_no_perc_toml_exits_1_with_json() {
    let dir = tempfile::tempdir().unwrap();

    let output = perc()
        .args(["--json", "deploy", "status"])
        .current_dir(&dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    let v: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap();
    assert_eq!(v["code"], "no_project");
}

// --- deploy remove ---

#[test]
fn help_deploy_shows_remove_subcommand() {
    perc()
        .args(["help", "deploy"])
        .assert()
        .success()
        .stdout(predicate::str::contains("remove"))
        .stdout(predicate::str::contains("Remove an app"));
}

#[test]
fn deploy_remove_no_perc_toml_exits_1() {
    let dir = tempfile::tempdir().unwrap();

    perc()
        .args(["deploy", "remove"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("perc.toml not found"));
}

#[test]
fn deploy_remove_no_targets_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "[app]\nname = \"myapp\"\n").unwrap();

    perc()
        .args(["deploy", "remove"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("no targets configured"));
}

#[test]
fn deploy_remove_json_no_perc_toml_exits_1_with_json() {
    let dir = tempfile::tempdir().unwrap();

    let output = perc()
        .args(["--json", "deploy", "remove"])
        .current_dir(&dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    let v: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap();
    assert_eq!(v["code"], "no_project");
}

#[test]
fn deploy_remove_with_explicit_name() {
    perc()
        .args(["help", "deploy", "remove"])
        .assert()
        .success()
        .stdout(predicate::str::contains("App name to remove"));
}

// --- deploy db ---

#[test]
fn help_deploy_shows_db_subcommand() {
    perc()
        .args(["help", "deploy"])
        .assert()
        .success()
        .stdout(predicate::str::contains("db"));
}

#[test]
fn deploy_db_no_perc_toml_exits_1() {
    let dir = tempfile::tempdir().unwrap();

    perc()
        .args(["deploy", "db"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("perc.toml not found"));
}

#[test]
fn deploy_db_no_targets_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "[app]\nname = \"myapp\"\n").unwrap();

    perc()
        .args(["deploy", "db"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("no targets configured"));
}

#[test]
fn deploy_db_json_no_perc_toml_exits_1_with_json() {
    let dir = tempfile::tempdir().unwrap();

    let output = perc()
        .args(["--json", "deploy", "db"])
        .current_dir(&dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    let v: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap();
    assert_eq!(v["code"], "no_project");
}

// --- deploy secret ---

#[test]
fn help_deploy_shows_secret_subcommand() {
    perc()
        .args(["help", "deploy"])
        .assert()
        .success()
        .stdout(predicate::str::contains("secret"))
        .stdout(predicate::str::contains("Manage secrets stored on the VPS"));
}

#[test]
fn help_deploy_secret_shows_subcommands() {
    perc()
        .args(["help", "deploy", "secret"])
        .assert()
        .success()
        .stdout(predicate::str::contains("set"))
        .stdout(predicate::str::contains("unset"))
        .stdout(predicate::str::contains("list"));
}

#[test]
fn deploy_secret_set_no_args_exits_2() {
    perc().args(["deploy", "secret", "set"]).assert().code(2);
}

#[test]
fn deploy_secret_unset_no_args_exits_2() {
    perc().args(["deploy", "secret", "unset"]).assert().code(2);
}

#[test]
fn deploy_secret_set_no_perc_toml_exits_1() {
    let dir = tempfile::tempdir().unwrap();

    perc()
        .args(["deploy", "secret", "set", "FOO=bar"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("perc.toml not found"));
}

#[test]
fn deploy_secret_set_no_targets_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "[app]\nname = \"myapp\"\n").unwrap();

    perc()
        .args(["deploy", "secret", "set", "FOO=bar"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("no targets configured"));
}

#[test]
fn deploy_secret_set_invalid_format_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("perc.toml"),
        "[app]\nname = \"myapp\"\n\n[targets.prod]\nhost = \"unreachable.invalid\"\n",
    )
    .unwrap();

    perc()
        .args(["deploy", "secret", "set", "NOEQUALS"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("expected KEY=VALUE"));
}

#[test]
fn deploy_secret_set_empty_key_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("perc.toml"),
        "[app]\nname = \"myapp\"\n\n[targets.prod]\nhost = \"unreachable.invalid\"\n",
    )
    .unwrap();

    perc()
        .args(["deploy", "secret", "set", "=value"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("key cannot be empty"));
}

#[test]
fn deploy_secret_unset_no_perc_toml_exits_1() {
    let dir = tempfile::tempdir().unwrap();

    perc()
        .args(["deploy", "secret", "unset", "FOO"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("perc.toml not found"));
}

#[test]
fn deploy_secret_unset_no_targets_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "[app]\nname = \"myapp\"\n").unwrap();

    perc()
        .args(["deploy", "secret", "unset", "FOO"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("no targets configured"));
}

#[test]
fn deploy_secret_list_no_perc_toml_exits_1() {
    let dir = tempfile::tempdir().unwrap();

    perc()
        .args(["deploy", "secret", "list"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("perc.toml not found"));
}

#[test]
fn deploy_secret_list_no_targets_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "[app]\nname = \"myapp\"\n").unwrap();

    perc()
        .args(["deploy", "secret", "list"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("no targets configured"));
}

#[test]
fn deploy_secret_set_json_no_perc_toml_exits_1_with_json() {
    let dir = tempfile::tempdir().unwrap();

    let output = perc()
        .args(["--json", "deploy", "secret", "set", "FOO=bar"])
        .current_dir(&dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    let v: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap();
    assert_eq!(v["code"], "no_project");
}

#[test]
fn deploy_secret_list_json_no_perc_toml_exits_1_with_json() {
    let dir = tempfile::tempdir().unwrap();

    let output = perc()
        .args(["--json", "deploy", "secret", "list"])
        .current_dir(&dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    let v: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap();
    assert_eq!(v["code"], "no_project");
}

// --- restate ---

#[test]
fn deploy_push_restate_missing_worker_binary_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("perc.toml"),
        "[app]\nname = \"myapp\"\n\n[restate]\nworker = \"myapp-worker\"\n\n[targets.prod]\nhost = \"example.com\"\n",
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"myapp\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();

    let bin_dir = dir.path().join("bin");
    fs::create_dir(&bin_dir).unwrap();
    let fake_cargo = bin_dir.join("cargo");
    fs::write(&fake_cargo, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake_cargo, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let fake_zigbuild = bin_dir.join("cargo-zigbuild");
    fs::write(&fake_zigbuild, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake_zigbuild, fs::Permissions::from_mode(0o755)).unwrap();
    }

    // Create a fake main binary so we get past the first check
    let release_dir = dir.path().join("target/x86_64-unknown-linux-musl/release");
    fs::create_dir_all(&release_dir).unwrap();
    fs::write(release_dir.join("myapp"), "fake binary").unwrap();

    perc()
        .args(["deploy", "push"])
        .current_dir(&dir)
        .env("PATH", format!("{}:/usr/bin:/bin", bin_dir.display()))
        .assert()
        .code(1)
        .stderr(predicate::str::contains("worker binary"))
        .stderr(predicate::str::contains("myapp-worker"));
}

#[test]
fn deploy_push_restate_default_worker_name() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("perc.toml"),
        "[app]\nname = \"myapp\"\n\n[restate]\n\n[targets.prod]\nhost = \"example.com\"\n",
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"myapp\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();

    let bin_dir = dir.path().join("bin");
    fs::create_dir(&bin_dir).unwrap();
    let fake_cargo = bin_dir.join("cargo");
    fs::write(&fake_cargo, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake_cargo, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let fake_zigbuild = bin_dir.join("cargo-zigbuild");
    fs::write(&fake_zigbuild, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake_zigbuild, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let release_dir = dir.path().join("target/x86_64-unknown-linux-musl/release");
    fs::create_dir_all(&release_dir).unwrap();
    fs::write(release_dir.join("myapp"), "fake binary").unwrap();

    // [restate] with no worker field should default to "myapp-worker"
    perc()
        .args(["deploy", "push"])
        .current_dir(&dir)
        .env("PATH", format!("{}:/usr/bin:/bin", bin_dir.display()))
        .assert()
        .code(1)
        .stderr(predicate::str::contains("myapp-worker"));
}

// --- deploy logs ---

#[test]
fn help_deploy_shows_logs_subcommand() {
    perc()
        .args(["help", "deploy"])
        .assert()
        .success()
        .stdout(predicate::str::contains("logs"))
        .stdout(predicate::str::contains("Show logs"));
}

#[test]
fn help_deploy_logs_shows_flags() {
    perc()
        .args(["help", "deploy", "logs"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--follow"))
        .stdout(predicate::str::contains("--lines"))
        .stdout(predicate::str::contains("50"));
}

#[test]
fn deploy_logs_no_perc_toml_exits_1() {
    let dir = tempfile::tempdir().unwrap();

    perc()
        .args(["deploy", "logs"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("perc.toml not found"));
}

#[test]
fn deploy_logs_no_targets_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("perc.toml"), "[app]\nname = \"myapp\"\n").unwrap();

    perc()
        .args(["deploy", "logs"])
        .current_dir(&dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("no targets configured"));
}

#[test]
fn deploy_logs_json_no_perc_toml_exits_1_with_json() {
    let dir = tempfile::tempdir().unwrap();

    let output = perc()
        .args(["--json", "deploy", "logs"])
        .current_dir(&dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    let v: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap();
    assert_eq!(v["code"], "no_project");
}
