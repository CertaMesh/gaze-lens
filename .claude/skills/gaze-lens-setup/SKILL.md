---
name: gaze-lens-setup
description: Install and configure gaze-lens (PII-safe read-access tool for live production investigation by AI agents) in a target project. Walks through binary install, profile init, SSH .env discovery, MCP wiring for Claude Code / Codex / Cursor, and first verified query. Use when the user asks to "set up gaze-lens", "install gaze-lens", or "wire gaze-lens to my project".
---

# gaze-lens Setup

Use this skill when a user asks to set up `gaze-lens` for a project. Goal: create a working `.gaze-lens.toml` profile, configure the user's MCP client, verify the trust report, and run one safe first query without exposing secrets.

## 1. Verify Local Install

Check whether `gaze-lens` is installed and which version is available.

```sh
command -v gaze-lens
gaze-lens --version
```

Compare the installed version with the latest release in the source repository's `CHANGELOG.md` or GitHub releases. Current latest in this repo at authoring time is `v0.4.1`.

If the binary exists but is older than the latest release, tell the user before replacing it.

## 2. Install If Missing

Apple Silicon is the current binary distribution target.

```sh
version="0.4.1"
curl -L "https://github.com/EmpireTwo/gaze-lens/releases/download/v${version}/gaze-lens-aarch64-apple-darwin.tar.xz" | tar -xJ
sudo mv gaze-lens-aarch64-apple-darwin/gaze-lens /usr/local/bin/gaze-lens
```

Other platforms do not have published first-class binary artifacts yet. Build from source only if the user accepts that path:

```sh
git clone https://github.com/EmpireTwo/gaze-lens.git
cd gaze-lens
cargo build --release --bin gaze-lens
```

## 3. Choose Project Root

Run setup from the application root that owns the live `.env`.

For Laravel projects, the correct root usually contains both files:

```sh
test -f artisan && test -f .env
```

Do not run setup from sibling docs, marketing, dashboard, or support folders unless that folder is the actual app root.

## 4. Run `gaze-lens init`

Always check help before writing commands, because old docs used stale flag names.

```sh
gaze-lens init --help
```

Current `v0.4.1` init flags include:

```text
--profile <profile>
--source-kind <mysql|postgres|sqlite|ssh-log>
--scope <project|user|project-auto-purge>
--source-host <host>
--source-port <port>
--source-database <database>
--source-username <username>
--source-password-env <env-var>
--secret-backend <env|keyring>
--source-password-keyring-service <service>
--source-password-keyring-account <account>
--no-keyring-write
--source-ssh-host <jump-host>
--source-local-port <port>
--source-path <path>
--source-json-text-columns <columns>
--client <codex|claude-code|cursor>
--no-mcp-config
--no-agents-md
--also-claude-md
--allow-overwrite
--non-interactive
--print-only
--write-all
--smoke-check
--discover-ssh-host <host>
--discover-env-path <remote-path-to-.env>
--accept-prod-rw <host>
--allow-new-ssh-host
```

Use `--source-kind`, `--discover-ssh-host`, and `--discover-env-path`. Do not use stale names such as `--source`, `--ssh-env-host`, or `--ssh-env-path`.

Prefer interactive setup unless the user explicitly needs automation.

```sh
gaze-lens init \
  --profile prod \
  --source-kind mysql \
  --scope project \
  --client codex \
  --client claude-code \
  --client cursor
```

For non-interactive DB setup with an environment variable secret:

```sh
gaze-lens init \
  --profile prod \
  --source-kind mysql \
  --scope project \
  --source-host db.example.internal \
  --source-port 3306 \
  --source-database app \
  --source-username readonly_user \
  --source-password-env GAZE_LENS_PROD_DB_PASSWORD \
  --client codex \
  --non-interactive \
  --write-all
```

For SQLite:

```sh
gaze-lens init \
  --profile local \
  --source-kind sqlite \
  --scope project \
  --source-path ./database/database.sqlite \
  --client codex
```

## 5. Use SSH `.env` Discovery Carefully

Laravel/Ploi projects often need one-time `.env` discovery over SSH:

```sh
gaze-lens init \
  --profile prod \
  --scope project \
  --discover-ssh-host ploi@example.com \
  --discover-env-path /home/ploi/site/.env \
  --client codex
```

Strict host-key checking is default. Only add `--allow-new-ssh-host` when the user accepts first-contact trust-on-first-use.

Discovery choices:

- Path A: Store discovered production credential. Requires explicit consent. In non-interactive mode use `--accept-prod-rw <host>`, where value exactly matches `--discover-ssh-host`.
- Path B: Recommended. Use discovered host/database metadata, then configure a separate read-only database credential.
- Path C: Abort without writing credentials.

## 6. Handle Secrets Safely

Prefer a read-only database user. Never paste production passwords into chat.

Preferred secret backends:

- `--secret-backend keyring` for operator-managed local credentials.
- `--secret-backend env` plus `--source-password-env <ENV_VAR>` for environment-managed credentials.

For non-interactive keyring profiles, the operator must create the keyring entry first:

```sh
gaze-lens init \
  --profile prod \
  --source-kind mysql \
  --scope project \
  --source-host db.example.internal \
  --source-database app \
  --source-username readonly_user \
  --secret-backend keyring \
  --source-password-keyring-service gaze-lens-prod \
  --source-password-keyring-account readonly_user \
  --no-keyring-write \
  --non-interactive \
  --write-all
```

Plaintext secret fallback is not part of the normal setup path. If a build or prompt exposes an explicit plaintext opt-in such as `--allow-plaintext`, stop and get explicit user consent before using it.

## 7. Verify With `check`

Run `check` before any query.

```sh
gaze-lens check --profile prod --explain-risk
```

Review the trust report. Confirm source class, redaction policy, snapshot location, and MCP surface look expected.

## 8. Run First Query

Start with a small row limit against a known table.

```sh
gaze-lens query --profile prod --table users --limit 5
```

Returned values are tokenized/redacted. Original values stay local and require replay.

## 9. Restart MCP Client

`gaze-lens init` writes per-agent snippets, including `agents_snippet.md`. Use those generated snippets as source of truth.

Claude Code:

```text
/mcp restart
```

Codex:

```sh
codex mcp restart gaze-lens
```

Cursor:

Restart Cursor or reload its MCP server from settings after applying the generated config.

If setup skipped MCP writes, copy the generated snippet for the user's client from `agents_snippet.md` into that client's MCP config.

## 10. Replay Basics

Use replay only on the operator's machine:

```sh
gaze-lens replay <session_ulid>
```

Replay restores original values from local snapshot files under `~/.gaze-lens/snapshots/`. Those files are operator-only and must not be shared with agents, pull requests, logs, tickets, or chat.

## Pitfalls

- Old README, blog, or copied notes may use stale flags. Check `gaze-lens init --help` first and use current names.
- Project-root confusion is common in Laravel apps. A folder named `dashboard/` may be PHP docs or a sibling shell, not the Laravel app. Look for `artisan` and `.env`.
- Claude-only Ploi agent contexts may already include the SSH host and `.env` path. Check those notes before asking the user to rediscover them.
