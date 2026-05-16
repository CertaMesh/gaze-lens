# gaze-lens

[![CI](https://github.com/EmpireTwo/gaze-lens/actions/workflows/ci.yml/badge.svg)](https://github.com/EmpireTwo/gaze-lens/actions/workflows/ci.yml)
[![Release](https://github.com/EmpireTwo/gaze-lens/actions/workflows/release.yml/badge.svg)](https://github.com/EmpireTwo/gaze-lens/actions/workflows/release.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](./LICENSE)

PII-safe read-access for live production investigation by AI agents.

`gaze-lens` lets a developer point their LLM agent at a production database or app log during an incident and get back **pseudonymized** results — `<EMAIL_001>` instead of `alice@example.invalid`. The engineer can later replay the agent's session locally to see the original values.

Built on the [Gaze](https://github.com/EmpireTwo/gaze) pseudonymization engine. Part of the [EmpireTwo](https://github.com/EmpireTwo) product family — every product in the family is named `gaze-X`.

> **Status:** v0.3.0 is the latest tagged release. The repository may still be private while public-readiness work is in flight; badges and links are written for the intended public GitHub location and may require repository access until visibility changes. The public surface remains locked: 5 MCP tools and 6 CLI subcommands. v0.3.0 moves the internal MCP chokepoint onto `gaze-mcp-core::PiiEnvelope` and switches the Gaze runtime to crates.io. See [SPEC.md](./SPEC.md) for the locked product spec, [ARCHITECTURE.md](./ARCHITECTURE.md) for the implementer spine, [CONTRIBUTING.md](./CONTRIBUTING.md) for dev workflow, and [SECURITY.md](./SECURITY.md) for vulnerability reporting.

## Why

Today, when an engineer wants their AI agent to investigate prod, they have two bad options:

1. Give the agent raw access — and leak names / emails / addresses to the model provider.
2. Give the agent nothing — and waste an hour eyeballing psql while the incident burns.

`gaze-lens` is the third option: **pseudonymized agent access with auditable, reversible token mapping**.

## Install

### Apple Silicon macOS quick install

> Prebuilt binaries currently target Apple Silicon macOS (`aarch64-apple-darwin`). Other platforms should build from source until the native ONNX Runtime distribution blocker is resolved.

```sh
curl -L https://github.com/EmpireTwo/gaze-lens/releases/download/v0.3.0/gaze-lens-aarch64-apple-darwin.tar.xz | tar -xJ
./gaze-lens demo
```

`gaze-lens demo` tokenizes a small canned dataset (3 emails, 2 phones, 1 SSN-shaped string) and inline-restores it in a single process — both sections print side by side. The demo writes nothing to `~/.gaze-lens/`; everything lives in a tempdir that is wiped on exit. No follow-up `gaze-lens replay <id>` is required.

The v0.3.0 tarball above ships an Apple Silicon (`aarch64-apple-darwin`) binary.

### Prebuilt binaries

Releases attach prebuilt tarballs to the [GitHub releases page](https://github.com/EmpireTwo/gaze-lens/releases). Near-term release automation intentionally builds only the Apple Silicon macOS archive. Intel macOS, Linux, and Windows remain source-build or future binary targets while the Gaze recognizer backend is made portable.

### Building from source

```sh
git clone https://github.com/EmpireTwo/gaze-lens
cd gaze-lens
cargo build --release
./target/release/gaze-lens demo
```

The `gaze`, `gaze-recognizers`, and `gaze-mcp-core` crates are wired from crates.io. `gaze` aliases the published `gaze-pii` package so existing imports stay stable. See [CONTRIBUTING.md](./CONTRIBUTING.md#gaze-dependency-pin) for the pin policy and the local-checkout patch recipe.

`gaze-lens` builds with stable Rust 1.89+.

On Linux, install the platform packages needed by the native keyring backend before building, for example `pkg-config` and `libdbus-1-dev` on Debian/Ubuntu.

## New Project Onboarding

After `gaze-lens demo` confirms the binary works, run guided init from the project you want your agent to investigate:

```sh
# 1. Create a profile and optional agent config.
gaze-lens init --profile prod

# 2. Validate profile parsing, policy, redaction, and source connectivity.
gaze-lens check --profile prod

# 3. Dry-run one human query through the same audit + redaction path as MCP.
gaze-lens query --profile prod --table users --limit 5
# Use compact JSON for scripts.
gaze-lens query --profile prod --table users --limit 5 --format json

# 4. Later, replay an agent session locally to restore original values.
gaze-lens replay <session_ulid>
```

`init` is the preferred setup path. It prompts for source kind (`mysql`, `postgres`, `sqlite`, or `ssh-log`), connection details, where to write the profile, optional Claude Code `.mcp.json` setup, and whether to append an AGENTS.md primer. Codex and Cursor MCP config files are written only when supplied explicitly with repeatable `--client codex` or `--client cursor` flags. For Laravel-style SSH targets, it can inspect an explicit remote `.env` path and guide you toward a read-only credential instead of copying production app secrets blindly.

By default, interactive init can write:

- `.gaze-lens.toml` in the project or `~/.gaze-lens/profiles.toml` for user-level profiles.
- `.mcp.json` for Claude Code, or `~/.codex/config.toml` / `.cursor/mcp.json` when `--client codex` or `--client cursor` is supplied.
- An AGENTS.md snippet telling future agents to call the 5 locked MCP tools with a `profile` argument.

`check` is the gate before giving an agent access. It validates the profile and source without writing a manifest row or snapshot. Add `--explain-risk` when you want a structured trust report for the profile.

`gaze-lens` ships six CLI subcommands: `serve`, `init`, `query`, `replay`, `check`, `demo`. See [docs/profiles.md](./docs/profiles.md) for profile schema and [docs/replay.md](./docs/replay.md) for replay + snapshot handling.

## Use it from your agent

The primary surface is the MCP server (stdio). Prefer `gaze-lens init`; it can write the right MCP config for Claude Code, Codex, or Cursor.

Manual MCP config is still small. Use one `gaze-lens` server entry:

```jsonc
{
  "mcpServers": {
    "gaze-lens": {
      "command": "gaze-lens",
      "args": ["serve"]
    }
  }
}
```

The server loads configured profiles. `serve --profile prod` remains available as a one-profile restrict-list, and `serve --profile prod --profile staging` exposes only those profiles. Every MCP tool call must still pass `profile: "prod"` or another configured profile name.

Example tool call:

```json
{"tool": "query", "args": {"profile": "prod", "table": "users", "limit": 5}}
```

The agent sees five tools and nothing else:

| Tool | Purpose |
|---|---|
| `query` | Run a canned structured DB query (no raw SQL accepted). |
| `schema` | Describe one raw configured table schema. |
| `list_tables` | List table names, raw by default. |
| `log_tail` | Tail a configured SSH log source. |
| `log_grep` | Search a configured SSH log source. |

Every tool result routes through `Session::dispatch_tool` before it leaves the process. Under the hood, `dispatch_tool` builds a `gaze_mcp_core::PiiEnvelope` whose sealed `ToolCtx` makes the redact→manifest→return ordering a compile-time invariant rather than a hand-rolled runtime guard. Tool args are tokenized through the Gaze path before the manifest is written, so the manifest never stores raw arguments. Schema/list output shows raw table and column names by default; set `schema_tokenize = true` in the profile when schema names themselves are sensitive. In tokenized mode, `schema_allowlist` is presentation-only: it keeps selected labels raw but does not grant query access, and canned queries still use raw configured table and column names. Restart or reload the MCP server after profile edits.

## Threat model — short version

`gaze-lens` defends against raw production data reaching the LLM, SQL string-injection, SSH command injection, operator-error retrieval bypass, and schema-name leak. It assumes the operator's laptop disk is encrypted (FileVault / LUKS), the DB user is read-only at the database side, SSH credentials are managed by the OS, and snapshot files are not auto-uploaded to cloud backups.

Full threat model + locked anti-features in [SPEC.md §Threat model](./SPEC.md#threat-model).

## Sources (v1)

- **Database** — MySQL, Postgres, SQLite via sqlx. Read-only. Canned structured queries only — no raw SQL strings in v1.
- **App logs** — file `tail` / `grep` over SSH. No server-side install required; gaze-lens shells out from the laptop.

## Reference

`reference/debug-proxy/` is the predecessor crate (extracted from the Gaze monorepo). Used as a mining source during v1 implementation, not part of the active build.

## Security

Report suspected vulnerabilities privately; do not open public issues for security reports. See [SECURITY.md](./SECURITY.md).

## License

Apache-2.0. See [LICENSE](./LICENSE) and `Cargo.toml` package metadata.
