//! CB6 partial-failure assertion. Drives `commit_plan` with a `FailingWriter`
//! that succeeds for the first N writes then fails. Asserts the resulting
//! `LensError::BatchPartial { applied, pending, failed, source }` shape.

use gaze_lens::cli::init::batch::FailingWriter;
use gaze_lens::cli::init::flow::{InitEnv, run_guided};
use gaze_lens::cli::init::prompter::FakePrompter;
use gaze_lens::cli::init::{InitArgs, InitScope, McpClient, SourceKind, commit_plan_for_test};
use gaze_lens::errors::LensError;

#[test]
fn batch_partial_when_mcp_write_fails_after_profile_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().join("cwd");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&home).unwrap();

    let mut args = InitArgs::default_for_test();
    args.non_interactive = true;
    args.profile = Some("p".into());
    args.source_kind = Some(SourceKind::Sqlite);
    args.source_path = Some("/tmp/x.db".into());
    args.scope = Some(InitScope::User);
    args.clients = vec![McpClient::ClaudeCode];
    args.no_agents_md = true;

    let env = InitEnv::test_with_home(home.clone(), cwd.clone(), None, None);
    let mut p = FakePrompter::new();
    let plan = run_guided(&args, &mut p, &env).expect("plan");

    // FailingWriter passes through 1 write (profile lands), then fails on
    // the second (MCP target).
    let mut w = FailingWriter::new(1);
    let err = commit_plan_for_test(&args, &plan, &mut w).expect_err("must fail");

    match err {
        LensError::BatchPartial {
            applied,
            pending,
            failed,
            source,
        } => {
            assert_eq!(applied.len(), 1, "profile must have landed");
            assert_eq!(applied[0], plan.profile_path);
            assert!(!pending.is_empty(), "MCP target must remain pending");
            assert!(
                failed.ends_with(".mcp.json"),
                "expected MCP target as failed dest; got {}",
                failed.display()
            );
            // Inner source must not be another BatchPartial; it's the leaf
            // FailingWriter error.
            assert!(
                !matches!(*source, LensError::BatchPartial { .. }),
                "BatchPartial nested inside BatchPartial is wrong"
            );
        }
        other => panic!("expected BatchPartial; got {other:?}"),
    }
}
