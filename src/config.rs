use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::PathBuf;
use std::{env, fs};

use color_eyre::eyre::{self, WrapErr};

fn config_dir() -> eyre::Result<PathBuf> {
    if let Ok(dir) = env::var("PERC_CONFIG_DIR") {
        return Ok(PathBuf::from(dir));
    }
    dirs::config_dir()
        .map(|d| d.join("perc"))
        .ok_or_else(|| eyre::eyre!("could not determine config directory"))
}

fn credentials_path() -> eyre::Result<PathBuf> {
    Ok(config_dir()?.join("credentials.toml"))
}

fn read_credentials() -> eyre::Result<toml_edit::DocumentMut> {
    let path = credentials_path()?;
    if !path.exists() {
        return Ok(toml_edit::DocumentMut::new());
    }
    let contents =
        fs::read_to_string(&path).wrap_err_with(|| format!("failed to read {}", path.display()))?;
    contents
        .parse::<toml_edit::DocumentMut>()
        .wrap_err("failed to parse credentials file")
}

fn write_credentials(doc: &toml_edit::DocumentMut) -> eyre::Result<PathBuf> {
    let dir = config_dir()?;
    if !dir.exists() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .wrap_err_with(|| format!("failed to create {}", dir.display()))?;
    }
    let path = dir.join("credentials.toml");
    fs::write(&path, doc.to_string())
        .wrap_err_with(|| format!("failed to write {}", path.display()))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .wrap_err("failed to set file permissions")?;
    Ok(path)
}

pub fn get(key: &str) -> eyre::Result<Option<GetResult>> {
    let (section, field) = parse_dotted_key(key)?;

    if let Some(val) = env_override(section, field) {
        return Ok(Some(GetResult {
            value: val,
            source: Source::Env,
        }));
    }

    let doc = read_credentials()?;
    let value = doc
        .get(section)
        .and_then(|t| t.get(field))
        .and_then(toml_edit::Item::as_str)
        .map(String::from);

    Ok(value.map(|v| GetResult {
        value: v,
        source: Source::File,
    }))
}

pub fn set(key: &str, value: &str) -> eyre::Result<PathBuf> {
    let (section, field) = parse_dotted_key(key)?;
    let mut doc = read_credentials()?;

    if !doc.contains_key(section) {
        doc[section] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    doc[section][field] = toml_edit::value(value);

    write_credentials(&doc)
}

pub fn resolve_tailscale_authkey() -> eyre::Result<String> {
    match get("tailscale.authkey")? {
        Some(result) => Ok(result.value),
        None => eyre::bail!(
            "tailscale auth key not found — set it with \
             `perc config set tailscale.authkey <key>` \
             or export TAILSCALE_AUTHKEY"
        ),
    }
}

fn parse_dotted_key(key: &str) -> eyre::Result<(&str, &str)> {
    key.split_once('.')
        .ok_or_else(|| eyre::eyre!("key must be in section.field format (e.g. tailscale.authkey)"))
}

fn env_override(section: &str, field: &str) -> Option<String> {
    let var_name = format!("{}_{}", section.to_uppercase(), field.to_uppercase());
    env::var(&var_name).ok()
}

pub struct GetResult {
    pub value: String,
    pub source: Source,
}

pub enum Source {
    Env,
    File,
}

impl Source {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Env => "env",
            Self::File => "file",
        }
    }
}
