# gaze-lens

PII-safe read-access for live production investigation by AI agents.

`gaze-lens` lets a developer point their LLM agent at a production database or app log during an incident and get back **pseudonymized** results — `<EMAIL_001>` instead of `alice@example.com`. The engineer can later replay the agent's session locally to see the original values.

Built on the [Gaze](https://github.com/PIInuts) pseudonymization engine. Part of the [PIInuts](https://github.com/PIInuts) product family — every product in the family is named `gaze-X`.

> **Status:** v0.2.0 is the latest tagged release; `main` is preparing v0.2.2. The public surface remains locked: 5 MCP tools and 6 CLI subcommands. See [SPEC.md](./SPEC.md) for the locked product spec, [ARCHITECTURE.md](./ARCHITECTURE.md) for the implementer spine, and [CONTRIBUTING.md](./CONTRIBUTING.md) for dev workflow.

## Why

Today, when an engineer wants their AI agent to investigate prod, they have two bad options:

1. Give the agent raw access — and leak names / emails / addresses to the model provider.
2. Give the agent nothing — and waste an hour eyeballing psql while the incident burns.

`gaze-lens` is the third option: **pseudonymized agent access with auditable, reversible token mapping**.

## Install

### One-line first-run (try the demo)

> Prebuilt binaries currently target Apple Silicon macOS (`aarch64-apple-darwin`). Other platforms should build from source until the native ONNX Runtime distribution blocker is resolved.

```sh
curl -L https://github.com/PIInuts/gaze-lens/releases/download/v0.2.0/gaze-lens-aarch64-apple-darwin.tar.xz | tar -xJ
./gaze-lens demo
```

`gaze-lens demo` tokenizes a small canned dataset (3 emails, 2 phones, 1 SSN-shaped string) and inline-restores it in a single process — both sections print side by side. The demo writes nothing to `~/.gaze-lens/`; everything lives in a tempdir that is wiped on exit. No follow-up `gaze-lens replay <id>` is required.

The v0.2.0 tarball above ships an Apple Silicon (`aarch64-apple-darwin`) binary.

### Prebuilt binaries

Releases attach prebuilt tarballs to the [GitHub releases page](https://github.com/PIInuts/gaze-lens/releases). Near-term release automation intentionally builds only the Apple Silicon macOS archive. Intel macOS, Linux, and Windows remain source-build or future binary targets while the Gaze recognizer backend is made portable.

### Building from source

```sh
git clone https://github.com/PIInuts/gaze-lens
cd gaze-lens
cargo build --release
./target/release/gaze-lens demo
```

The `gaze` and `gaze-recognizers` crates are wired as git dependencies pinned to a `PIInuts/gaze` tag (currently `v0.6.4`). See [CONTRIBUTING.md](./CONTRIBUTING.md#gaze-dependency-pin) for the pin policy and the local-checkout patch recipe.

`gaze-lens` builds with stable Rust 1.89+.

On Linux, install the platform packages needed by the native keyring backend before building, for example `pkg-config` and `libdbus-1-dev` on Debian/Ubuntu.

## Quickstart

After running `gaze-lens demo` to confirm the install works, point it at a real source:

```sh
# 1. Scaffold a project profile next to your repo and a user-local transport file.
gaze-lens init --profile prod

# 2. Validate profile parsing, policy, Gaze pipeline, and source connectivity.
#    No manifest or snapshot side effects.
gaze-lens check --profile prod

# 3. Dry-run a canned query as a human (same audit + redaction path as MCP).
gaze-lens query --profile prod --table users --limit 5

# 4. Restore the tokenized arguments of a recorded session locally.
gaze-lens replay <session_ulid>
```

`gaze-lens` ships six CLI subcommands: `init`, `check`, `query`, `replay`, `serve`, `demo`. See [docs/profiles.md](./docs/profiles.md) for profile schema and [docs/replay.md](./docs/replay.md) for replay + snapshot handling.

## Use it from your agent

The primary surface is the MCP server (stdio). Wire `gaze-lens serve` into any MCP-capable agent (Claude Code, Cursor, Codex, custom):

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

`serve --profile prod` remains available when you want to expose only one
profile, but MCP tool calls must still pass `profile: "prod"`. The agent sees
five tools and nothing else:

| Tool | Purpose |
|---|---|
| `query` | Run a canned structured DB query (no raw SQL accepted). |
| `schema` | Describe one tokenized table schema. |
| `list_tables` | List tokenized table names. |
| `log_tail` | Tail a configured SSH log source. |
| `log_grep` | Search a configured SSH log source. |

Every tool result routes through `gaze::Pipeline::redact` before it leaves the process. Tool args are tokenized through the same path before the manifest is written, so the manifest never stores raw arguments.

## Threat model — short version

`gaze-lens` defends against raw production data reaching the LLM, SQL string-injection, SSH command injection, operator-error retrieval bypass, and schema-name leak. It assumes the operator's laptop disk is encrypted (FileVault / LUKS), the DB user is read-only at the database side, SSH credentials are managed by the OS, and snapshot files are not auto-uploaded to cloud backups.

Full threat model + locked anti-features in [SPEC.md §Threat model](./SPEC.md#threat-model).

## Sources (v1)

- **Database** — MySQL, Postgres, SQLite via sqlx. Read-only. Canned structured queries only — no raw SQL strings in v1.
- **App logs** — file `tail` / `grep` over SSH. No server-side install required; gaze-lens shells out from the laptop.

## Reference

`reference/debug-proxy/` is the predecessor crate (extracted from the Gaze monorepo). Used as a mining source during v1 implementation, not part of the active build.

## License

Apache-2.0. See `Cargo.toml` for the package metadata.
