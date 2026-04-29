use clap::Parser;
use std::process::ExitCode;

use gaze_lens::errors::sanitize_error;

fn main() -> ExitCode {
    let cli = gaze_lens::cli::Cli::parse();
    match gaze_lens::cli::run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{}", sanitize_error(&err));
            ExitCode::FAILURE
        }
    }
}
