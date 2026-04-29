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

## MySQL DATETIME

MySQL `DATETIME` has no timezone. gaze-lens normalizes timezone-less MySQL datetime values to UTC RFC3339 strings by default, preserving typed `LensValue::DateTime` semantics while avoiding a local-timezone dependency in audit output. Use source-side conversion when a deployment needs a different timezone interpretation.
