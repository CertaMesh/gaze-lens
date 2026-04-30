//! Trust report data model for `gaze-lens check --explain-risk`.
//!
//! The field set is closed under `REPORT_VERSION = 1`: adding, removing, or
//! renaming any serialized field requires bumping the report version and
//! documenting the migration.

use clap::ValueEnum;
use serde::Serialize;

pub const REPORT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
pub enum TrustFormat {
    #[default]
    Text,
    Json,
}

#[derive(Debug, Serialize)]
pub struct TrustReport {
    pub report_version: u32,
    pub profile: String,
    pub input_surface: InputSurface,
    pub process_surface: ProcessSurface,
    pub output_surface: OutputSurface,
    pub at_rest_surface: AtRestSurface,
    pub handoff_surface: HandoffSurface,
}

impl TrustReport {
    #[doc(hidden)]
    pub fn stub_for_test(profile: &str) -> Self {
        Self {
            report_version: REPORT_VERSION,
            profile: profile.to_string(),
            input_surface: InputSurface {
                mcp_tools: vec!["query", "schema", "list_tables", "log_tail", "log_grep"],
                cli_subcommands: vec!["init", "query", "replay", "check", "serve", "demo"],
                query_mode: "canned-structured",
                raw_sql: "disabled (v1 lock, D5)",
                source_kind: "sqlite",
                source_transport: serde_json::json!({
                    "path": "stub.sqlite",
                    "readonly_required": true
                }),
                sqlite_json_text_policy: Some(Vec::new()),
                code_evidence: vec![CodeEvidence {
                    file: "src/frontend/mcp.rs",
                    line: 19,
                    claim: "locked public MCP tools",
                }],
            },
            process_surface: ProcessSurface {
                process_model: "single-process MCP stdio (no daemon, D17)",
                profile_under_review: profile.to_string(),
                serve_default_scope: "all configured profiles unless serve --profile restrict-list",
                cross_profile_correlation: "default (Conversation; D10)".to_string(),
                connect_lifecycle: "eager-parse, lazy-connect",
                code_evidence: vec![CodeEvidence {
                    file: "src/cli/serve.rs",
                    line: 67,
                    claim: "serve expands shared manifest and snapshot paths",
                }],
            },
            output_surface: OutputSurface {
                dispatch_chokepoint: CodeEvidence {
                    file: "src/session/mod.rs",
                    line: 304,
                    claim: "dispatch_tool redacts source results",
                },
                tool_arg_redaction: "on (D7)",
                schema_policy: SchemaPolicy {
                    table_allowlist: None,
                    column_redaction_mode: "default (gaze recognizer pack)",
                },
                recognizer_pack: RecognizerPack {
                    source: "default-empty",
                    policy_path: None,
                    policy_sha256: None,
                    recognizer_keys: vec!["database".to_string()],
                    recognizer_classes: Vec::new(),
                    default_empty: true,
                },
                output_caps: OutputCapsView {
                    rows: 100,
                    bytes: 262_144,
                    cell_bytes: 4096,
                    line_bytes: 8192,
                    timeout_secs: 10,
                },
                code_evidence: vec![CodeEvidence {
                    file: "src/session/mod.rs",
                    line: 304,
                    claim: "dispatch_tool is the redaction chokepoint",
                }],
            },
            at_rest_surface: AtRestSurface {
                manifest: PathArtifact {
                    path: "~/.gaze-lens/manifest.sqlite".to_string(),
                    exists: false,
                    mode_ok: None,
                    expected_mode: "0600",
                },
                snapshot_dir: PathArtifact {
                    path: "~/.gaze-lens/snapshots".to_string(),
                    exists: false,
                    mode_ok: None,
                    expected_mode: "0700",
                },
                snapshot_retention_days: None,
                auto_purge: "off",
                snapshot_encryption_at_rest: "none (v1) - operator must run FileVault/LUKS",
                secret_backend: SecretLocator {
                    backend: "none",
                    identity: "not required".to_string(),
                },
                code_evidence: vec![CodeEvidence {
                    file: "src/session/manifest.rs",
                    line: 1,
                    claim: "manifest stores tokenized audit rows",
                }],
            },
            handoff_surface: HandoffSurface {
                residual_risks: vec![ResidualRisk {
                    id: "disk_encryption",
                    summary: "snapshot files rely on operator-managed disk encryption",
                    mitigation: "run FileVault or LUKS on hosts that store snapshots",
                }],
            },
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CodeEvidence {
    pub file: &'static str,
    pub line: u32,
    pub claim: &'static str,
}

#[derive(Debug, Serialize)]
pub struct InputSurface {
    pub mcp_tools: Vec<&'static str>,
    pub cli_subcommands: Vec<&'static str>,
    pub query_mode: &'static str,
    pub raw_sql: &'static str,
    pub source_kind: &'static str,
    pub source_transport: serde_json::Value,
    pub sqlite_json_text_policy: Option<Vec<String>>,
    pub code_evidence: Vec<CodeEvidence>,
}

#[derive(Debug, Serialize)]
pub struct ProcessSurface {
    pub process_model: &'static str,
    pub profile_under_review: String,
    pub serve_default_scope: &'static str,
    pub cross_profile_correlation: String,
    pub connect_lifecycle: &'static str,
    pub code_evidence: Vec<CodeEvidence>,
}

#[derive(Debug, Serialize)]
pub struct OutputSurface {
    pub dispatch_chokepoint: CodeEvidence,
    pub tool_arg_redaction: &'static str,
    pub schema_policy: SchemaPolicy,
    pub recognizer_pack: RecognizerPack,
    pub output_caps: OutputCapsView,
    pub code_evidence: Vec<CodeEvidence>,
}

#[derive(Debug, Serialize)]
pub struct SchemaPolicy {
    pub table_allowlist: Option<Vec<String>>,
    pub column_redaction_mode: &'static str,
}

#[derive(Debug, Serialize)]
pub struct RecognizerPack {
    pub source: &'static str,
    pub policy_path: Option<String>,
    pub policy_sha256: Option<String>,
    pub recognizer_keys: Vec<String>,
    pub recognizer_classes: Vec<String>,
    pub default_empty: bool,
}

#[derive(Debug, Serialize)]
pub struct OutputCapsView {
    pub rows: usize,
    pub bytes: usize,
    pub cell_bytes: usize,
    pub line_bytes: usize,
    pub timeout_secs: u64,
}

#[derive(Debug, Serialize)]
pub struct AtRestSurface {
    pub manifest: PathArtifact,
    pub snapshot_dir: PathArtifact,
    pub snapshot_retention_days: Option<u32>,
    pub auto_purge: &'static str,
    pub snapshot_encryption_at_rest: &'static str,
    pub secret_backend: SecretLocator,
    pub code_evidence: Vec<CodeEvidence>,
}

#[derive(Debug, Serialize)]
pub struct PathArtifact {
    pub path: String,
    pub exists: bool,
    pub mode_ok: Option<bool>,
    pub expected_mode: &'static str,
}

#[derive(Debug, Serialize)]
pub struct SecretLocator {
    pub backend: &'static str,
    pub identity: String,
}

#[derive(Debug, Serialize)]
pub struct HandoffSurface {
    pub residual_risks: Vec<ResidualRisk>,
}

#[derive(Debug, Serialize)]
pub struct ResidualRisk {
    pub id: &'static str,
    pub summary: &'static str,
    pub mitigation: &'static str,
}
