use gaze::{Action, ClassRule, DefaultRule, RawDocument};
use gaze_recognizers::RegexDetector;

fn main() {
    match run() {
        Ok(()) => {}
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<(), String> {
    let session =
        gaze::Session::new(gaze::Scope::Conversation("wave-b-a7-smoke".to_string()))
            .map_err(|err| err.to_string())?;
    let pipeline_a = build_pipeline()?;
    let pipeline_b = build_pipeline()?;

    let token_a = redact(&pipeline_a, &session)?;
    let token_b = redact(&pipeline_b, &session)?;
    println!("pipeline_a: {token_a}");
    println!("pipeline_b: {token_b}");

    if token_a == token_b {
        Ok(())
    } else {
        Err("tokens differ across identical pipelines sharing one session".to_string())
    }
}

fn build_pipeline() -> Result<gaze::Pipeline, String> {
    gaze::Pipeline::builder()
        .detector(RegexDetector::emails().map_err(|err| err.to_string())?)
        .rule(ClassRule::new(gaze::PiiClass::Email, Action::Tokenize))
        .rule(DefaultRule::new(Action::Preserve))
        .build()
        .map_err(|err| err.to_string())
}

fn redact(pipeline: &gaze::Pipeline, session: &gaze::Session) -> Result<String, String> {
    match pipeline
        .redact(session, RawDocument::Text("alice@example.com".to_string()))
        .map_err(|err| err.to_string())?
    {
        gaze::CleanDocument::Text(text) => Ok(text),
        gaze::CleanDocument::Structured(_) => Err("expected text redaction".to_string()),
    }
}
