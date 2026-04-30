//! Trust report data model for `gaze-lens check --explain-risk`.
//!
//! The field set is closed under `REPORT_VERSION = 1`: adding, removing, or
//! renaming any serialized field requires bumping the report version and
//! documenting the migration.

use clap::ValueEnum;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::Path;

use crate::errors::LensError;
use crate::profile::{Profile, SecretSpec, SourceSpec};

const CLI_SUBCOMMANDS: [&str; 6] = ["init", "query", "replay", "check", "serve", "demo"];
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

pub fn build_report(
    profile: &Profile,
    manifest_path: &Path,
    snapshot_dir: &Path,
    _parsed_policy: Option<(&Path, &[u8], &toml::Value)>,
) -> Result<TrustReport, LensError> {
    let recognizer_pack = match _parsed_policy {
        Some((path, raw_bytes, parsed)) => {
            recognizer_pack_from_parsed(Some(path), Some(parsed), Some(raw_bytes))
        }
        None => recognizer_pack_from_parsed(None, None, None),
    };
    Ok(TrustReport {
        report_version: REPORT_VERSION,
        profile: profile.name.clone(),
        input_surface: collect_input_surface(profile),
        process_surface: collect_process_surface(profile),
        output_surface: collect_output_surface(profile, recognizer_pack),
        at_rest_surface: collect_at_rest_surface(profile, manifest_path, snapshot_dir),
        handoff_surface: collect_handoff_surface(),
    })
}

pub fn collect_input_surface(profile: &Profile) -> InputSurface {
    InputSurface {
        mcp_tools: crate::frontend::mcp::McpFrontend::public_tool_names(),
        cli_subcommands: CLI_SUBCOMMANDS.to_vec(),
        query_mode: "canned-structured",
        raw_sql: "disabled (v1 lock, D5)",
        source_kind: source_kind(&profile.source),
        source_transport: source_transport(&profile.source),
        sqlite_json_text_policy: sqlite_json_text_policy(&profile.source),
        code_evidence: vec![
            CodeEvidence {
                file: "src/frontend/mcp.rs",
                line: 19,
                claim: "locked public MCP tools",
            },
            CodeEvidence {
                file: "src/source/db/query.rs",
                line: 1,
                claim: "canned structured query model",
            },
        ],
    }
}

pub fn collect_process_surface(profile: &Profile) -> ProcessSurface {
    ProcessSurface {
        process_model: "single-process MCP stdio (no daemon, D17)",
        profile_under_review: profile.name.clone(),
        serve_default_scope: "all configured profiles unless serve --profile restrict-list",
        cross_profile_correlation: "default (Conversation; D10)".to_string(),
        connect_lifecycle: "eager-parse, lazy-connect",
        code_evidence: vec![CodeEvidence {
            file: "src/cli/serve.rs",
            line: 67,
            claim: "serve loads configured profiles before MCP startup",
        }],
    }
}

pub fn collect_handoff_surface() -> HandoffSurface {
    HandoffSurface {
        residual_risks: vec![
            ResidualRisk {
                id: "disk_encryption",
                summary: "snapshot files rely on operator-managed disk encryption",
                mitigation: "enable FileVault, LUKS, or equivalent full-disk encryption on hosts that store snapshots",
            },
            ResidualRisk {
                id: "db_user_privileges",
                summary: "database credentials can still expose anything their role may read",
                mitigation: "use least-privilege read-only users and narrow schema allowlists",
            },
            ResidualRisk {
                id: "ssh_auth",
                summary: "ssh_log profiles inherit the operator's SSH trust and host-key posture",
                mitigation: "use dedicated read-only log access and pinned host verification",
            },
            ResidualRisk {
                id: "backup_exclusion",
                summary: "local backups may copy manifests and raw token snapshots",
                mitigation: "exclude ~/.gaze-lens/snapshots from unmanaged backups or encrypt those backups",
            },
            ResidualRisk {
                id: "cross_profile_correlation",
                summary: "conversation-scoped tokens can correlate repeated values across profiles in one session",
                mitigation: "run separate sessions when cross-profile correlation is undesirable",
            },
            ResidualRisk {
                id: "binary_attestation",
                summary: "v1 source installs do not prove binary provenance to agents",
                mitigation: "build from reviewed source or use future signed release artifacts when available",
            },
        ],
    }
}

pub fn source_kind(source: &SourceSpec) -> &'static str {
    match source {
        SourceSpec::Mysql { .. } => "mysql",
        SourceSpec::Postgres { .. } => "postgres",
        SourceSpec::Sqlite { .. } => "sqlite",
        SourceSpec::SshLog { .. } => "ssh_log",
    }
}

pub fn source_transport(source: &SourceSpec) -> serde_json::Value {
    match source {
        SourceSpec::Mysql {
            host,
            port,
            database,
            username,
            ssh_host,
            local_port,
            readonly_required,
            ..
        }
        | SourceSpec::Postgres {
            host,
            port,
            database,
            username,
            ssh_host,
            local_port,
            readonly_required,
            ..
        } => serde_json::json!({
            "host": host,
            "port": port,
            "database": database,
            "username": username,
            "ssh_host": ssh_host,
            "local_port": local_port,
            "readonly_required": readonly_required,
        }),
        SourceSpec::Sqlite {
            path,
            readonly_required,
            ..
        } => serde_json::json!({
            "path": path,
            "readonly_required": readonly_required,
        }),
        SourceSpec::SshLog { host, path } => serde_json::json!({
            "host": host,
            "path": path,
        }),
    }
}

pub fn secret_locator(source: &SourceSpec) -> SecretLocator {
    match source {
        SourceSpec::Mysql {
            password_env,
            secret,
            ..
        }
        | SourceSpec::Postgres {
            password_env,
            secret,
            ..
        } => match (password_env, secret) {
            (Some(env), None) => SecretLocator {
                backend: "env",
                identity: format!("var={env}"),
            },
            (None, Some(SecretSpec::Env { var })) => SecretLocator {
                backend: "env",
                identity: format!("var={var}"),
            },
            (None, Some(SecretSpec::Keyring { service, account })) => SecretLocator {
                backend: "keyring",
                identity: format!("service={service} account={account}"),
            },
            _ => SecretLocator {
                backend: "profile",
                identity: "invalid".to_string(),
            },
        },
        SourceSpec::Sqlite { .. } | SourceSpec::SshLog { .. } => SecretLocator {
            backend: "none",
            identity: "not required".to_string(),
        },
    }
}

pub fn sqlite_json_text_policy(source: &SourceSpec) -> Option<Vec<String>> {
    match source {
        SourceSpec::Sqlite {
            json_text_columns, ..
        } => Some(json_text_columns.clone()),
        SourceSpec::Mysql { .. } | SourceSpec::Postgres { .. } | SourceSpec::SshLog { .. } => None,
    }
}

pub fn inspect_path(path: &Path, expected_mode: u32) -> PathArtifact {
    let expanded = expand_path_lossy(path);
    let mode = std::fs::metadata(&expanded).ok().map(|metadata| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode() & 0o777
        }
        #[cfg(not(unix))]
        {
            let _ = metadata;
            expected_mode
        }
    });
    PathArtifact {
        path: expanded.display().to_string(),
        exists: mode.is_some(),
        mode_ok: mode.map(|actual| actual == expected_mode),
        expected_mode: mode_label(expected_mode),
    }
}

pub fn collect_at_rest_surface(
    profile: &Profile,
    manifest_path: &Path,
    snapshot_dir: &Path,
) -> AtRestSurface {
    AtRestSurface {
        manifest: inspect_path(manifest_path, 0o600),
        snapshot_dir: inspect_path(snapshot_dir, 0o700),
        snapshot_retention_days: profile.snapshot_retention_days,
        auto_purge: profile.auto_purge.as_str(),
        snapshot_encryption_at_rest: "none (v1) - operator must run FileVault/LUKS",
        secret_backend: secret_locator(&profile.source),
        code_evidence: vec![CodeEvidence {
            file: "src/session/manifest.rs",
            line: 1,
            claim: "manifest stores tokenized audit rows",
        }],
    }
}

pub fn recognizer_pack_from_parsed(
    policy_path: Option<&Path>,
    parsed: Option<&toml::Value>,
    raw_bytes: Option<&[u8]>,
) -> RecognizerPack {
    let Some(parsed) = parsed else {
        return RecognizerPack {
            source: "default-empty",
            policy_path: None,
            policy_sha256: None,
            recognizer_keys: vec!["database".to_string()],
            recognizer_classes: Vec::new(),
            default_empty: true,
        };
    };

    let mut recognizer_keys = Vec::new();
    let mut recognizer_classes = Vec::new();
    if let Some(policy) = parsed.get("policy").and_then(toml::Value::as_table) {
        for (section, value) in policy {
            recognizer_keys.push(section.to_string());
            collect_policy_classes(section, value, &mut recognizer_classes);
        }
    }
    recognizer_keys.sort();
    recognizer_classes.sort();
    RecognizerPack {
        source: "policy-toml",
        policy_path: policy_path.map(|path| expand_path_lossy(path).display().to_string()),
        policy_sha256: raw_bytes.map(sha256_hex),
        recognizer_keys,
        recognizer_classes,
        default_empty: false,
    }
}

pub fn render_text(report: &TrustReport, out: &mut dyn Write) -> std::io::Result<()> {
    if report.output_surface.recognizer_pack.default_empty {
        writeln!(
            out,
            "WARN: no recognizer pack - running with default-empty policy"
        )?;
    }
    writeln!(out, "Trust report v{}", report.report_version)?;
    writeln!(out, "profile: {}", report.profile)?;
    writeln!(out)?;

    writeln!(out, "Input surface")?;
    writeln!(
        out,
        "mcp_tools: {}",
        report.input_surface.mcp_tools.join(", ")
    )?;
    writeln!(
        out,
        "cli_subcommands: {}",
        report.input_surface.cli_subcommands.join(", ")
    )?;
    writeln!(out, "query_mode: {}", report.input_surface.query_mode)?;
    writeln!(out, "raw_sql: {}", report.input_surface.raw_sql)?;
    writeln!(out, "source_kind: {}", report.input_surface.source_kind)?;
    writeln!(
        out,
        "sqlite_json_text_policy: {}",
        format_optional_list(report.input_surface.sqlite_json_text_policy.as_ref())
    )?;
    writeln!(
        out,
        "evidence: {}",
        evidence_refs(&report.input_surface.code_evidence)
    )?;
    writeln!(out)?;

    writeln!(out, "Process surface")?;
    writeln!(
        out,
        "process_model: {}",
        report.process_surface.process_model
    )?;
    writeln!(
        out,
        "profile_under_review: {}",
        report.process_surface.profile_under_review
    )?;
    writeln!(
        out,
        "serve_default_scope: {}",
        report.process_surface.serve_default_scope
    )?;
    writeln!(
        out,
        "cross_profile_correlation: {}",
        report.process_surface.cross_profile_correlation
    )?;
    writeln!(
        out,
        "connect_lifecycle: {}",
        report.process_surface.connect_lifecycle
    )?;
    writeln!(
        out,
        "evidence: {}",
        evidence_refs(&report.process_surface.code_evidence)
    )?;
    writeln!(out)?;

    writeln!(out, "Output surface")?;
    writeln!(
        out,
        "dispatch_chokepoint: (see {}:{})",
        report.output_surface.dispatch_chokepoint.file,
        report.output_surface.dispatch_chokepoint.line
    )?;
    writeln!(
        out,
        "tool_arg_redaction: {}",
        report.output_surface.tool_arg_redaction
    )?;
    writeln!(
        out,
        "table_allowlist: {}",
        format_optional_list(report.output_surface.schema_policy.table_allowlist.as_ref())
    )?;
    writeln!(
        out,
        "column_redaction_mode: {}",
        report.output_surface.schema_policy.column_redaction_mode
    )?;
    writeln!(
        out,
        "recognizer_pack: {}",
        report.output_surface.recognizer_pack.source
    )?;
    writeln!(
        out,
        "recognizer_keys: {}",
        format_list(&report.output_surface.recognizer_pack.recognizer_keys)
    )?;
    writeln!(
        out,
        "recognizer_classes: {}",
        format_list(&report.output_surface.recognizer_pack.recognizer_classes)
    )?;
    writeln!(
        out,
        "evidence: {}",
        evidence_refs(&report.output_surface.code_evidence)
    )?;
    writeln!(out)?;

    writeln!(out, "At-rest surface")?;
    writeln!(out, "manifest: {}", report.at_rest_surface.manifest.path)?;
    writeln!(
        out,
        "manifest_mode_ok: {}",
        format_mode(report.at_rest_surface.manifest.mode_ok)
    )?;
    writeln!(
        out,
        "snapshot_dir: {}",
        report.at_rest_surface.snapshot_dir.path
    )?;
    writeln!(
        out,
        "snapshot_dir_mode_ok: {}",
        format_mode(report.at_rest_surface.snapshot_dir.mode_ok)
    )?;
    writeln!(
        out,
        "snapshot_retention_days: {}",
        report
            .at_rest_surface
            .snapshot_retention_days
            .map(|days| days.to_string())
            .unwrap_or_else(|| "unlimited".to_string())
    )?;
    writeln!(out, "auto_purge: {}", report.at_rest_surface.auto_purge)?;
    writeln!(
        out,
        "snapshot_encryption_at_rest: {}",
        report.at_rest_surface.snapshot_encryption_at_rest
    )?;
    writeln!(
        out,
        "secret_backend: {} {}",
        report.at_rest_surface.secret_backend.backend,
        report.at_rest_surface.secret_backend.identity
    )?;
    writeln!(
        out,
        "evidence: {}",
        evidence_refs(&report.at_rest_surface.code_evidence)
    )?;
    writeln!(out)?;

    writeln!(out, "Operator handoff")?;
    for risk in &report.handoff_surface.residual_risks {
        writeln!(
            out,
            "risk.{}: {} mitigation={}",
            risk.id, risk.summary, risk.mitigation
        )?;
    }
    Ok(())
}

fn collect_output_surface(profile: &Profile, recognizer_pack: RecognizerPack) -> OutputSurface {
    let caps = crate::session::OutputCaps::default();
    OutputSurface {
        dispatch_chokepoint: CodeEvidence {
            file: "src/session/mod.rs",
            line: 304,
            claim: "dispatch_tool redacts source results",
        },
        tool_arg_redaction: "on (D7)",
        schema_policy: SchemaPolicy {
            table_allowlist: profile.schema_allowlist.clone(),
            column_redaction_mode: "default (gaze recognizer pack)",
        },
        recognizer_pack,
        output_caps: OutputCapsView {
            rows: caps.rows,
            bytes: caps.bytes,
            cell_bytes: caps.cell_bytes,
            line_bytes: caps.line_bytes,
            timeout_secs: caps.timeout.as_secs(),
        },
        code_evidence: vec![CodeEvidence {
            file: "src/session/mod.rs",
            line: 304,
            claim: "dispatch_tool is the redaction chokepoint",
        }],
    }
}

fn collect_policy_classes(prefix: &str, value: &toml::Value, out: &mut Vec<String>) {
    if let Some(table) = value.as_table() {
        for (key, nested) in table {
            let next = format!("{prefix}.{key}");
            if nested.as_table().is_some() {
                collect_policy_classes(&next, nested, out);
            } else {
                out.push(next);
            }
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn expand_path_lossy(path: &Path) -> std::path::PathBuf {
    shellexpand::full(&path.to_string_lossy())
        .map(|path| std::path::PathBuf::from(path.into_owned()))
        .unwrap_or_else(|_| path.to_path_buf())
}

fn mode_label(mode: u32) -> &'static str {
    match mode {
        0o600 => "0600",
        0o700 => "0700",
        _ => "custom",
    }
}

fn evidence_refs(evidence: &[CodeEvidence]) -> String {
    evidence
        .iter()
        .map(|item| format!("(see {}:{})", item.file, item.line))
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_optional_list(values: Option<&Vec<String>>) -> String {
    values.map_or_else(|| "n/a".to_string(), |values| format_list(values))
}

fn format_list(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_string()
    } else {
        values.join(", ")
    }
}

fn format_mode(mode_ok: Option<bool>) -> &'static str {
    match mode_ok {
        Some(true) => "ok",
        Some(false) => "mismatch",
        None => "not present",
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
