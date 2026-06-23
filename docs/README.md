# gaze-lens documentation

`gaze-lens` is a PII-safe read-access tool for live production investigation by
AI agents, built on the Gaze pseudonymization engine. An agent calls one of 5
MCP tools; gaze-lens redacts the result through Gaze before returning anything
and writes a tokenized audit row to a local SQLite manifest — later you `replay`
the session locally to see the original values.

## Tutorials

- [Getting started](tutorials/getting-started.md) — install → `init` a profile →
  run your first redacted query → `replay` it to see the originals.

## How-to

- [Configure profiles](how-to/configure-profiles.md) — project vs user files, source kinds, secrets.
- [Set up a production profile](how-to/set-up-production-NER.md) — the `production = true` NER mandate (fail-closed, no name leaks).
- [Search logs](how-to/search-logs.md) — local or SSH log profiles, `log_tail`, `log_grep` (regex), and the opt-in `keyword` mode.
- [Replay a session](how-to/replay-a-session.md) — restore original values locally + read the restore telemetry.
- [Wire up MCP clients](how-to/wire-up-mcp-clients.md) — Claude Code, Cursor, Codex, generic MCP.

## Reference

- [Product spec](reference/spec.md) — the locked surface, threat model, anti-features, roadmap (the source of truth).
- [CLI](reference/cli.md) — the 6 subcommands and their flags.
- [MCP tools](reference/mcp-tools.md) — the 5 tools and their arguments.
- [Profile schema](reference/profile-schema.md) — `profiles.toml` fields.
- [Policy schema](reference/policy-schema.md) — redaction policy TOML (`[ner]`, column rules, log strip).
- [Architecture](reference/architecture.md) — the implementer spine, core traits, session/manifest flow.

## Explanation

- [Pseudonymization and replay](explanation/pseudonymization-and-replay.md) — why tokens are reversible only locally.
- [Threat model](explanation/threat-model.md) — what gaze-lens defends against, and the residual risks it does not.
- [Cross-platform roadmap](explanation/cross-platform-roadmap.md) — why prebuilt binaries are Apple Silicon–only for now.

---

New here? The top-level [README](../README.md) has install + a quickstart.
Contributors: see [CONTRIBUTING.md](../CONTRIBUTING.md).
