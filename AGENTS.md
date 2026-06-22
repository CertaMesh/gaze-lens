# AGENTS.md

This file provides guidance for AI agents (Claude Code, Codex, Cursor, etc.) when working with code in this repository.

`gaze-lens` is a v0.1 PII-safe read-access tool for live production investigation by AI agents, built on the Gaze pseudonymization engine.

## Public surface (locked at v1)

The product surface is exactly:

- **5 MCP tools:** `query`, `schema`, `list_tables`, `log_tail`, `log_grep`.
- **6 CLI subcommands:** `serve`, `init`, `query`, `replay`, `check`, `demo`.

`demo` is a CLI-only inline-replay helper introduced in v0.2.0; it tokenizes a canned in-memory dataset and inline-restores it without persistent state. Adding a 7th subcommand or new MCP tool requires a SPEC amendment PR, not an impl PR. Internal helpers are fine; do not wire them through `frontend::mcp::McpFrontend` without updating [SPEC.md](./docs/reference/spec.md).

## Non-negotiables

- **No raw SQL.** v1 accepts canned structured queries only (`{table, columns?, where?, order_by?, limit?}`). Raw SQL behind opt-in profile flag is a v1.x candidate; do not introduce it as part of v1 work.
- **All retrievals route through the gaze-mcp-core chokepoint.** `Session::dispatch_tool` in `src/session/mod.rs` is the Lens-layer entry point; it constructs a `gaze_mcp_core::PiiEnvelope` and calls `envelope.dispatch(...)`. The envelope enforces redact→manifest→return ordering at the type level via the sealed `ToolCtx` parameter every `Tool::invoke` receives — adapters cannot return raw output to the agent without going through `gaze::Pipeline::redact` and the `ManifestStore`. New sources register a `Tool` impl in `Session::core_tool_registry` and dispatch through the same envelope; do not let raw values bypass redaction into the manifest, tracing, or error formatting.
- **Snapshot files require disk encryption.** `~/.gaze-lens/snapshots/` holds raw token mappings as `0600` files in a `0700` directory. v1 does not implement per-snapshot encryption-at-rest. The threat model assumes operators run FileVault / LUKS. Do not weaken that assumption without a SPEC update.
- **No `sqlx::query!` / `query_as!` macros for production-source adapters.** See [CONTRIBUTING.md §sqlx macro policy](./CONTRIBUTING.md#sqlx-macro-policy-banned-for-production-source-queries).
- **SSH command construction never uses interpolated strings.** Always use the `ssh -- <host> <command> -- <quoted_path>` form with validated host arguments. Reject `-`-prefixed hosts.
- **`LensValue` decode-failures reject the row.** Never silently fall back to empty string. SQLite JSON-in-TEXT must be policy-driven via the `json_text_columns` allowlist (default-deny).
- **Schema presentation is raw by default.** `schema` and `list_tables` may return raw table/column names unless the profile explicitly sets `schema_tokenize = true`. `schema_allowlist` is only a presentation exception in tokenized mode; it must not grant query access or imply tokenized mode by itself.

## Commands

```sh
# Build
cargo build --all-targets

# Test (default suite — no docker required)
cargo test --all-targets
cargo test --all-targets <test_name>          # one test
cargo test --test cli_query                   # one integration file
cargo test --doc                              # doc tests

# Lint + format
cargo fmt --check
cargo clippy --all-targets --no-deps -- -D warnings

# Feature-gated integration tests (require docker)
cargo build --features integration-postgres
cargo test --all-targets --features integration-postgres
cargo test --all-targets --features integration-mysql

# Run locally
cargo run -- init --profile dev
cargo run -- check --profile dev
cargo run -- query --profile dev --table users --limit 5
cargo run -- serve --profile dev    # MCP stdio server
```

`gaze`, `gaze-recognizers`, and `gaze-mcp-core` are crates.io deps. `gaze` aliases the `gaze-pii` package with `package = "gaze-pii"` so existing `use gaze::*` sites keep working. Local development can patch them to a checkout via `[patch.crates-io]` in `~/.cargo/config.toml` — see [CONTRIBUTING.md](./CONTRIBUTING.md#gaze-dependency-pin).

Detailed Rust conventions auto-load from `.claude/rules/rust.md` when editing `*.rs` files (paths-frontmatter; no `@import` needed).

## Architecture

The big picture: an AI agent calls one of 5 MCP tools over stdio. `gaze-lens` redacts the result through Gaze before returning anything, and writes a tokenized audit row to a local SQLite manifest. Later, the operator replays the session locally to see the original values.

```
agent → rmcp stdio → frontend::mcp::McpFrontend
                          ↓
                Session::dispatch_tool
                          ↓
       gaze_mcp_core::PiiEnvelope::dispatch  ◄── type-level chokepoint
       (sealed ToolCtx enforces redact + manifest ordering)
                          ↓
   ┌──────────────────────┼──────────────────────┐
   ↓                      ↓                      ↓
DbSource           LogSource (SSH)        SchemaTokenizer
(MySQL/PG/SQLite)  (tail/grep argv)       (presentation privacy)
   │                      │                      │
   └────── LensValue ─────┴── LensRow ───────────┘
                          ↓
              gaze::Pipeline::redact
                          ↓
   ┌──────────────────────┼──────────────────────┐
   ↓                                              ↓
GazeMcpManifestAdapter →             tokenized result → agent
LensManifestStore (SQLite)
   ↓
SensitiveSnapshot (out-of-row 0600 file)
   ↓
gaze-lens replay <session_ulid> → gaze::Session::import → original values
```

**Key invariant:** the CLI `query` command and the MCP `query` tool both route through `Session::dispatch_tool`, which builds the same `PiiEnvelope` from gaze-mcp-core — they share the redaction + manifest path. The envelope's sealed `ToolCtx` makes "raw adapter output reaching the agent without redaction" a compile-time error, not a runtime guard. Do not introduce a parallel direct-to-source code path.

**Source/Frontend split** is a pluggable spine — see `src/source/mod.rs` for the unified `Source` trait. v1 adapters: `db/{mysql,postgres,sqlite}.rs`, `log/ssh_log.rs`. Future v1.x adapters drop in additively without rewriting session/audit/restore.

**Session lifecycle is decoupled from MCP-stdio process lifecycle** (D13). This lets v1.x add a long-running daemon mode (for SDK push ingest) without rewriting session/audit/restore.

**Manifest vs Gaze redaction log.** `gaze-lens` writes its own SQLite manifest at `~/.gaze-lens/manifest.sqlite`; this coexists with Gaze's metadata-only redaction log but is a separate data plane. Snapshot blobs are stored as out-of-row files referenced from the manifest (D9).

For full architectural detail, file-by-file mining verdicts, and the 16 locked design decisions (D1-D16), read [ARCHITECTURE.md](./docs/reference/architecture.md) and the `Open decisions` reference in CONTRIBUTING.md.

## Where to look first

- [docs/reference/spec.md](./docs/reference/spec.md) — locked product spec, threat model, anti-features, v1.x / v2 roadmap.
- [docs/reference/architecture.md](./docs/reference/architecture.md) — implementer spine, core traits, session/manifest flow, file-by-file mining verdict.
- [CONTRIBUTING.md](./CONTRIBUTING.md) — dev workflow, crates.io Gaze dependency pinning, sqlx ban, PR review routing, integration-test feature flags.
- [docs/reference/profile-schema.md](./docs/reference/profile-schema.md) — profile schema, project vs user file precedence, schema policy.
- [docs/how-to/replay-a-session.md](./docs/how-to/replay-a-session.md) — `replay` command usage and snapshot operator controls.

## Status

v0.4.1 shipped 2026-05-27 (see CHANGELOG.md). The Gaze runtime resolves from crates.io (`gaze-pii`, `gaze-recognizers`, `gaze-mcp-core` at 0.9.0-rc.1); the legacy `GAZE_REPO_TOKEN` PAT is no longer required. Crates.io publish of `gaze-lens` itself is tracked separately. The predecessor crate at `reference/debug-proxy/` is historical reference material, excluded from the published package, and not part of the active build.
