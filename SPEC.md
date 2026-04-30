# gaze-lens — product spec v1

> **Status:** locked 2026-04-26 via `/interview-me` + `/grill-me` sessions.
> **Org:** [PIInuts](https://github.com/PIInuts)
> **Engine:** built on the Gaze pseudonymization runtime (closed source).
> **Naming:** all PIInuts products carry the `gaze-` prefix. Decided 2026-04-26 with Markus. The original working name "Glance" was retired in favor of `gaze-lens` to fit the family convention.

## Problem

Dev teams want LLM agents to investigate production during incidents — query the DB, search app logs — without leaking PII to the model provider. Today there's no safe path: agents either get raw prod data (privacy violation, GDPR exposure, model-provider leak) or get nothing (and waste an hour while the on-call engineer eyeballs psql).

## Target user

Developer mid-incident. AI agent (Claude Code, Cursor, Codex, custom) running on the engineer's laptop. Wants to ask:

> *"Check the users table for accounts created in the last hour."*
> *"Grep the auth log for 500s in the past 10 min and group by route."*

…and get back tokenized results that say `<EMAIL_001>` instead of `alice@example.com`. Later wants to replay the agent's session with real values to verify what was found.

## Architecture

- **`gaze-lens` binary on the engineer's laptop.** Zero server-side install required at v1 — adoption-blocker for devs without prod-install rights.
- **MCP server is the primary surface** (stdio). Agent connects, calls tools, gets pseudonymized results. CLI is secondary, for humans to dry-run, replay sessions, configure profiles. **A single `gaze-lens serve` process exposes all configured profiles; each MCP tool call carries a required `profile` argument selecting which configured source to dispatch.** (D17)
- **`gaze-lens` owns connections + creds.** Reads from existing tooling (`.env`, Doppler, Vault, `~/.ssh/config`). Agent never sees raw connection strings.
- **Powered by Gaze engine** (closed-source dependency) for pseudonymization, audit log, restore manifest.
- **Pluggable spine** — source trait + frontend trait + shared session/manifest core. v1 fills DB+log sources behind an MCP frontend; v1.x adds new sources/frontends additively.
- v1 manifest is a **gaze-lens-local SQLite manifest**, distinct from Gaze's metadata-only redaction log. Snapshot blobs (`gaze::SensitiveSnapshot`) are stored as out-of-row 0600 files referenced from the manifest. (D9)

### Session lifecycle

Session-core lifecycle is decoupled from MCP-stdio process lifecycle. This lets v1.x add a long-running daemon mode (for SDK push ingest) without rewriting session/audit/restore.

### MCP server

A single `gaze-lens serve` process binds to one MCP entry per host and exposes all configured profiles. (D17)

- **One process, all profiles.** `init` writes a single MCP entry: `{ command: "gaze-lens", args: ["serve"] }`. Per-profile MCP entries from v0.2.x are deprecated; `init` rerun detects and offers to migrate them.
- **Required `profile` argument.** Every MCP tool call carries `profile: string` selecting the source. No default. Empty / unknown profile rejected as MCP `InvalidParams` with the loaded profile list in the error message.
- **Eager parse, lazy connect.** All profile TOML, policy files, and Gaze pipelines validated at process start. Source connections (sqlx pools, SSH validation) are deferred to first tool call referencing the profile and cached for the process lifetime. Profile reload requires restart.
- **Single Session, per-profile Pipeline.** One shared `gaze::Session` (one `lens_session_id`) with a per-profile `gaze::Pipeline` registry. Cross-profile token correlation is intentional: the same input redacts to the same token across profiles (Conversation scope semantics, D10).
- **Profile arg flows through `Pipeline::redact` (D7).** Routing extracts `profile` raw before redaction; the full args (with `profile` field intact) flow into `redact_args` and the manifest. Tokenized in storage, raw at dispatch.
- **Source-class compatibility checked.** Calling `query`/`schema`/`list_tables` with a log profile (or `log_tail`/`log_grep` with a DB profile) returns `InvalidParams` with a structured profile-class mismatch error.
- **Restrict-list `--profile`.** `gaze-lens serve --profile A --profile B` exposes only the listed profiles. Empty = all configured. Backward compatible with v0.2.x `serve --profile <name>` single-profile MCP entries (the existing entry becomes a one-element restrict-list).

## Threat model

### In scope (gaze-lens defends against)

- Raw production data reaching the LLM through any retrieval path. Mitigation: every retrieval routes through `gaze::Pipeline::redact` before manifest write or output return.
- Misconfigured queries leaking data via SQL string-injection or vendor side effects. Mitigation: canned structured queries only at v1; no raw SQL string accepted.
- Remote command injection through SSH log/tail tooling. Mitigation: validated host arguments (reject `-`-prefixed); fixed `ssh -- <host> tail -n N -- <quoted_path>` form (a second `--` between host and `tail` would be sent to the remote shell as a literal command and cause `--: command not found`); no shell-string interpolation.
- Operator-error retrieval bypass. Mitigation: human CLI `query` uses the same audit/redaction path as MCP. (D4)
- Schema-name leak (`customer_email_unhashed` etc.). Mitigation: tokenize column/table names with session-stable mapping + explicit allowlist policy. (D2)

### Out of scope (operator responsibility)

- **Laptop disk compromise.** Snapshot files contain raw token mappings; a laptop with unencrypted disk leaks them on physical theft. Operators MUST run FileVault (macOS) or LUKS (Linux) on the laptop running gaze-lens. v0.1 is Unix-only by build contract. v1 does not implement per-snapshot encryption-at-rest; this is a v1.x hardening tracked against gaze upstream feedback.
- **Same-uid attacker after process compromise.** Snapshot files are 0600 in a 0700 directory; this protects against other-user attackers but not against root or same-uid compromise.
- **SSH-side credential compromise.** gaze-lens reuses `~/.ssh/config` and the SSH agent; auth is the operator's responsibility.
- **Database write privilege.** gaze-lens never writes; the DB user MUST be configured read-only at the database side.
- **Backups.** Snapshot files are not auto-uploaded; operator is responsible for excluding `~/.gaze-lens/` from cloud backups if their threat model requires it.
- **Cross-profile token correlation.** A single MCP process running multiple profiles shares one `gaze::Session` snapshot; tokens for entities appearing in profile A and profile B collide deterministically. This is the same correlation an operator gets from running two CLI `query` calls in one Conversation-scope session and is acceptable per D10. Operators who need disjoint token spaces across profiles must run separate `gaze-lens serve` processes (one per profile group) until v2 introduces per-profile session scoping.

### v1 stop-gates

(See ARCHITECTURE.md §Stop-gates for the implementer-facing list.)

## v1 sources (cut tight)

1. **DB queries** — sqlx-backed (MySQL / Postgres / SQLite). Read-only. v1 query is a **canned structured shape** (`{table, columns?, where?, order_by?, limit?}`) compiled to safe parameterized SQL by gaze-lens. **No raw SQL strings in v1.** Raw SQL behind opt-in profile flag is a v1.x candidate. MCP tools: `query`, `schema`, `list_tables`. All three accept a required `profile: string` argument selecting the configured DB profile. (D5, D1)
2. **App logs** — plain file tail / grep over SSH. `gaze-lens` shells out to `ssh user@host tail -n 500 /var/log/app.log` (or `grep`), streams stdout, pseudonymizes per Gaze policy. Remote tail/grep is implemented as gaze-lens-local SSH command construction with strict shell-quoting and `--`-separated host arguments — not as a lift of debug-proxy code. (D16) MCP tools: `log_tail`, `log_grep`. Both accept a required `profile: string` argument selecting the configured ssh_log profile.

## CLI subcommand surface

v0.2 ships 6 CLI subcommands: `serve`, `init`, `query`, `replay`, `check`, `demo`. The 5 MCP tools surface remains locked.

The `demo` subcommand is a CLI-only inline-replay helper for adopters; it does not extend the MCP `frontend::mcp::McpFrontend` tool list and does not introduce a new data source. Adding any further subcommand or any new MCP tool still requires a SPEC amendment PR.

## Audit + restore

- Every MCP and CLI retrieval call writes to a **gaze-lens-local SQLite manifest** (D9). Manifest schema is gaze-lens's own; it coexists with Gaze's metadata-only redaction log but is not the same data plane (per Codex r1 unique insight).
- **Whole-session replay only at v1.** `gaze-lens replay <session_ulid>` walks the manifest call history and restores tokens via `gaze::Session::import(snapshot)`. Per-call replay is v1.x stop-gated on Gaze feedback for redaction-row correlation. (D8)
- Default Gaze session scope is `Scope::Conversation(<lens_session_id>)`; gaze-lens rejects `Scope::Ephemeral` at session construction because `Session::export()` rejects it. (D10)
- Tool args (SQL `where` AST, grep patterns, table/column names) are tokenized via the same `Pipeline::redact` path as result data **before** manifest write. Manifest never stores raw args. Raw args are reconstructable on operator replay via the session snapshot. (D7)
- Schema metadata (table/column names) flows through Gaze with a `schema_metadata` source class. This is presentation privacy for schema/list output, not an access-control grant. Query access is governed by each column's `ColumnInfo.allowed` value during canned-query validation. Default-deny posture; allowlist common safe names via `[schema] allow_columns = [...]`. Tokens are session-stable. (D2)

## Snapshot retention (v0.2 opt-in)

v0.2 introduces two profile fields governing snapshot lifecycle. Both are opt-in; v0.1 default-unlimited behavior is preserved when neither is set.

- `snapshot_retention_days: Option<u32>` — when set, snapshots whose ULID-embedded creation timestamp is older than `retention_days * 86_400_000` ms are subject to a startup sweep. `None` (default) = unlimited; manifest remains the operator's audit log of record per D3.
- `auto_purge: bool` — `false` (default) emits warn-only stderr listings of expired snapshots that would be purged. `true` performs a best-effort `remove_file` on each expired snapshot and writes a `purged_at_ms` tombstone on the corresponding `calls` row. The manifest row is preserved; only `snapshot_ref` is cleared. Replay of a tombstoned row produces a structured `LensError::SnapshotPurged` citing the policy — never a silent FS-only delete.

**Profile merge rule (destructive-default default-deny).** When merging a project profile file with a user profile file, the resolved `auto_purge` is

```text
merged_auto_purge = project.auto_purge && user.auto_purge
```

Plain conjunction. If the project file does not enable `auto_purge`, the user file cannot override to `true`. If the project file enables `auto_purge`, the user file may downgrade to warn-only by setting `auto_purge = false`. Consent for a destructive operational policy must be expressed at the team-shared (project) layer.

`snapshot_retention_days` itself merges by user-overrides-project (standard merge); the destructive-conjunction rule applies only to `auto_purge`.

**Multi-profile process retention bound.** When `gaze-lens serve` loads multiple profiles, snapshot retention is bound by the most-restrictive policy across loaded profiles, computed in addition to the per-profile project×user merge:

- `snapshot_retention_days = min(days)` over the loaded set; `None` is treated as +∞.
- `auto_purge` is a plain conjunction across loaded profiles; if any loaded profile resolves `auto_purge = false`, the process runs warn-only.

Shared snapshot_dir means the sweep affects all profiles' replay. Most-restrictive merge is the only safe boundary — never escalate destructiveness silently.

## Anti-features (locked)

- Not a DB GUI / admin tool. No web UI, no Sequel Pro replacement.
- Not a wire-protocol DB proxy. Apps don't connect through `gaze-lens`; only CLI / MCP / lib callers do.
- Not a credential vault. Reuses existing secret tooling.
- Not an ACL replacement. Assumes the DB user is already scoped read-only at the database level.
- No mutations at v1.
- No raw SQL queries at v1 — canned structured query shape only (D5).
- No server-side install required at v1. Standard SSH + DB conn from laptop.

## v1.x roadmap (not v1)

- Nginx / Apache log format-aware parsing.
- `systemd` journal (`gaze-lens journal --unit ...`).
- Structured JSON log parsing.
- DML / writes (deferred — see open Q5).

## v2 roadmap (within 12 months of v1 GA)

- **SDK ingest** — apps call `gaze_lens::dump($context)` from Laravel / Rails / JS / Python; payload streams to local `gaze-lens` daemon over HTTP; agent queries `recent_events --since 5m` via MCP.
  - Wedge: async / queue / multi-process debug + pseudonymized timeline.
  - `gaze-lens` becomes a long-running daemon at this point. Source-trait abstraction + decoupled session lifecycle make this additive, not rewrite.
- This is the closest `gaze-lens` gets to a Ray-style debugging companion. **No human GUI** — the consumer of pushed events is the agent, not human eyeballs.

## Sibling products in the gaze-* family

- **gaze** — pseudonymization engine. Closed source. The substrate `gaze-lens` and every other PIInuts product builds on.
- **gaze-laravel** — Laravel adapter for the Gaze engine. Already in development.
- **`gaze-lens`** (this product) — laptop-side, agent reaches OUT to prod via SSH/DB.
- **Future server-side companion (name TBD).** Installable on the prod box itself; inspects incoming SSH access to enable team-wide PII-safe access without each engineer running their own `gaze-lens`. Deferred: scoped + named in a separate session when ready. Likely repo `PIInuts/gaze-<X>`.

## Explicitly out of roadmap

- **Desktop GUI** (Spatie Ray-style). Decided 2026-04-26: agent-first product, no GUI unless a real human-eyeballs use case surfaces. Stays out of competitive lane with Ray.
- **Wire-protocol proxy** (mysql_proxy / pgbouncer-style). Different product, different tech, not the moat.
- **Server-side adapter inside `gaze-lens`.** This was the v1.x roadmap item before the 2026-04-26 product split — server-side is now its own future product, not a feature of `gaze-lens`.

## Locked decisions and v1.x candidates

- Snapshot encryption-at-rest is deferred to v1.x. v1 mitigation is the disk-encryption prerequisite in the threat model above.
- Per-call replay is deferred to v1.x and stop-gated on Gaze redaction-row correlation feedback.
- Raw SQL mode is a v1.x candidate behind explicit opt-in controls; v1 accepts only canned structured queries. (D5)
- Wider schema policy defaults remain ongoing as adopters report real schemas. (D2)
- Default Gaze policy remains product work, but does not block PR1's spine/audit contract.
- DML/write paths remain out of v1 and are future-product design work.
- Pricing and licensing remain deferred.

## Provenance

- Extracted from the `crates/debug-proxy/` crate inside the Gaze monorepo (kept as `reference/debug-proxy/` for evaluation).
- Spec authored via `/interview-me` + `/grill-me` sessions 2026-04-26 with Krishan.
- Renamed from "Glance" to `gaze-lens` 2026-04-26 after gaze-X family naming convention agreed with Markus.
- Architectural decisions mirrored to MemPalace under `wing_architect` and `wing_glance` (legacy) / `wing_gaze-lens`.
- Counselors r1 multi-voice review folded into plan rev 2 (scratchpad 488); decisions D1-D16 locked in scratchpad 477.
