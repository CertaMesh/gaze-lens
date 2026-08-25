use clap::Parser;
use std::process::ExitCode;

use gaze_lens::errors::format_cli_error;

fn main() -> ExitCode {
    let cli = gaze_lens::cli::Cli::parse();
    match gaze_lens::cli::run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            let verbose = std::env::var("GAZE_LENS_VERBOSE_ERRORS").as_deref() == Ok("1");
            eprintln!("{}", format_cli_error(&err, verbose));
            ExitCode::FAILURE
        }
    }
}
