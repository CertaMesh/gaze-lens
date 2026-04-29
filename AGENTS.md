# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

`gaze-lens` is a v0.1 PII-safe read-access tool for live production investigation by AI agents, built on the Gaze pseudonymization engine.

## Public surface (locked at v1)

The product surface is exactly:

- **5 MCP tools:** `query`, `schema`, `list_tables`, `log_tail`, `log_grep`.
- **5 CLI subcommands:** `serve`, `init`, `query`, `replay`, `check`.

Adding a 6th tool or subcommand requires a SPEC amendment PR, not an impl PR. Internal helpers are fine; do not wire them through `frontend::mcp::McpFrontend` without updating [SPEC.md](./SPEC.md).

## Non-negotiables

- **No raw SQL.** v1 accepts canned structured queries only (`{table, columns?, where?, order_by?, limit?}`). Raw SQL behind opt-in profile flag is a v1.x candidate; do not introduce it as part of v1 work.
- **All retrievals route through `Session::dispatch_tool`** in `src/session/mod.rs`. That call wraps every adapter result in `gaze::Pipeline::redact` before the manifest is written or output is returned. New sources must dispatch through the same path; do not let raw values bypass redaction into the manifest, tracing, or error formatting.
- **Snapshot files require disk encryption.** `~/.gaze-lens/snapshots/` holds raw token mappings as `0600` files in a `0700` directory. v1 does not implement per-snapshot encryption-at-rest. The threat model assumes operators run FileVault / LUKS. Do not weaken that assumption without a SPEC update.
- **No `sqlx::query!` / `query_as!` macros for production-source adapters.** See [CONTRIBUTING.md §sqlx macro policy](./CONTRIBUTING.md#sqlx-macro-policy-banned-for-production-source-queries).
- **SSH command construction never uses interpolated strings.** Always use the `ssh -- <host> <command> -- <quoted_path>` form with validated host arguments. Reject `-`-prefixed hosts.
- **`LensValue` decode-failures reject the row.** Never silently fall back to empty string. SQLite JSON-in-TEXT must be policy-driven via the `json_text_columns` allowlist (default-deny).

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

`gaze` and `gaze-recognizers` are local path-deps under `../../../../Workspace/bets/Gaze/crates/`. Pin the Gaze checkout to a specific tag — see [CONTRIBUTING.md](./CONTRIBUTING.md#gaze-path-dependency).

Detailed Rust conventions auto-load from `.claude/rules/rust.md` when editing `*.rs` files (paths-frontmatter; no `@import` needed).

## Architecture

The big picture: an AI agent calls one of 5 MCP tools over stdio. `gaze-lens` redacts the result through Gaze before returning anything, and writes a tokenized audit row to a local SQLite manifest. Later, the operator replays the session locally to see the original values.

```
agent → rmcp stdio → frontend::mcp::McpFrontend
                          ↓
                Session::dispatch_tool  ◄── single chokepoint
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
ManifestWriter (SQLite)              tokenized result → agent
   ↓
SensitiveSnapshot (out-of-row 0600 file)
   ↓
gaze-lens replay <session_ulid> → gaze::Session::import → original values
```

**Key invariant:** the CLI `query` command and the MCP `query` tool both route through `Session::dispatch_tool` — they share the same redaction + manifest path. Do not introduce a parallel direct-to-source code path.

**Source/Frontend split** is a pluggable spine — see `src/source/mod.rs` for the unified `Source` trait. v1 adapters: `db/{mysql,postgres,sqlite}.rs`, `log/ssh_log.rs`. Future v1.x adapters drop in additively without rewriting session/audit/restore.

**Session lifecycle is decoupled from MCP-stdio process lifecycle** (D13). This lets v1.x add a long-running daemon mode (for SDK push ingest) without rewriting session/audit/restore.

**Manifest vs Gaze redaction log.** `gaze-lens` writes its own SQLite manifest at `~/.gaze-lens/manifest.sqlite`; this coexists with Gaze's metadata-only redaction log but is a separate data plane. Snapshot blobs are stored as out-of-row files referenced from the manifest (D9).

For full architectural detail, file-by-file mining verdicts, and the 16 locked design decisions (D1-D16), read [ARCHITECTURE.md](./ARCHITECTURE.md) and the `Open decisions` reference in CONTRIBUTING.md.

## Where to look first

- [SPEC.md](./SPEC.md) — locked product spec, threat model, anti-features, v1.x / v2 roadmap.
- [ARCHITECTURE.md](./ARCHITECTURE.md) — implementer spine, core traits, session/manifest flow, file-by-file mining verdict.
- [CONTRIBUTING.md](./CONTRIBUTING.md) — dev workflow, Gaze path-dep pinning, sqlx ban, PR review routing, integration-test feature flags.
- [docs/profiles.md](./docs/profiles.md) — profile schema, project vs user file precedence, schema policy.
- [docs/replay.md](./docs/replay.md) — `replay` command usage and snapshot operator controls.

## Status

v0.1.1 ships from source (binary tarball available); v0.2 in flight. Crates.io publish + prebuilt binaries are tracked separately (cargo-dist workflow in progress). The predecessor crate at `reference/debug-proxy/` is a mining source, not part of the active build.
