use std::path::Path;
use std::process;

use serde::{Deserialize, Serialize};

use crate::output::Output;

#[derive(Deserialize)]
struct PercConfig {
    app: AppConfig,
    #[serde(default)]
    #[expect(
        clippy::zero_sized_map_values,
        reason = "only target keys matter, values are ignored"
    )]
    targets: std::collections::HashMap<String, serde::de::IgnoredAny>,
}

#[derive(Deserialize)]
struct AppConfig {
    name: String,
}

#[derive(Serialize)]
struct StatusOutput {
    app_name: String,
    targets: Vec<String>,
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "will return errors once config validation is added"
)]
pub fn run(output: &Output, _target: &str) -> color_eyre::Result<()> {
    let config_path = Path::new("perc.toml");

    if !config_path.exists() {
        output.error("not_a_project", "not a perc project (no perc.toml found)");
        process::exit(1);
    }

    let contents = match std::fs::read_to_string(config_path) {
        Ok(c) => c,
        Err(e) => {
            output.error(
                "config_read_error",
                &format!("failed to read perc.toml: {e}"),
            );
            process::exit(1);
        }
    };

    let config: PercConfig = match toml_edit::de::from_str(&contents) {
        Ok(c) => c,
        Err(e) => {
            output.error(
                "config_parse_error",
                &format!("failed to parse perc.toml: {e}"),
            );
            process::exit(1);
        }
    };

    let mut targets: Vec<String> = config.targets.into_keys().collect();
    targets.sort();

    let status = StatusOutput {
        app_name: config.app.name,
        targets,
    };

    output.success(&status);
    Ok(())
}
