//! AGENTS.md / CLAUDE.md bounded-marker patcher.
//!
//! Stub — full impl lands in P7. Renderer must:
//! - Recognize `<!-- gaze-lens:init:start -->` / `<!-- gaze-lens:init:end -->`.
//! - Reject duplicate start, duplicate end, end-before-start (directive 12).
//! - Replace bounded content idempotently when re-run with the same profile.
//! - Append a fresh marker block when no markers present (default-N at the
//!   flow layer means we only reach this code when the user opted in).
//!
//! Until P7 ships, this stub returns a minimal valid block so commit_plan
//! compiles. It still emits the required substrings (`6 CLI subcommands`,
//! `5 MCP tools`) so P7's tests pin behavior without a separate refactor.

use crate::cli::init::profile_writer::RenderError;

const SNIPPET_TEMPLATE: &str = include_str!("agents_snippet.md");

const START_MARKER: &str = "<!-- gaze-lens:init:start -->";
const END_MARKER: &str = "<!-- gaze-lens:init:end -->";

pub fn render_agents_md_patch(
    existing: Option<&str>,
    profile_name: &str,
) -> Result<String, RenderError> {
    let snippet = SNIPPET_TEMPLATE.replace("{{PROFILE}}", profile_name);
    let block = format!("{START_MARKER}\n{snippet}\n{END_MARKER}\n");

    let Some(existing) = existing else {
        return Ok(block);
    };

    // Validate marker structure.
    let start_count = existing.matches(START_MARKER).count();
    let end_count = existing.matches(END_MARKER).count();
    if start_count > 1 {
        return Err(RenderError::Collision {
            name: format!("AGENTS.md duplicate start marker (found {start_count})"),
        });
    }
    if end_count > 1 {
        return Err(RenderError::Collision {
            name: format!("AGENTS.md duplicate end marker (found {end_count})"),
        });
    }
    if start_count == 1 && end_count == 1 {
        let s_idx = existing.find(START_MARKER).unwrap();
        let e_idx = existing.find(END_MARKER).unwrap();
        if e_idx < s_idx {
            return Err(RenderError::Collision {
                name: "AGENTS.md end marker before start marker".to_string(),
            });
        }
        // Bounded replace.
        let before = &existing[..s_idx];
        let after_end = e_idx + END_MARKER.len();
        let after = &existing[after_end..];
        let mut out = String::with_capacity(before.len() + block.len() + after.len());
        out.push_str(before);
        out.push_str(&block);
        if !after.is_empty() && !after.starts_with('\n') {
            out.push('\n');
        }
        out.push_str(after);
        return Ok(out);
    }
    if start_count == 0 && end_count > 0 {
        return Err(RenderError::Collision {
            name: "AGENTS.md end marker without start marker".to_string(),
        });
    }
    if start_count > 0 && end_count == 0 {
        return Err(RenderError::Collision {
            name: "AGENTS.md start marker without end marker".to_string(),
        });
    }
    // No markers present → append.
    let sep = if existing.ends_with('\n') { "" } else { "\n" };
    Ok(format!("{existing}{sep}\n{block}"))
}
