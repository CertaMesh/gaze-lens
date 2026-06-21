use std::collections::BTreeMap;
use std::path::PathBuf;

use async_trait::async_trait;
use gaze_lens::errors::LensError;
use gaze_lens::session::restore::restore_whole_session;
use gaze_lens::session::{Session, SourceClass, ToolCall};
use gaze_lens::source::{FakeSource, SourceOutput, ToolArgs};
use gaze_lens::value::LensValue;

const CANARY: &str = "alice.replay@example.com";

fn main() {
    let result = match std::env::args().nth(1).as_deref() {
        Some("seed") => seed(),
        Some("restore") => restore(),
        _ => Err("usage: replay-fixture seed --manifest <path> --snapshot-dir <dir> | restore --manifest <path> --lens-session <ulid>".to_string()),
    };

    if let Err(err) = result {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn seed() -> Result<(), String> {
    let manifest = required_path("--manifest")?;
    let snapshot_dir = required_path("--snapshot-dir")?;
    let session =
        Session::new(&policy(), &manifest, &snapshot_dir).map_err(|err| err.to_string())?;
    session.register_fake_source_for_profile(
        SourceClass::Database,
        "default",
        Box::new(CanarySource),
    );
    let runtime = tokio::runtime::Runtime::new().map_err(|err| err.to_string())?;
    runtime
        .block_on(session.dispatch_tool(ToolCall {
            call_id: ulid::Ulid::new().to_string(),
            tool_name: "query".to_string(),
            args: ToolArgs(serde_json::json!({ "profile": "default", "email": CANARY })),
        }))
        .map_err(|err| err.to_string())?;
    println!("SEEDED: {}", session.lens_session_id());
    Ok(())
}

fn restore() -> Result<(), String> {
    let manifest = required_path("--manifest")?;
    let lens_session = required_value("--lens-session")?;
    let restored =
        restore_whole_session(&manifest, &lens_session, 0).map_err(|err| err.to_string())?;
    let saw_canary = restored
        .calls
        .iter()
        .any(|call| call.restored_args_json.contains(CANARY));
    if !saw_canary {
        return Err("restored session did not contain canary".to_string());
    }
    println!("RESTORED: {CANARY}");
    Ok(())
}

fn required_path(flag: &str) -> Result<PathBuf, String> {
    required_value(flag).map(PathBuf::from)
}

fn required_value(flag: &str) -> Result<String, String> {
    let mut args = std::env::args().skip(2);
    while let Some(arg) = args.next() {
        if arg == flag {
            return args
                .next()
                .ok_or_else(|| format!("missing value for {flag}"));
        }
    }
    Err(format!("missing required flag {flag}"))
}

fn policy() -> gaze::Policy {
    let mut policy = gaze::Policy::default();
    policy.session.scope = gaze::SessionScope::Conversation;
    policy.rulepacks.bundled = vec!["core".to_string()];
    policy
}

struct CanarySource;

#[async_trait]
impl FakeSource for CanarySource {
    async fn invoke(&self, _args: &ToolArgs) -> Result<SourceOutput, LensError> {
        Ok(SourceOutput::Rows(vec![BTreeMap::from([(
            "email".to_string(),
            LensValue::String(CANARY.to_string()),
        )])]))
    }
}
