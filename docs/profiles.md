# Profiles

Profiles are loaded from two TOML files:

- Project file: `.gaze-lens.toml` by default, override with `--project-config`.
- User file: `~/.gaze-lens/profiles.toml` by default, override with `--user-config`.

When both files define the same profile, the project file owns PII policy and schema policy, while the user file owns transport overrides. In practice:

- Project wins for `policy`, `schema_allowlist`, database name, username, password env name, and read-only requirement.
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

## Check

Run `check` before serving MCP or running a CLI query:

```sh
gaze-lens --project-config .gaze-lens.toml check --profile prod
```

`check` validates profile parsing, policy parsing, Gaze pipeline construction, and the source connection. Database profiles perform a read-only connection and table-list ping. SSH log profiles perform command-construction validation without tailing remote logs.

`check` has no manifest or snapshot side effects.

## Init merge semantics

`gaze-lens init` is additive by design. When a project or user profile file already exists, the init writer:

- **Preserves** unrelated `[[profiles]]` entries verbatim, including their `auto_purge` strings, `schema_allowlist` arrays, and `policy` paths. The TOML is merged via `toml_edit`, so comments and formatting on existing entries survive untouched.
- **Refuses** to overwrite an entry of the same name unless `--allow-overwrite` (or `--write-all`) is set. The renderer returns `RenderError::Collision`, which the CLI surfaces as a clear "rerun with --allow-overwrite" message.
- **Skips writes** when the rendered TOML matches the on-disk bytes (CB7 byte-compare). Rerunning with identical inputs prints `no changes` and exits 0.
- **Writes atomically**: each destination is staged as `<dest>.tmp.<pid>`, fsynced, renamed, and the parent directory fsynced. A failure mid-batch surfaces as `LensError::BatchPartial { applied, pending, failed, source }` so operators see what landed and what didn't.

`init` never writes a `password = "..."` line. Database connections rely on `password_env`; the operator sets the env var separately. The rendered profile honors the project-vs-user role split documented above: `init --scope project` refuses to set transport overrides like `host` / `port` for security-sensitive defaults, and `init --scope user` refuses to set policy or `auto_purge`.

`auto_purge` is gated to `--scope project-auto-purge`. Any other scope leaves the rendered TOML without an `auto_purge` line (default = `off`). Project-only opt-in matches the merge rule documented in `src/profile.rs:325-341`: profiles defined ONLY in the user file are forced to `Off` regardless of any value set there.

## MySQL DATETIME

MySQL `DATETIME` has no timezone. gaze-lens normalizes timezone-less MySQL datetime values to UTC RFC3339 strings by default, preserving typed `LensValue::DateTime` semantics while avoiding a local-timezone dependency in audit output. Use source-side conversion when a deployment needs a different timezone interpretation.
