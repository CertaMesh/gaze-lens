# gaze-lens — agent instructions

This is `gaze-lens` v1.0. PII-safe read-access to live production for AI agents,
built on the Gaze pseudonymization engine.

## Public surface (locked at v1)

The product surface is exactly:

- **5 MCP tools:** `query`, `schema`, `list_tables`, `log_tail`, `log_grep`.
- **5 CLI subcommands:** `serve`, `init`, `query`, `replay`, `check`.

Adding a 6th tool requires a SPEC amendment PR, not an impl PR. Internal helpers
are fine; do not wire them through `frontend::mcp::McpFrontend` without updating
[SPEC.md](./SPEC.md). Same rule for new CLI subcommands.

## Non-negotiables

- **No raw SQL.** v1 accepts canned structured queries only (`{table, columns?,
  where?, order_by?, limit?}`). Raw SQL behind opt-in profile flag is a v1.x
  candidate; do not introduce it as part of v1 work.
- **All retrievals route through `Session::dispatch_tool`** in `src/session/mod.rs`.
  That call wraps every adapter result in `gaze::Pipeline::redact` before the
  manifest is written or output is returned. New sources must dispatch through
  the same path; do not let raw values bypass redaction into the manifest,
  tracing, or error formatting.
- **Snapshot files require disk encryption.** `~/.gaze-lens/snapshots/` holds
  raw token mappings as `0600` files in a `0700` directory. v1 does not
  implement per-snapshot encryption-at-rest. The threat model assumes operators
  run FileVault / LUKS. Do not weaken that assumption without a SPEC update.
- **No `sqlx::query!` / `query_as!` macros for production-source adapters.**
  See [CONTRIBUTING.md §sqlx macro policy](./CONTRIBUTING.md#sqlx-macro-policy-banned-for-production-source-queries).
- **SSH command construction never uses interpolated strings.** Always use the
  `ssh -- <host> <command> -- <quoted_path>` form with validated host arguments.

## Where to look first

- [SPEC.md](./SPEC.md) — locked product spec, threat model, anti-features, v1.x / v2 roadmap.
- [ARCHITECTURE.md](./ARCHITECTURE.md) — implementer spine, core traits, session/manifest flow, file-by-file mining verdict.
- [CONTRIBUTING.md](./CONTRIBUTING.md) — dev workflow, Gaze path-dep pinning, sqlx ban, PR review routing, integration-test feature flags.
- [docs/profiles.md](./docs/profiles.md) — profile schema, project vs user file precedence, schema policy.
- [docs/replay.md](./docs/replay.md) — `replay` command usage and snapshot operator controls.

## Status

v1.0 ships from source. Crates.io publish + prebuilt binaries deferred. The
predecessor crate at `reference/debug-proxy/` is a mining source, not part of
the active build.
