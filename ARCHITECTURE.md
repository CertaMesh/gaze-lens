# gaze-lens architecture

> Implementer-facing companion to `SPEC.md`. SPEC is the product contract; this is the spine.

## Spine layout

From scratchpad 458 §"Recommended v1 spine".

```text
src/
  lib.rs
  main.rs
  errors.rs
  source/
    mod.rs
    db/{mod.rs,mysql.rs,postgres.rs,sqlite.rs}
    log/{mod.rs,file_log.rs}
    ssh_tunnel.rs
  frontend/
    mod.rs
    mcp.rs
  session/
    mod.rs
    manifest.rs
    restore.rs
  policy.rs
  profile.rs
  cli/{mod.rs,init.rs,check.rs,serve.rs,query.rs,replay.rs}
```

## Core traits (v1)

From scratchpad 458 §"Core traits".

```rust
#[async_trait]
pub trait DbSource: Send + Sync {
    fn kind(&self) -> DbKind;
    fn profile_name(&self) -> &str;
    async fn list_tables(&self) -> Result<Vec<String>, SourceError>;
    async fn schema(&self, table: &str) -> Result<TableSchema, SourceError>;
    async fn query(&self, sql: &str, limit: usize) -> Result<Vec<BTreeMap<String, gaze::Value>>, SourceError>;
}

#[async_trait]
pub trait LogSource: Send + Sync {
    fn profile_name(&self) -> &str;
    async fn tail(&self, lines: usize) -> Result<Vec<String>, SourceError>;
    async fn grep(&self, pattern: &str, level: Option<&str>, limit: usize) -> Result<Vec<String>, SourceError>;
}

#[async_trait]
pub trait Frontend: Send + Sync {
    async fn serve(self, session: Arc<Session>, shutdown: ShutdownToken) -> Result<(), FrontendError>;
}
```

## Session/Manifest/Restore

From plan rev 2 §4 PR1 acceptance.

- `Session` owns `gaze::Session`, `gaze::Pipeline`, source maps, `ManifestWriter`.
- Session is **decoupled from MCP stdio** — constructible without any frontend.
- `dispatch_tool(call)` flow:
  1. `manifest.begin_call(&call)` — fail-closed.
  2. Adapter returns raw values/text.
  3. `Pipeline::redact(&gaze_session, RawDocument::*)` produces clean output and `SqliteLogger` metadata.
  4. `manifest.finish_call(...)` stores tokenized args, status, result summary, snapshot reference.
  5. If begin/finish fails, no raw output is returned.

## File-by-file mining verdict (debug-proxy → gaze-lens)

From scratchpad 442 mining audit + Codex r1 hardening notes.

| File | Verdict | Notes |
|---|---|---|
| adapter/ssh_tunnel.rs | LIFT + harden | Add `--` separator + host validation (D15). |
| adapter/laravel_log.rs | DROP | Reads local files; v1 needs remote tail. New code (D16). |
| adapter/mysql.rs | LIFT + relax | Fit `DbSource` trait; replace silent-empty-string fallback with explicit error (D11). Use dynamic sqlx (D12). |
| adapter/mod.rs | RESTRUCTURE | `ToolContext<D,L>` couples session to frontend; replace with frontend-trait + session-core split. |
| mcp/server.rs | RESTRUCTURE | Tool router becomes `frontend::mcp::McpFrontend` over `Arc<Session>`. Public surface is exactly the 5 SPEC tools. |
| mcp/errors.rs | LIFT | Error sanitizer thin wrapper. |
| policy.rs | LIFT + relax | Drop production-only constraint; multi-profile via `profile.rs`. |
| cli/* | PARTIAL LIFT | New: `query`, `replay` per D5/D8. `init` per D6. |

## Stop-gates (implementer)

PR1:

- Stop PR1 if Gaze path deps do not compile locally.
- Stop PR1 if cross-process replay cannot be proven with current Gaze APIs and accepted snapshot policy.
- Stop PR1 if fail-closed manifest behavior cannot be tested without leaking raw output.
- Stop PR1 if `Scope::Conversation` default and rejection of `Scope::Ephemeral` are not enforced at construction.
- Stop PR1 if `LensValue` round-trip does not preserve typed semantics for all v1 supported types: NULL, bool, i64, u64, f64, decimal-as-string, string, bytes, datetime, uuid, json.
- Stop PR1 if raw tool args can reach manifest/tracing/errors without `Pipeline::redact`.

PR2a:

- Stop PR2a if SQL safety cannot be made defensible through canned structured query validation + bound parameters + DB read-only credentials + caps.
- Stop PR2a if `ssh_tunnel.rs` does not reject `-`-prefixed hosts or lacks `--` host separation.
- Stop any adapter PR that introduces `sqlx::query!` / `query_as!` macros for production-source queries (D12 ban).
- Stop any adapter PR that expands public MCP surface without SPEC update.

PR2b:

- Stop PR2b if remote command construction is built from interpolated strings.

PR3:

- Stop PR3 if Postgres/SQLite cannot pass the same value-conversion + caps + audit gates as MySQL.

## Pluggability for v1.x and v2

- New sources behind `DbSource` / `LogSource` trait — additive only.
- New frontends behind `Frontend` trait — additive only (e.g. v2 HTTP intake daemon per SDK ingest).
- Coupling source==frontend is forbidden; v2 daemon mode must reuse the same redaction path.
