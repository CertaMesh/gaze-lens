use clap::Parser;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = gaze_lens::cli::Cli::parse();
    gaze_lens::cli::run(cli)?;
    Ok(())
}
