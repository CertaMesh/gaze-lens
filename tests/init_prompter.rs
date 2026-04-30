use gaze_lens::cli::init::prompter::{FakePrompter, PromptError, Prompter};

#[test]
fn fake_prompter_returns_scripted_inputs_in_order() {
    let mut fake = FakePrompter::new()
        .with_text("prod")
        .with_confirm(true)
        .with_select(2)
        .with_password("hunter2");
    assert_eq!(fake.input("Profile name?", None).unwrap(), "prod");
    assert!(fake.confirm("Continue?", false).unwrap());
    assert_eq!(
        fake.select("Source?", &["mysql", "postgres", "sqlite", "ssh-log"])
            .unwrap(),
        2
    );
    assert_eq!(fake.password("DB password?").unwrap(), "hunter2");
}

#[test]
fn fake_prompter_errors_on_unscripted_call_by_default() {
    let mut fake = FakePrompter::new();
    let err = fake.confirm("anything?", false).unwrap_err();
    assert!(matches!(
        err,
        PromptError::ScriptExhausted { kind: "confirm" }
    ));
    let err = fake.input("name?", Some("default")).unwrap_err();
    assert!(matches!(err, PromptError::ScriptExhausted { kind: "text" }));
}

#[test]
fn fake_prompter_with_allow_defaults_returns_default_when_underflow() {
    let mut fake = FakePrompter::new().allow_defaults();
    assert_eq!(fake.input("n?", Some("d")).unwrap(), "d");
    assert!(!fake.confirm("c?", false).unwrap());
}

#[test]
fn non_interactive_path_makes_zero_prompter_calls() {
    let fake = FakePrompter::new();
    assert!(fake.is_strict_and_empty());
}

#[test]
fn fake_prompter_records_last_prompt_message() {
    let mut fake = FakePrompter::new().with_confirm(true);
    let _ = fake
        .confirm(
            "This deletes snapshot files older than 7 days. Continue?",
            false,
        )
        .unwrap();
    assert_eq!(
        fake.last_prompt.as_deref(),
        Some("This deletes snapshot files older than 7 days. Continue?"),
    );
}
