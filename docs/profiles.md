# Profiles

Profiles are loaded from two TOML files:

- Project file: `.gaze-lens.toml` by default, override with `--project-config`.
- User file: `~/.gaze-lens/profiles.toml` by default, override with `--user-config`.

When both files define the same profile, the project file owns PII policy and schema policy, while the user file owns transport overrides. In practice:

- Project wins for `policy`, `schema_allowlist`, database name, username, secret reference, and read-only requirement.
- User wins for host, port, SSH host, local forwarded port, and local SQLite path when supplied.

This lets teams commit the PII policy while each operator keeps laptop-specific transport in their home directory.

## Session Policy

Only `conversation` scope is supported in v0.1. `persistent` is reserved for v1.x.

## Schema Policy

Schema names are presentation-sensitive metadata. `schema` and `list_tables` tokenize table and column names through the `schema_metadata` source class unless names are allowlisted.

Query access is separate from presentation tokenization. Canned queries are compiled only against columns whose `ColumnInfo.allowed` value is true. A displayed token or allowlisted label does not grant query access by itself.

Example:

```toml
[[profiles]]
name = "prod"
policy = "./gaze-policy.toml"
schema_allowlist = ["id", "created_at", "updated_at"]

[profiles.source]
kind = "mysql"
host = "prod-db.internal"
port = 3306
database = "app"
username = "gaze_ro"
password_env = "GAZE_LENS_DB_PASSWORD"
readonly_required = true
```

## Database Secrets

Database profiles support exactly one password reference:

- `password_env = "GAZE_LENS_DB_PASSWORD"` for the legacy environment-variable backend.
- `secret = { type = "env", var = "GAZE_LENS_DB_PASSWORD" }` for the explicit env backend form.
- `secret = { type = "keyring", service = "gaze-lens", account = "prod" }` for the native OS keyring backend.

Existing profiles that use `password_env` continue to parse unchanged. New profiles should prefer `secret` because it makes the selected backend explicit. A profile that sets both `password_env` and `secret`, or neither for a MySQL/Postgres source, is rejected at load time.

`gaze-lens` never accepts or renders a literal `password = "..."` profile field. Profile validation rejects that key before merge, including inline-table forms, so secret bytes do not land in `.gaze-lens.toml` or `~/.gaze-lens/profiles.toml`.

Keyring profiles store only the keyring locator in TOML:

```toml
[[profiles]]
name = "prod"
policy = "./gaze-policy.toml"
schema_allowlist = ["id", "created_at", "updated_at"]

[profiles.source]
kind = "postgres"
host = "prod-db.internal"
port = 5432
database = "app"
username = "gaze_ro"
secret = { type = "keyring", service = "gaze-lens", account = "prod" }
readonly_required = true
```

On macOS, Windows, and desktop Linux with an unlocked Secret Service provider, `secret.type = "keyring"` resolves through the platform keyring. On bare servers, containers, headless Linux sessions without DBus / Secret Service, or locked keyrings, use `password_env`; `check` reports the keyring backend as unavailable instead of synthesizing a DBus session.

## SSH `.env` Discovery

`init` can optionally read a deployed Laravel-style `.env` over SSH once during setup:

```sh
gaze-lens init --profile prod \
  --discover-ssh-host deploy@app01 \
  --discover-env-path /var/www/app/.env
```

Discovery is setup-time only. It reads the explicit path you provide, extracts flat `DB_*` keys, and never re-reads the remote file from MCP tools, CLI `query`, or any per-query path.

The interactive flow offers three choices:

- Path B, recommended and selected by default: keep discovered host, port, and database, then enter a separate readonly username and password for keyring storage.
- Path A: store the discovered production credential as-is in the keyring. Interactive use requires typing the discovered username back. Non-interactive use requires `--accept-prod-rw=<host>`, and the value must exactly match `--discover-ssh-host`.
- Path C: abort without materializing a profile or keyring write.

SSH runs with `BatchMode=yes`; load your key first, for example:

```sh
ssh-add ~/.ssh/id_ed25519
```

Strict host-key checking is on by default (`StrictHostKeyChecking=yes`). First contact should pin the host before running discovery:

```sh
ssh-keyscan -t ed25519 app01 >> ~/.ssh/known_hosts
```

For ephemeral lab hosts only, `--allow-new-ssh-host` opts into `StrictHostKeyChecking=accept-new`.

Discovery does not auto-probe alternate paths, run `docker exec` or `kubectl exec`, validate credentials through a remote database process, or parse framework-specific executable config. Remote stdout is capped at 64 KiB, SSH argv uses the repository-safe `ssh -- <host> <command> -- <path>` form with validated host/path values, and profile provenance records the SSH host, path, timestamp, host-key record, and credential class.

## Check

Run `check` before serving MCP or running a CLI query:

```sh
gaze-lens --project-config .gaze-lens.toml check --profile prod
```

`check` validates profile parsing, policy parsing, Gaze pipeline construction, secret backend reachability, and the source connection. Database profiles perform a read-only connection and table-list ping. SSH log profiles perform command-construction validation without tailing remote logs.

Secret validation prints a distinct status before the source ping:

- `secret: ok (...)` when the configured backend resolves.
- `secret: NOT FOUND (...)` when the referenced env var or keyring entry is absent.
- `secret: ACCESS DENIED (...)` when the keyring rejects access.
- `secret: BACKEND UNAVAILABLE (...)` when the platform keyring backend cannot be reached.

Secret status output names the backend and locator only; it never prints password bytes.

`check` has no manifest or snapshot side effects.

### Trust report (`--explain-risk`)

Use `check --explain-risk` for a local-only trust report that describes what the selected profile exposes and what residual risks remain:

```sh
gaze-lens --project-config .gaze-lens.toml check --profile prod --explain-risk
gaze-lens --project-config .gaze-lens.toml check --profile prod --explain-risk --format json
```

The trust report does not connect to the source, read keyring or env password values, invoke SSH, or write the manifest. It validates the profile, policy, and Gaze pipeline, then reports the input, process, output, at-rest, and operator handoff surfaces.

Text mode writes status lines and the report to stdout. JSON mode writes status lines to stderr and emits only one JSON object on stdout. The JSON object includes `report_version: 1`; the v1 field set is closed, so any field addition, removal, or rename requires `report_version: 2` with a deprecation note.

## Multi-profile MCP server

`gaze-lens serve` loads every configured profile by default and exposes them
through one MCP server entry. Each MCP tool call must include `profile` with one
of the loaded names:

```json
{"profile": "prod", "table": "users", "limit": 5}
```

Use `gaze-lens serve --profile prod --profile staging` to restrict a server
process to a subset. Existing `serve --profile prod` entries remain valid as a
one-profile restrict-list, but the MCP tool schemas still require `profile`.

Startup eagerly parses TOML, validates profile names, validates policy files,
and builds Gaze pipelines. Source connections are lazy: DB pools and SSH log
sources are created on first tool call for that profile and cached for the
process lifetime.

When multiple profiles are loaded, snapshot retention uses the most restrictive
loaded policy: minimum `snapshot_retention_days` and the least destructive
`auto_purge` value.

## Init merge semantics

`gaze-lens init` is additive by design. When a project or user profile file already exists, the init writer:

- **Preserves** unrelated `[[profiles]]` entries verbatim, including their `auto_purge` strings, `schema_allowlist` arrays, and `policy` paths. The TOML is merged via `toml_edit`, so comments and formatting on existing entries survive untouched.
- **Refuses** to overwrite an entry of the same name unless `--allow-overwrite` (or `--write-all`) is set. The renderer returns `RenderError::Collision`, which the CLI surfaces as a clear "rerun with --allow-overwrite" message.
- **Skips writes** when the rendered TOML matches the on-disk bytes (CB7 byte-compare). Rerunning with identical inputs prints `no changes` and exits 0.
- **Writes atomically**: each destination is staged as `<dest>.tmp.<pid>`, fsynced, renamed, and the parent directory fsynced. A failure mid-batch surfaces as `LensError::BatchPartial { applied, pending, failed, source }` so operators see what landed and what didn't.

`init` never writes a `password = "..."` line. Database connections rely on either `password_env` or `source.secret`; for keyring profiles the rendered file contains only `service` and `account`. When `init --secret-backend keyring` writes a password, it first performs a keyring round-trip preflight, refuses to replace an existing different entry unless `--allow-overwrite` (or `--write-all`) is set, verifies the written value with a read-back, and then writes the profile atomically. If the keyring write succeeds but the profile commit fails, `init` reports the orphaned keyring locator so the operator can rerun after fixing the file issue or delete the entry manually.

The rendered profile honors the project-vs-user role split documented above: `init --scope project` refuses to set transport overrides like `host` / `port` for security-sensitive defaults, and `init --scope user` refuses to set policy or `auto_purge`.

`auto_purge` is gated to `--scope project-auto-purge`. Any other scope leaves the rendered TOML without an `auto_purge` line (default = `off`). Project-only opt-in matches the merge rule documented in `src/profile.rs:325-341`: profiles defined ONLY in the user file are forced to `Off` regardless of any value set there.

## MySQL DATETIME

MySQL `DATETIME` has no timezone. gaze-lens normalizes timezone-less MySQL datetime values to UTC RFC3339 strings by default, preserving typed `LensValue::DateTime` semantics while avoiding a local-timezone dependency in audit output. Use source-side conversion when a deployment needs a different timezone interpretation.
