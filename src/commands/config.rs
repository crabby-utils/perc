use std::process;

use serde::Serialize;

use crate::config;
use crate::output::Output;

#[derive(Serialize)]
struct ConfigValue {
    key: String,
    value: String,
    source: String,
}

#[derive(Serialize)]
struct ConfigSetResult {
    key: String,
    path: String,
}

#[expect(clippy::unnecessary_wraps, reason = "dispatch requires Result return")]
pub fn run_set(output: &Output, key: &str, value: &str) -> color_eyre::Result<()> {
    match config::set(key, value) {
        Ok(path) => {
            output.success(&ConfigSetResult {
                key: key.to_string(),
                path: path.display().to_string(),
            });
            Ok(())
        }
        Err(e) => {
            output.error("config_set_error", &format!("{e}"));
            process::exit(1);
        }
    }
}

#[expect(clippy::unnecessary_wraps, reason = "dispatch requires Result return")]
pub fn run_get(output: &Output, key: &str) -> color_eyre::Result<()> {
    match config::get(key) {
        Ok(Some(result)) => {
            output.success(&ConfigValue {
                key: key.to_string(),
                value: result.value,
                source: result.source.as_str().to_string(),
            });
            Ok(())
        }
        Ok(None) => {
            output.error("not_found", &format!("no value set for {key}"));
            process::exit(1);
        }
        Err(e) => {
            output.error("config_get_error", &format!("{e}"));
            process::exit(1);
        }
    }
}
