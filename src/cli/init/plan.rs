//! In-memory plan produced by the guided flow.
//!
//! `run_guided` returns an `InitPlan` describing every file `commit_plan`
//! intends to write. `commit_plan` consumes the plan via `BatchWriter` so
//! tests can inject `FailingWriter` to drive partial-failure paths (CB6).

use std::path::PathBuf;

use crate::cli::init::{InitScope, McpClient, SourceKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSection {
    pub name: String,
    pub source_kind: SourceKind,
    pub source_host: Option<String>,
    pub source_port: Option<u16>,
    pub source_database: Option<String>,
    pub source_username: Option<String>,
    pub source_password_env: Option<String>,
    pub source_ssh_host: Option<String>,
    pub source_local_port: Option<u16>,
    pub source_path: Option<PathBuf>,
    pub source_json_text_columns: Vec<String>,
    pub policy_path: Option<PathBuf>,
    pub schema_allowlist: Vec<String>,
    pub snapshot_retention_days: Option<u32>,
    /// CB2: enum, never bool. `Purge` only when scope == ProjectAutoPurge AND
    /// the operator confirms the destructive prompt.
    pub auto_purge: AutoPurgeChoice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoPurgeChoice {
    Off,
    Warn,
    Purge,
}

/// Directive 19: NO `entry_key` field. The MCP writer (P6) reads existing file
/// content and chooses `gaze-lens` vs `gaze-lens-<name>` at write-time.
#[derive(Debug, Clone)]
pub struct McpTarget {
    pub client: McpClient,
    pub path: PathBuf,
    pub command: String,
    pub args: Vec<String>,
    pub profile_name: String,
}

#[derive(Debug, Clone)]
pub struct AgentsMdPatch {
    pub path: PathBuf,
    pub also_claude_md: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct InitPlan {
    pub profile_path: PathBuf,
    pub profile_scope: InitScope,
    pub profile_section: ProfileSection,
    pub mcp_targets: Vec<McpTarget>,
    pub agents_md: Option<AgentsMdPatch>,
    /// In-memory password value for the smoke-check phase. Never persisted to
    /// disk — set via stdin/prompt and forwarded to `std::env::set_var` only
    /// when `--smoke-check` is on.
    pub smoke_check_password_env_value: Option<String>,
}
