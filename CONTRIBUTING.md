# Contributing to gaze-lens

## Dev workflow

1. Anvil worktree per feature: `anvil create gaze-lens/<topic> --base main`.
2. PR-based; no direct-on-main.
3. `[agent]` prefix on every commit.
4. Test suite green before each commit; fix-and-commit-separately on failure.
5. Stage specific files by name; never `git add -A`.

## sqlx macro policy (banned for production-source queries)

**Do not use `sqlx::query!`, `sqlx::query_as!`, or any compile-time query macro for production-source adapters** (MySQL, Postgres, SQLite reading from operator profiles).

Reasons:
- Each backend's offline metadata cache is per-DB-version per-CI-pipeline; cost scales with the matrix.
- Reference adapter at `reference/debug-proxy/src/adapter/mysql.rs:51,114,125,143` already uses dynamic `sqlx::query`/`query_as`; that pattern is the production path.

Exceptions:
- Internal manifest schema queries against gaze-lens's own SQLite may use compile-time macros, since the DB schema is owned and stable.

## PR review routing

(Per orchestrator-mode skill.)

| PR class | Reviewer |
|---|---|
| Code (small-medium scope) | Codex |
| Spec/prose-heavy code | Claude |
| Docs-only (`*.md`) | None — orchestrator squash-merges after sanity read |
| High-stakes (release / security / multi-module) | Dual: Claude + Codex |

## Public-surface expansion rule

The 5 SPEC v1 MCP tools (`query`, `schema`, `list_tables`, `log_tail`, `log_grep`) are the locked public surface. Adding a 6th requires a SPEC amendment PR, not an impl PR. Internal helper methods are fine; do not wire them through `frontend::mcp::McpFrontend` without SPEC.

## MySQL integration tests

The standard `cargo test --all-targets` run must not require Docker or a local database. MySQL end-to-end tests belong behind the opt-in `integration-mysql` feature and should be run explicitly in environments that provide a MySQL testcontainer or compatible local service.

## Postgres integration tests

The standard `cargo test --all-targets` run must not require Docker or a local database. Postgres end-to-end tests belong behind the opt-in `integration-postgres` feature and should be run explicitly in environments that provide a Postgres testcontainer or compatible local service. SQLite target-source tests use a temp-file database and run in the standard suite.

Run the opt-in Postgres smoke with:

```sh
cargo test --all-targets --features integration-postgres
```

Default CI does not run this feature in v1; enabling it in CI is a deferred decision.

## Runtime policy fallback

`serve` loads `profile.policy` when a profile names a policy TOML. Profiles without an explicit policy use the built-in fallback equivalent to an empty `[policy.database]` section, which preserves non-sensitive fields and still enables the default email detector.
