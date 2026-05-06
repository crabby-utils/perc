use std::collections::BTreeMap;
use std::path::Path;
use std::process;

use color_eyre::eyre::WrapErr;
use serde::Serialize;

use crate::output::Output;

#[derive(Serialize)]
struct EnvSetResult {
    set: Vec<String>,
}

#[derive(Serialize)]
struct EnvUnsetResult {
    unset: Vec<String>,
}

#[derive(Serialize)]
struct EnvListResult {
    env: BTreeMap<String, String>,
}

fn read_perc_toml(output: &Output) -> (String, toml_edit::DocumentMut) {
    let path = Path::new("perc.toml");
    let Ok(contents) = std::fs::read_to_string(path) else {
        output.error(
            "no_project",
            "perc.toml not found — run this from a perc project directory",
        );
        process::exit(1);
    };
    let Ok(doc) = contents.parse::<toml_edit::DocumentMut>() else {
        output.error("config_parse", "failed to parse perc.toml");
        process::exit(1);
    };
    (contents, doc)
}

#[expect(clippy::unnecessary_wraps, reason = "dispatch requires Result return")]
pub fn run_set(output: &Output, vars: &[String]) -> color_eyre::Result<()> {
    let mut pairs = Vec::new();
    for var in vars {
        let Some((key, value)) = var.split_once('=') else {
            output.error(
                "invalid_format",
                &format!("expected KEY=VALUE, got {var:?}"),
            );
            process::exit(1);
        };
        if key.is_empty() {
            output.error("invalid_format", "key cannot be empty");
            process::exit(1);
        }
        pairs.push((key.to_string(), value.to_string()));
    }

    let (_, mut doc) = read_perc_toml(output);

    if doc.get("env").is_none() {
        doc["env"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    let env_table = doc["env"]
        .as_table_mut()
        .expect("env should be a table after init");

    let keys_set: Vec<String> = pairs.iter().map(|(k, _)| k.clone()).collect();
    for (key, value) in pairs {
        env_table.insert(&key, toml_edit::value(&value));
    }

    if let Err(e) =
        std::fs::write("perc.toml", doc.to_string()).wrap_err("failed to write perc.toml")
    {
        output.error("config_write", &format!("{e}"));
        process::exit(1);
    }

    output.success(&EnvSetResult { set: keys_set });
    Ok(())
}

#[expect(clippy::unnecessary_wraps, reason = "dispatch requires Result return")]
pub fn run_unset(output: &Output, keys: &[String]) -> color_eyre::Result<()> {
    let (_, mut doc) = read_perc_toml(output);

    if let Some(env_table) = doc.get_mut("env").and_then(toml_edit::Item::as_table_mut) {
        for key in keys {
            env_table.remove(key);
        }
    }

    if let Err(e) =
        std::fs::write("perc.toml", doc.to_string()).wrap_err("failed to write perc.toml")
    {
        output.error("config_write", &format!("{e}"));
        process::exit(1);
    }

    output.success(&EnvUnsetResult {
        unset: keys.to_vec(),
    });
    Ok(())
}

#[expect(clippy::unnecessary_wraps, reason = "dispatch requires Result return")]
pub fn run_list(output: &Output) -> color_eyre::Result<()> {
    let (_, doc) = read_perc_toml(output);

    let env: BTreeMap<String, String> = doc
        .get("env")
        .and_then(toml_edit::Item::as_table)
        .map(|t| {
            t.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.to_string(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();

    output.success(&EnvListResult { env });
    Ok(())
}
