mod config;
mod deploy;
mod dev;
mod env;
mod new;
mod status;

use crate::cli::{Args, Command, ConfigAction, DeployAction, DevAction, EnvAction, SecretAction};
use crate::output::Output;

pub async fn dispatch(args: Args) -> color_eyre::Result<()> {
    let output = Output::new(args.json);
    match args.command {
        Command::New { name } => new::run(&output, &name),
        Command::Status => status::run(&output, &args.target),
        Command::Config { action } => match action {
            ConfigAction::Set { key, value } => config::run_set(&output, &key, &value),
            ConfigAction::Get { key } => config::run_get(&output, &key),
        },
        Command::Env { action } => match action {
            EnvAction::Set { vars } => env::run_set(&output, &vars),
            EnvAction::Unset { keys } => env::run_unset(&output, &keys),
            EnvAction::List => env::run_list(&output),
        },
        Command::Deploy { force, action } => match action {
            DeployAction::Init { host } => deploy::run_init(&output, &host).await,
            DeployAction::Push => deploy::run_push(&output, &args.target, force).await,
            DeployAction::Domain { name } => {
                deploy::run_domain(&output, &args.target, &name, force).await
            }
            DeployAction::Add { host } => deploy::run_add(&output, &host).await,
            DeployAction::Status => deploy::run_status(&output, &args.target).await,
            DeployAction::Remove { name } => {
                deploy::run_remove(&output, &args.target, name.as_deref(), force).await
            }
            DeployAction::Db => deploy::run_db(&output, &args.target, force).await,
            DeployAction::Logs { lines, follow } => {
                deploy::run_logs(&output, &args.target, lines, follow).await
            }
            DeployAction::Secret { action } => match action {
                SecretAction::Set { vars } => {
                    deploy::run_secret_set(&output, &args.target, &vars, force).await
                }
                SecretAction::Unset { keys } => {
                    deploy::run_secret_unset(&output, &args.target, &keys, force).await
                }
                SecretAction::List { reveal } => {
                    deploy::run_secret_list(&output, &args.target, reveal).await
                }
            },
        },
        Command::Dev { action } => {
            let action = action.unwrap_or(DevAction::Up);
            match action {
                DevAction::Up => dev::run_up(&output).await,
                DevAction::Stop => dev::run_stop(&output).await,
                DevAction::Reset => dev::run_reset(&output).await,
                DevAction::Status => dev::run_status(&output).await,
            }
        }
    }
}
