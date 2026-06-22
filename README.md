# gaze-lens

[![CI](https://github.com/EmpireTwo/gaze-lens/actions/workflows/ci.yml/badge.svg)](https://github.com/EmpireTwo/gaze-lens/actions/workflows/ci.yml)
[![Release](https://github.com/EmpireTwo/gaze-lens/actions/workflows/release.yml/badge.svg)](https://github.com/EmpireTwo/gaze-lens/actions/workflows/release.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](./LICENSE)

PII-safe read-access for live production investigation by AI agents.

`gaze-lens` lets a developer point their LLM agent at a production database or app log during an incident and get back **pseudonymized** results — `<EMAIL_001>` instead of `alice@example.invalid` — while a local audit manifest records every retrieval. The engineer can later replay the agent's session locally to see the original values. Built on the [Gaze](https://github.com/EmpireTwo/gaze) pseudonymization engine; part of the [EmpireTwo](https://github.com/EmpireTwo) `gaze-X` product family.

> **Status:** v0.5.x. The public surface is locked: **5 MCP tools** (`query`, `schema`, `list_tables`, `log_tail`, `log_grep`) and **6 CLI subcommands** (`serve`, `init`, `query`, `replay`, `check`, `demo`). Log profiles can read remote logs over SSH (`ssh_log`) or a local file path (`local_log`) through the same `log_tail` / `log_grep` tools. The Gaze runtime crates (`gaze-pii`, `gaze-recognizers`, `gaze-mcp-core`) resolve from crates.io at `0.11`.

## Quick start

Download the latest prebuilt Apple Silicon macOS binary and run the built-in demo — it tokenizes a small canned dataset and restores it inline in one process, writing nothing to `~/.gaze-lens/`:

```sh
curl -L https://github.com/EmpireTwo/gaze-lens/releases/latest/download/gaze-lens-aarch64-apple-darwin.tar.xz | tar -xJ
./gaze-lens demo
```

Not on Apple Silicon, or want the full first-query-and-replay loop? Start with the [getting-started tutorial](./docs/tutorials/getting-started.md), which also covers building from source.

## Documentation

See the [docs hub](./docs/README.md) for the full map.

### Tutorials
- [Getting started](./docs/tutorials/getting-started.md) — install, run `demo`, then your first real redacted query and its replay (~10 min). **Start here.**

### How-to
- [Configure profiles](./docs/how-to/configure-profiles.md) — define a source, policy, and schema posture.
- [Wire up MCP clients](./docs/how-to/wire-up-mcp-clients.md) — connect Claude Code, Codex, or Cursor.
- [Search app logs](./docs/how-to/search-logs.md) — local or SSH log profiles, `log_tail` / `log_grep`, regex vs keyword mode.
- [Replay a session](./docs/how-to/replay-a-session.md) — recover original values and tune snapshot retention.
- [Set up production NER](./docs/how-to/set-up-production-NER.md) — the model-backed `production = true` profile tier.

### Reference
- [Product spec](./docs/reference/spec.md) — the locked v1 product contract, threat model, anti-features, roadmap.
- [Architecture](./docs/reference/architecture.md) — implementer spine, core traits, session/manifest flow, locked design decisions.
- [CLI](./docs/reference/cli.md) — every subcommand, flag, and exit behavior.
- [MCP tools](./docs/reference/mcp-tools.md) — the 5 tools and their argument schemas.
- [Profile schema](./docs/reference/profile-schema.md) — profile fields, project vs user precedence, schema policy.
- [Policy schema](./docs/reference/policy-schema.md) — redaction policy TOML.

### Explanation
- [Pseudonymization and replay](./docs/explanation/pseudonymization-and-replay.md) — why tokens are reversible only locally.
- [Threat model](./docs/explanation/threat-model.md) — what gaze-lens defends against, and the residual risks it does not.
- [Cross-platform roadmap](./docs/explanation/cross-platform-roadmap.md) — why prebuilt binaries are Apple Silicon-only for now.

Contributors: see [CONTRIBUTING.md](./CONTRIBUTING.md) for the dev workflow, the crates.io Gaze dependency pin, the sqlx macro ban, and PR review routing.

## Security

`gaze-lens` defends against raw production data reaching the LLM, SQL string-injection, SSH command injection, operator-error retrieval bypass, and schema-name leak. It assumes the operator's laptop disk is encrypted (FileVault / LUKS), the DB user is read-only at the database side, and snapshot files are not auto-uploaded to cloud backups. Full threat model and locked anti-features: [docs/reference/spec.md](./docs/reference/spec.md#threat-model) and [docs/explanation/threat-model.md](./docs/explanation/threat-model.md).

Report suspected vulnerabilities privately; do not open public issues for security reports. See [SECURITY.md](./SECURITY.md).

## License

Apache-2.0. See [LICENSE](./LICENSE) and `Cargo.toml` package metadata.
