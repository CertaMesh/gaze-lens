use gaze_lens::cli::init::agents_md::render_agents_md_patch;

#[test]
fn duplicate_start_marker_is_explicit_error() {
    let existing = "<!-- gaze-lens:init:start -->\nA\n<!-- gaze-lens:init:start -->\nB\n<!-- gaze-lens:init:end -->\n";
    let err = render_agents_md_patch(Some(existing), "p").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("duplicate start marker"), "{msg}");
}

#[test]
fn duplicate_end_marker_is_explicit_error() {
    let existing = "<!-- gaze-lens:init:start -->\nA\n<!-- gaze-lens:init:end -->\n<!-- gaze-lens:init:end -->\n";
    let err = render_agents_md_patch(Some(existing), "p").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("duplicate end marker"), "{msg}");
}

#[test]
fn end_before_start_is_explicit_error() {
    let existing = "<!-- gaze-lens:init:end -->\nA\n<!-- gaze-lens:init:start -->\n";
    let err = render_agents_md_patch(Some(existing), "p").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("end marker before start"), "{msg}");
}

#[test]
fn empty_existing_yields_marker_block_with_profile_substituted() {
    let out = render_agents_md_patch(None, "dev").unwrap();
    assert!(out.contains("<!-- gaze-lens:init:start -->"));
    assert!(out.contains("<!-- gaze-lens:init:end -->"));
    assert!(out.contains("dev"));
    assert!(out.contains("Every MCP tool call requires a `profile` argument"));
    assert!(out.contains("`dev`"));
    assert!(out.contains(r#""profile": "dev""#));
    assert!(
        out.contains("6 CLI subcommands"),
        "snippet must reference SPEC subcommand count"
    );
    assert!(
        out.contains("5 MCP tools"),
        "snippet must reference SPEC MCP tool count"
    );
}

#[test]
fn existing_without_markers_appends_block() {
    let existing = "# Existing AGENTS.md\nbody\n";
    let out = render_agents_md_patch(Some(existing), "p").unwrap();
    assert!(out.starts_with("# Existing AGENTS.md\nbody\n"));
    assert!(out.contains("<!-- gaze-lens:init:start -->"));
}

#[test]
fn bounded_replace_inside_markers_preserves_outside() {
    let existing = "Outside above.\n<!-- gaze-lens:init:start -->\nold\n<!-- gaze-lens:init:end -->\nOutside below.\n";
    let out = render_agents_md_patch(Some(existing), "p").unwrap();
    assert!(out.contains("Outside above."));
    assert!(out.contains("Outside below."));
    assert!(
        !out.contains("\nold\n"),
        "stale marker block must be replaced"
    );
}

#[test]
fn idempotent_when_re_run_with_same_profile() {
    let first = render_agents_md_patch(None, "p").unwrap();
    let second = render_agents_md_patch(Some(&first), "p").unwrap();
    assert_eq!(first, second, "second render must be byte-identical");
}

#[test]
fn start_marker_without_end_is_explicit_error() {
    let existing = "<!-- gaze-lens:init:start -->\nbody\n";
    let err = render_agents_md_patch(Some(existing), "p").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("start marker without end"), "{msg}");
}

#[test]
fn end_marker_without_start_is_explicit_error() {
    let existing = "body\n<!-- gaze-lens:init:end -->\n";
    let err = render_agents_md_patch(Some(existing), "p").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("end marker without start"), "{msg}");
}
