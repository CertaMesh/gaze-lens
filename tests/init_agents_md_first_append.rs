//! Flow-layer test: directive 12 default-N when AGENTS.md exists without markers.

use gaze_lens::cli::init::flow::{InitEnv, run_guided};
use gaze_lens::cli::init::prompter::FakePrompter;
use gaze_lens::cli::init::{InitArgs, InitScope, SourceKind};

#[test]
fn default_no_appends_when_declined() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path();
    // Existing AGENTS.md without markers — synthesis blocker #8 default-N.
    std::fs::write(cwd.join("AGENTS.md"), "# Existing\nbody\n").unwrap();
    let env = InitEnv::test_with_home(dir.path().join("home"), cwd.to_path_buf(), None, None);
    let mut args = InitArgs::default_for_test();
    args.profile = Some("p".into());
    args.source_kind = Some(SourceKind::Sqlite);
    args.source_path = Some("/tmp/x.db".into());
    args.scope = Some(InitScope::User);
    args.no_mcp_config = true;
    // Default-N: user declines AGENTS.md append.
    let mut p = FakePrompter::new().with_confirm(false).with_confirm(false);
    let plan = run_guided(&args, &mut p, &env).expect("plan");
    assert!(
        plan.agents_md.is_none(),
        "AGENTS.md patch must be skipped on default-N"
    );
}
