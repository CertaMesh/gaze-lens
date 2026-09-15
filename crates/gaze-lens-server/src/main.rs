use clap::{Parser, Subcommand};
use std::path::PathBuf;
#[derive(Parser)]
#[command(
    name = "gaze-lens-server",
    version,
    disable_help_subcommand = true,
    about = "Unpublished execution-server development slice"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Serve {
        #[arg(long)]
        config: PathBuf,
    },
    Check {
        #[arg(long)]
        config: PathBuf,
    },
}
#[tokio::main]
async fn main() {
    std::panic::set_hook(Box::new(|_| eprintln!("internal_failure")));
    let result = match Cli::parse().command {
        Command::Serve { config } => gaze_lens_server::service::run(&config).await,
        Command::Check { config } => gaze_lens_server::config::check(&config).await.map(|_| ()),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
