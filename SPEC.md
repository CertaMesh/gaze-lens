# Glance — product spec v1

> **Status:** locked 2026-04-26 via interview-me + grill-me sessions.
> **Org:** [PIInuts](https://github.com/PIInuts)
> **Engine:** built on the Gaze pseudonymization runtime (closed source).

## Problem

Dev teams want LLM agents to investigate production during incidents — query the DB, search app logs — without leaking PII to the model provider. Today there's no safe path: agents either get raw prod data (privacy violation, GDPR exposure, model-provider leak) or get nothing (and waste an hour while the on-call engineer eyeballs psql).

## Target user

Developer mid-incident. AI agent (Claude Code, Cursor, Codex, custom) running on the engineer's laptop. Wants to ask:

> *"Check the users table for accounts created in the last hour."*
> *"Grep the auth log for 500s in the past 10 min and group by route."*

…and get back tokenized results that say `<EMAIL_001>` instead of `alice@example.com`. Later wants to replay the agent's session with real values to verify what was found.

## Architecture

- **Glance binary on the engineer's laptop.** Zero server-side install required at v1 — adoption-blocker for devs without prod-install rights.
- **MCP server is the primary surface** (stdio). Agent connects, calls tools, gets pseudonymized results. CLI is secondary, for humans to dry-run, replay sessions, configure profiles.
- **Glance owns connections + creds.** Reads from existing tooling (`.env`, Doppler, Vault, `~/.ssh/config`). Agent never sees raw connection strings.
- **Powered by Gaze engine** (closed-source dependency) for pseudonymization, audit log, restore manifest.
- **Pluggable spine** — source trait + frontend trait + shared session/manifest core. v1 fills DB+log sources behind an MCP frontend; v1.x adds new sources/frontends additively.

### Key v1 design constraint

Session-core lifecycle is decoupled from MCP-stdio process lifecycle. This lets v1.x add a long-running daemon mode (for SDK push ingest) without rewriting session/audit/restore.

## v1 sources (cut tight)

1. **DB queries** — sqlx-backed (MySQL / Postgres / SQLite). Read-only (SELECT / EXPLAIN). MCP tools: `query`, `schema`, `list_tables`.
2. **App logs** — plain file tail / grep over SSH. Glance shells out to `ssh user@host tail -n 500 /var/log/app.log` (or `grep`), streams stdout, pseudonymizes per Gaze policy. MCP tools: `log_tail`, `log_grep`.

## Audit + restore

- Every MCP call writes to a local SQLite manifest (Gaze audit schema v2).
- Human-only `glance replay <session>` walks the log and restores tokens to real values for after-the-fact review.
- No write path for the agent. Restore is operator-only, gated behind CLI invocation on the engineer's machine.

## Anti-features (locked)

- Not a DB GUI / admin tool. No web UI, no Sequel Pro replacement.
- Not a wire-protocol DB proxy. Apps don't connect through Glance; only CLI / MCP / lib callers do.
- Not a credential vault. Reuses existing secret tooling.
- Not an ACL replacement. Assumes the DB user is already scoped read-only at the database level.
- No mutations at v1.
- No server-side install required at v1. Standard SSH + DB conn from laptop.

## v1.x roadmap (not v1)

- Nginx / Apache log format-aware parsing.
- `systemd` journal (`glance journal --unit ...`).
- Structured JSON log parsing.
- **Optional server-side adapter** for adopters who *can* install on prod — PII never leaves prod boundary unredacted. Pluggable transport designed for from day one.
- DML / writes (deferred — see open Q5).

## v2 roadmap (within 12 months of v1 GA)

- **SDK ingest** — apps call `glance::dump($context)` from Laravel / Rails / JS / Python; payload streams to local Glance daemon over HTTP; agent queries `recent_events --since 5m` via MCP.
  - Wedge: async / queue / multi-process debug + pseudonymized timeline.
  - Glance becomes a long-running daemon at this point. Source-trait abstraction + decoupled session lifecycle make this additive, not rewrite.
- This is the closest Glance gets to a Ray-style debugging companion. **No human GUI** — the consumer of pushed events is the agent, not human eyeballs.

## Explicitly out of roadmap

- **Desktop GUI** (Spatie Ray-style). Decided 2026-04-26: agent-first product, no GUI unless a real human-eyeballs use case surfaces. Stays out of competitive lane with Ray.
- **Wire-protocol proxy** (mysql_proxy / pgbouncer-style). Different product, different tech, not the moat.

## Open questions

1. **Profile config shape.** Per-project file (`.glance.toml`, checked in, secrets via env) or per-user (`~/.glance/profiles.toml`) or both? Recommend: both, project file wins.
2. **Schema introspection privacy.** Does `glance schema` itself need pseudonymization? Column names like `customer_email_unhashed` can leak business model intel.
3. **Default Gaze policy.** Does Glance ship with a sensible PII policy out of the box, or force adopters to author `policy.toml` before first use?
4. **SSH access model.** Reuse `~/.ssh/config` + SSH agent (zero net-new) or own connection profile with explicit auth? Recommend: reuse SSH config.
5. **DML path (v2).** When writes land — agents see-then-write tokens (auto-restore on write) or write tokens that get rejected by Gaze guardrails?
6. **Pluggable transport API.** v1 ships laptop-only, but design needs an extension point for a future server-side adapter. Lock the trait shape now or defer?
7. **Pricing / licensing model.** Defer-decided 2026-04-26. Build OSS-shaped, monetize later (likely paid hosted audit log + team features).
8. **Reuse vs rewrite from `reference/debug-proxy/`.** debug-proxy has working MCP + sqlx + gaze integration for MySQL. Mine specific pieces (MCP server scaffolding, sqlx + gaze redaction loop) vs design fresh against the pluggable spine. Decide before v1 implementation kicks off.

## Provenance

- Extracted from the `crates/debug-proxy/` crate inside the Gaze monorepo (kept as `reference/debug-proxy/` for evaluation).
- Spec authored via `/interview-me` + `/grill-me` session 2026-04-26 with Krishan.
- Architectural decisions mirrored to MemPalace under `wing_architect`.
