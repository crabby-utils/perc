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
    color_eyre::install()?;
    let args = cli::Args::parse();
    init_tracing(args.verbose, args.json);
    commands::dispatch(args).await
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
