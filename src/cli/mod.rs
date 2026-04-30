use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::errors::LensError;

pub mod check;
pub mod check_trust;
pub mod demo;
pub mod init;
pub mod query;
pub mod replay;
pub mod retention;
pub mod serve;

#[derive(Debug, Parser)]
#[command(name = "gaze-lens", version, propagate_version = true)]
pub struct Cli {
    #[arg(long, env = "GAZE_LENS_PROJECT_CONFIG")]
    pub project_config: Option<PathBuf>,
    #[arg(long, env = "GAZE_LENS_USER_CONFIG")]
    pub user_config: Option<PathBuf>,
    #[arg(long)]
    pub log: Option<String>,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    Init(init::InitArgs),
    Query(query::QueryArgs),
    Replay(replay::ReplayArgs),
    Check(check::CheckArgs),
    Serve(serve::ServeArgs),
    /// Run a quick PII-redaction demonstration; tokenizes seeded canned data and
    /// inline-restores it. No persistent state.
    Demo(demo::DemoArgs),
}

pub fn run(cli: Cli) -> Result<(), LensError> {
    if let Some(log) = &cli.log {
        unsafe {
            std::env::set_var("RUST_LOG", log);
        }
    }
    match cli.cmd {
        Cmd::Init(args) => init::run(
            args,
            cli.project_config.as_deref(),
            cli.user_config.as_deref(),
        ),
        Cmd::Query(args) => {
            let runtime = tokio::runtime::Runtime::new().map_err(|err| LensError::Internal {
                detail: err.to_string(),
            })?;
            runtime.block_on(query::run(
                args,
                cli.project_config.as_deref(),
                cli.user_config.as_deref(),
            ))
        }
        Cmd::Replay(args) => replay::run(args),
        Cmd::Check(args) => {
            let runtime = tokio::runtime::Runtime::new().map_err(|err| LensError::Internal {
                detail: err.to_string(),
            })?;
            runtime.block_on(check::run(
                args,
                cli.project_config.as_deref(),
                cli.user_config.as_deref(),
            ))
        }
        Cmd::Serve(args) => {
            let runtime = tokio::runtime::Runtime::new().map_err(|err| LensError::Internal {
                detail: err.to_string(),
            })?;
            runtime.block_on(serve::run(
                args,
                cli.project_config.as_deref(),
                cli.user_config.as_deref(),
            ))
        }
        Cmd::Demo(args) => {
            let runtime = tokio::runtime::Runtime::new().map_err(|err| LensError::Internal {
                detail: err.to_string(),
            })?;
            runtime.block_on(demo::run(args))
        }
    }
}
