use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "perc",
    version,
    about = "Scaffold, run, and deploy Rust web apps to your own VPS"
)]
pub struct Args {
    /// Deploy target name (uses first configured target when not specified)
    #[arg(long, default_value = "local", global = true)]
    pub target: String,

    /// Output in machine-readable JSON format
    #[arg(long, global = true)]
    pub json: bool,

    /// Increase log verbosity (-v, -vv, -vvv)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Create a new perc project
    New {
        /// Name of the project (used as directory name and crate name)
        name: String,
    },
    /// Show project status
    Status,
    /// Manage operator configuration and credentials
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Manage environment variables in project config (perc.toml)
    Env {
        #[command(subcommand)]
        action: EnvAction,
    },
    /// Deploy apps and manage remote targets
    Deploy {
        /// Clear a stale deploy lock before running
        #[arg(long)]
        force: bool,

        #[command(subcommand)]
        action: DeployAction,
    },
    /// Run the local development environment
    Dev {
        #[command(subcommand)]
        action: Option<DevAction>,
    },
}

#[derive(Subcommand)]
pub enum DevAction {
    /// Start services and run the app (default)
    Up,
    /// Stop service containers (leaves data intact)
    Stop,
    /// Stop and remove containers and volumes (clean slate)
    Reset,
    /// Show running services and connection details
    Status,
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Set a configuration value (e.g. perc config set tailscale.authkey tskey-auth-...)
    Set {
        /// Dotted key path (e.g. tailscale.authkey)
        key: String,
        /// Value to store
        value: String,
    },
    /// Get a configuration value
    Get {
        /// Dotted key path (e.g. tailscale.authkey)
        key: String,
    },
}

#[derive(Subcommand)]
pub enum DeployAction {
    /// Bootstrap a fresh VPS for deployment
    Init {
        /// Hostname or IP address of the target server (SSH as root must work)
        host: String,
    },
    /// Build and push the app to a target
    Push,
    /// Associate a domain with a target for automatic HTTPS
    Domain {
        /// Domain name (e.g. example.com)
        name: String,
    },
    /// Add an already-initialized host as a deploy target
    Add {
        /// Tailscale hostname or IP of the target server
        host: String,
    },
    /// Show deployed apps on a target
    Status,
    /// Remove an app from a target
    Remove {
        /// App name to remove (defaults to current project's app name)
        name: Option<String>,
    },
    /// Provision a database for the app (migrations are your app's responsibility)
    Db,
    /// Show logs for the deployed app
    Logs {
        /// Number of lines to show
        #[arg(short = 'n', long, default_value = "50")]
        lines: u32,
        /// Follow log output in real time
        #[arg(short, long)]
        follow: bool,
    },
    /// Open the perc-stats monitoring dashboard in a browser
    Monitor,
    /// Manage secrets stored on the VPS (not in version control)
    Secret {
        #[command(subcommand)]
        action: SecretAction,
    },
}

#[derive(Subcommand)]
pub enum EnvAction {
    /// Set environment variables (e.g. perc env set S3_REGION=us-east-1)
    Set {
        /// KEY=VALUE pairs to set
        #[arg(required = true)]
        vars: Vec<String>,
    },
    /// Remove environment variables
    Unset {
        /// Keys to remove
        #[arg(required = true)]
        keys: Vec<String>,
    },
    /// List environment variables
    List,
}

#[derive(Subcommand)]
pub enum SecretAction {
    /// Set secret values on the VPS (e.g. perc deploy secret set S3_ACCESS_KEY=xxx)
    #[expect(
        clippy::doc_markdown,
        reason = "backticks around the example render poorly in --help"
    )]
    Set {
        /// KEY=VALUE pairs to set
        #[arg(required = true)]
        vars: Vec<String>,
    },
    /// Remove secrets from the VPS
    Unset {
        /// Keys to remove
        #[arg(required = true)]
        keys: Vec<String>,
    },
    /// List secrets stored on the VPS
    List {
        /// Show full secret values (masked by default)
        #[arg(long)]
        reveal: bool,
    },
}
