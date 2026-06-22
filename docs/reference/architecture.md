# gaze-lens architecture

> Implementer-facing companion to [`spec.md`](./spec.md). SPEC is the product contract; this is the spine.

## Spine layout

From scratchpad 458 §"Recommended v1 spine".

```text
src/
  lib.rs
  main.rs
  errors.rs
  value.rs
  source/
    mod.rs
    ssh_tunnel.rs
    db/{mod.rs,mysql.rs,postgres.rs,sqlite.rs,query.rs,schema.rs}
    log/{mod.rs,ssh_log.rs}
  frontend/
    mod.rs
    mcp.rs
  session/
    mod.rs
    manifest.rs
    restore.rs
  policy.rs
  profile.rs
  cli/{mod.rs,init.rs,check.rs,serve.rs,query.rs,replay.rs,demo.rs}
```

Notes vs original spine sketch:

- `log/file_log.rs` was DROP per the mining audit; the actual v1 log source is `log/ssh_log.rs` (D16, PR2b).
- `value.rs` holds `LensValue` and the typed row plumbing introduced in PR1.
- `source/db/{query.rs,schema.rs}` split the canned-query AST and schema presentation helpers out of the per-engine adapters (PR2a/PR3). `schema`/`list_tables` present raw labels by default; `schema_tokenize = true` enables legacy tokenized presentation with allowlist exceptions.
- `examples/replay-fixture.rs` is an internal helper binary used by cross-process replay tests. It stays out of `src/bin` so Cargo's normal public/install bin surface only exposes `gaze-lens`.
- `cli/demo.rs` (added in v0.2.0) provides the `gaze-lens demo` inline-replay subcommand: it builds a tempdir manifest + snapshot dir, dispatches a canned in-memory query through the same `Session::dispatch_tool` entry as `query`/`serve` (which v0.3.0 reroutes onto `gaze_mcp_core::PiiEnvelope::dispatch`), then calls `gaze::Session::import` against the just-written snapshot to restore the tokenized result in the same process. No persistent state is touched.

## Core traits (v1)

From scratchpad 458 §"Core traits".

```rust
#[async_trait]
pub trait DbSource: Send + Sync {
    fn kind(&self) -> DbKind;
    fn profile_name(&self) -> &str;
    async fn list_tables(&self) -> Result<Vec<String>, LensError>;
    async fn schema(&self, table: &str) -> Result<TableSchema, LensError>;
    async fn query(&self, query: &CannedQuery) -> Result<Vec<LensRow>, LensError>;
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

- `Session` owns one shared `gaze::Session`, a per-profile `gaze::Pipeline` registry, a source map keyed by `(SourceClass, profile_name) -> Arc<LazySource>`, and `ManifestWriter`.
- Session is **decoupled from MCP stdio** — constructible without any frontend.
- `dispatch_tool(call)` flow (v0.3.0+ delegates the redact/manifest sequencing to `gaze_mcp_core::PiiEnvelope`; the lens-layer responsibilities are profile routing and `ManifestStore` adaptation):
  0. Extract `profile` from `call.args` raw (mode-aware: required in MultiProfile, defaults to configured name in SingleProfile); resolve `(tool_kind(tool), profile) -> Arc<LazySource>`; resolve `profile -> Arc<gaze::Pipeline>`.
  1. Build a `PiiEnvelope` over the per-profile pipeline, `LensAuthHook`, and `GazeMcpManifestAdapter` (which wraps the `LensManifestStore` over `~/.gaze-lens/manifest.sqlite`).
  2. `envelope.dispatch(...)` calls `manifest.begin_call(...)` — fail-closed.
  3. The matching `Tool::invoke` runs through a sealed `ToolCtx`; the adapter returns raw values/text into the envelope.
  4. `Pipeline::redact(&gaze_session, RawDocument::*)` produces clean output and `SqliteLogger` metadata before the envelope returns.
  5. `manifest.finish_call(...)` stores tokenized args, status, result summary, snapshot reference.
  6. If begin/finish fails, no raw output is returned. The sealed `ToolCtx` parameter makes a "raw output escapes without redaction" path a type error rather than a runtime check.

### Multi-profile session map

`gaze-lens serve` eagerly parses every selected profile, validates each runtime policy, and builds one `gaze::Pipeline` per profile before starting the MCP stdio server. It does not open DB pools or validate SSH reachability at startup. Source construction is lazy: the first tool call for a `(SourceClass, profile_name)` pair initializes the corresponding `LazySource` through `tokio::sync::OnceCell`, and concurrent first calls await the same initializer.

The MCP frontend still delegates to `Session::dispatch_tool(call)` with the original call args. The required `profile` argument is extracted inside `Session` before redaction for routing, but the same field remains in the args passed to `redact_args`, so the manifest stores the tokenized profile value rather than raw operator text.

### Manifest schema versioning

The `calls` table version is tracked via SQLite `PRAGMA user_version`. v0.1.x manifests use `user_version = 2`. v0.2 bumps this to `user_version = 3` by adding one nullable column:

```sql
ALTER TABLE calls ADD COLUMN purged_at_ms INTEGER;
```

SQLite's `ADD COLUMN` semantics give every existing v2 row a `NULL` value for the new column without rewriting page content; the migration is O(catalog), not O(rows). v0.1.x → v0.2 upgrades are automatic on first manifest open. Rollback is supported: a v0.1.x build opening a v0.2 manifest reads only the columns it knows about, ignoring `purged_at_ms`. New schema additions in v0.2.x must follow the same nullable-column pattern (or bump `user_version` again with an explicit migration step).

### ManifestMaintenance

Snapshot retention sweeping is implemented as a separate `ManifestMaintenance` type, distinct from `Session::new_with_pipeline`. The split is deliberate: session construction is a hot path on every CLI invocation and MCP frontend boot; the destructive sweep belongs on a different code path so it can be reasoned about and tested in isolation.

```rust
pub struct ManifestMaintenance { /* conn, manifest_path, snapshot_dir */ }

impl ManifestMaintenance {
    pub fn open(manifest_path: &Path, snapshot_dir: &Path) -> Result<Self, LensError>;
    pub fn sweep_expired_snapshots(
        &self,
        retention_days: u32,
        auto_purge: bool,
    ) -> Result<SweepReport, LensError>;
}
```

`open` opens the manifest at rest (no Gaze pipeline, no source connections). `sweep_expired_snapshots` walks `calls` rows with `status = 'ok'`, `snapshot_ref IS NOT NULL`, and `purged_at_ms IS NULL`, derives age from the ULID-embedded ms timestamp on the snapshot filename (FS-independent, deterministic), and either lists or removes plus tombstones expired entries depending on `auto_purge`.

CLI builders (`src/cli/query.rs`, `src/cli/serve.rs`) invoke `ManifestMaintenance::open(...).sweep_expired_snapshots(...)` BEFORE constructing the `Session`. This ordering matters: a sweep failure must not silently take a session down, and a sweep that emits friction-warning stderr should appear before any tool dispatch output.

When the active profile has `snapshot_retention_days = None`, CLI builders skip the maintenance call entirely — no manifest open, no sweep, zero IO. v0.1 default-unlimited semantics are preserved.

### v0.9 observability spine

v0.9 observability is a metadata extension to the existing redaction path. It does not add MCP tools, CLI subcommands, source traits, or a parallel diagnostic channel.

Implementation-facing constraints:

- Observability records are produced inside or immediately after `Pipeline::redact` and are returned only through `PiiEnvelope::dispatch` after manifest begin/finish succeeds.
- `Session::dispatch_tool` remains the only Lens-layer retrieval entry point. CLI `query`, MCP `query`, and future daemon ingest must not construct an observability-only path around the envelope.
- Manifest storage should use nullable additions, following the `purged_at_ms` migration precedent, and should persist only bounded tokenized metadata.
- Allowed signal families are ambiguity counts, validator-veto counts, collision-family / anchor-resolution outcomes, and locale-aware safety-net dispatch counts.
- Disallowed payload content includes raw PII, rejected raw candidates, exact raw-source snippets, reconstructable offsets, validator internals, connection secrets, or provider-specific model traces.
- Existing public surfaces consume observability first: optional MCP response metadata, CLI diagnostics on existing subcommands, `check` validation, `replay` explanation, and stderr/tracing health logs from `serve`.

New observability-specific MCP tools or CLI subcommands are explicitly not part of v0.9. They require a later SPEC amendment naming the new surface.

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

## Type Conversion Notes

- DB adapters return `LensRow` values, not lossy strings. `NULL` remains distinct from an empty string, bytes carry base64 metadata, and decode failures reject the row with an explicit conversion error.
- MySQL `DATETIME` has no timezone. v1 normalizes timezone-less MySQL datetime values to UTC RFC3339 strings by default so manifest and CLI output remain stable across operator laptops.

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

## Historical reference: `reference/debug-proxy/`

`reference/debug-proxy/` is the historical predecessor crate extracted from the Gaze monorepo (`crates/debug-proxy/`). It is retained for archaeology only — the [file-by-file mining verdict](#file-by-file-mining-verdict-debug-proxy--gaze-lens) above records what was lifted, hardened, restructured, or dropped from it. It is excluded from the published package and is not part of the active public source or build.

## See also

- [spec.md](./spec.md) — the locked product spec, threat model, and design decisions (D1–D16).
- [cli.md](./cli.md) — CLI subcommand surface.
- [mcp-tools.md](./mcp-tools.md) — MCP tool surface and the dispatch chokepoint.
- [profile-schema.md](./profile-schema.md) — profile fields and snapshot retention.
- [policy-schema.md](./policy-schema.md) — `gaze-policy.toml` fields.
- [pseudonymization-and-replay.md](../explanation/pseudonymization-and-replay.md) — session/manifest narrative and cross-profile token correlation.
