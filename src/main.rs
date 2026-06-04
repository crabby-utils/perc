#![warn(clippy::pedantic)]
#![allow(clippy::missing_errors_doc, clippy::missing_panics_doc)]

mod cli;
mod commands;
mod config;
mod output;

use clap::Parser;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    raise_fd_limit();
    color_eyre::install()?;
    let args = cli::Args::parse();
    init_tracing(args.verbose, args.json);
    commands::dispatch(args).await
}

/// Raise the soft `RLIMIT_NOFILE` (open file descriptor) limit so that
/// subprocesses we spawn — notably `cargo zigbuild`'s link step — don't hit
/// macOS's low default (256) and fail with `ProcessFdQuotaExceeded` (EMFILE).
///
/// Child processes inherit rlimits, so doing this once at startup covers every
/// build we shell out to. Failure is non-fatal: the OS may clamp the request to
/// the hard limit or `kern.maxfilesperproc`, and on Linux the limit is usually
/// already high enough that this is a harmless no-op.
///
/// We request `u64::MAX` rather than a fixed number: the argument is only a
/// ceiling, and `increase_nofile_limit` clamps it down to whatever the OS
/// actually permits (the hard limit, and on macOS `kern.maxfilesperproc`). It
/// never lowers an already-higher soft limit, so this just takes the maximum
/// available instead of guessing a magic constant.
fn raise_fd_limit() {
    // Runs before the tracing subscriber is installed, so warn via stderr.
    match rlimit::increase_nofile_limit(u64::MAX) {
        Ok(limit) => tracing::debug!("raised NOFILE soft limit to {limit}"),
        Err(err) => eprintln!("warning: could not raise open file descriptor limit: {err}"),
    }
}

fn init_tracing(verbose: u8, json: bool) {
    let filter = if std::env::var("RUST_LOG").is_ok() {
        EnvFilter::from_default_env()
    } else {
        let level = match verbose {
            0 => "warn",
            1 => "info",
            2 => "debug",
            _ => "trace",
        };
        EnvFilter::new(level)
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(!json)
        .with_writer(std::io::stderr)
        .init();
}
