# Profile schema reference

Profiles configure the sources `gaze-lens` connects to and the PII policy applied to each. They are TOML, loaded from two files and merged. This page documents every field, its type, default, and merge rule.

For setup examples see [configure-profiles.md](../how-to/configure-profiles.md); for the policy file referenced by `policy` see [policy-schema.md](./policy-schema.md).

## File locations and precedence

Profiles are loaded from two TOML files and merged per profile name:

| File | Default path | Override |
|---|---|---|
| Project file | `.gaze-lens.toml` (cwd) | `--project-config` / `GAZE_LENS_PROJECT_CONFIG` |
| User file | `~/.gaze-lens/profiles.toml` | `--user-config` / `GAZE_LENS_USER_CONFIG` |

When both files define a profile of the same name, ownership is split so teams can commit PII policy while operators keep laptop-specific transport in their home directory:

| Field group | Owner | Fields |
|---|---|---|
| PII / schema policy | **Project** | `policy`, `schema_tokenize`, `schema_allowlist`, `database`, `username`, `secret`/`password_env`, `readonly_required` |
| Transport | **User** | `host`, `port`, `ssh_host`, `local_port`, sqlite `path`, local log `path` (when supplied) |

Special merge rules apply to `production`, `snapshot_retention_days`, and `auto_purge` (below).

## Profile table

Each profile is a `[[profiles]]` array entry with a `[profiles.source]` table.

```toml
[[profiles]]
name = "prod"
production = true
policy = "./gaze-policy.toml"
schema_tokenize = true
schema_allowlist = ["id", "created_at", "updated_at"]
snapshot_retention_days = 30
auto_purge = "warn"

[profiles.source]
kind = "mysql"
host = "db.example.invalid"
port = 3306
database = "app"
username = "gaze_ro"
secret = { type = "keyring", service = "gaze-lens", account = "prod" }
readonly_required = true
```

### Top-level fields

| Field | Type | Default | Merge | Description |
|---|---|---|---|---|
| `name` | string | (required) | key | Profile identifier; used as the `profile` argument in CLI/MCP calls. |
| `policy` | path | built-in fallback | project | Path to the `gaze-policy.toml` for this profile. Omitted = built-in fallback equivalent to an empty `[policy.database]` (non-sensitive fields preserved, default email detector enabled). |
| `schema_tokenize` | bool | `false` | project | Tokenize schema-name presentation in `schema`/`list_tables`. See [Schema presentation](#schema-presentation). |
| `schema_allowlist` | string[] | (none) | project | Labels kept raw when `schema_tokenize = true`. Presentation only. |
| `production` | bool | `false` | escalate-only | Marks the profile as pointing at production data; mandates an NER model. See [Production tier](#production-tier). |
| `snapshot_retention_days` | u32 | `None` (unlimited) | user-overrides-project | Snapshot TTL in days. See [Snapshot retention](#snapshot-retention). |
| `auto_purge` | string enum | `"off"` | least-destructive | Snapshot sweep policy: `"off"`, `"warn"`, `"purge"`. See [Snapshot retention](#snapshot-retention). |

### Provenance fields (written by `init` discovery)

When `init` reads a remote `.env` over SSH, it records provenance on the profile. These are descriptive metadata, not connection inputs:

| Field | Type | Description |
|---|---|---|
| `discovered_from_ssh_host` | string | SSH host the credential was discovered from. |
| `discovered_from_path` | path | Remote `.env` path read during discovery. |
| `discovered_at` | string | Discovery timestamp. |
| `discovered_ssh_host_key_fingerprint` | string | Host-key record pinned at discovery. |
| `credential_class` | string | Recorded credential class (e.g. read-only vs production-rw). |

## Source spec (`[profiles.source]`)

The `kind` tag selects the source variant. Source class (`database` vs `log`) is derived from `kind` and governs MCP tool compatibility — see [mcp-tools.md](./mcp-tools.md#source-class-compatibility).

### `kind = "mysql"` / `kind = "postgres"` (class: database)

| Field | Type | Default | Description |
|---|---|---|---|
| `host` | string | (required) | DB host. |
| `port` | u16 | (required) | DB port. |
| `database` | string | (required) | Database name. |
| `username` | string | (required) | DB username. |
| `password_env` | string | (none) | Env var holding the password (legacy backend). |
| `secret` | SecretSpec | (none) | Explicit secret backend (see [Database secrets](#database-secrets)). |
| `ssh_host` | string | (none) | SSH tunnel jump host. |
| `local_port` | u16 | (none) | SSH tunnel local forwarded port. |
| `readonly_required` | bool | `true` | Require the DB user to be read-only. |

Exactly one of `password_env` or `secret` must be set for MySQL/Postgres; both or neither is rejected at load time.

### `kind = "sqlite"` (class: database)

| Field | Type | Default | Description |
|---|---|---|---|
| `path` | path | (required) | SQLite database file path. |
| `readonly_required` | bool | `true` | Require read-only access. |
| `json_text_columns` | string[] | `[]` | Allowlist of TEXT columns parsed as JSON-in-TEXT (default-deny; columns not listed are treated as opaque TEXT). |

SQLite profiles have no password.

### `kind = "ssh_log"` (class: log)

| Field | Type | Default | Description |
|---|---|---|---|
| `host` | string | (required) | SSH host (`user@host` form). |
| `path` | string | (required) | Remote log file path. |

`ssh_log` profiles have no password; SSH auth is the operator's (`~/.ssh/config`, SSH agent). Command construction uses the fixed `ssh -- <host> <command> -- <quoted_path>` form with validated host arguments (`-`-prefixed hosts rejected).

### `kind = "local_log"` (class: log)

| Field | Type | Default | Description |
|---|---|---|---|
| `path` | path | (required) | Local log file path on the laptop running `gaze-lens`. |

`local_log` profiles have no password, host, or SSH fields. `gaze-lens` opens the configured file read-only through the local filesystem, applies the same line/byte caps, bounded grep window, redaction chokepoint, manifest-first ordering, regex/keyword semantics, and snapshot controls as `ssh_log`, and exposes it through the unchanged `log_tail` and `log_grep` tools. Because no shell command or remote argv is constructed, the SSH command-injection surface does not apply; operators are still responsible for pointing `path` at the intended file.

## Database secrets

Database profiles support exactly one password reference. `gaze-lens` never accepts a literal `password = "..."` field; that key is rejected before merge (including inline-table forms).

| Form | TOML | Backend |
|---|---|---|
| Legacy env | `password_env = "GAZE_LENS_DB_PASSWORD"` | environment variable |
| Explicit env | `secret = { type = "env", var = "GAZE_LENS_DB_PASSWORD" }` | environment variable |
| Keyring | `secret = { type = "keyring", service = "gaze-lens", account = "prod" }` | native OS keyring |

`SecretSpec` uses `deny_unknown_fields`. Existing `password_env` profiles parse unchanged; new profiles should prefer `secret` to make the backend explicit. Keyring profiles store only the `service`/`account` locator in TOML.

Keyring resolution works on macOS, Windows, and desktop Linux with an unlocked Secret Service provider. On bare servers, containers, headless Linux without DBus/Secret Service, or locked keyrings, use `password_env`; [`check`](./cli.md#check) reports the keyring backend as `BACKEND UNAVAILABLE` rather than synthesizing a DBus session.

## Schema presentation

`schema` and `list_tables` show raw table/column labels by default. Setting `schema_tokenize = true` tokenizes schema-name presentation; `schema_allowlist` then keeps the listed labels raw. Without `schema_tokenize = true`, `schema_allowlist` has no effect and `check` warns about it.

Schema presentation is **privacy only, not access control**. Canned queries always use raw configured table/column names and are governed by each column's `ColumnInfo.allowed` value (see [policy-schema.md](./policy-schema.md)). After editing a profile, restart or reload the MCP server so the lazy source is rebuilt.

## Session scope

Only `conversation` scope is supported in v0.1; `persistent` is reserved for v1.x. The Gaze session default is `Scope::Conversation(<lens_session_id>)`; `Scope::Ephemeral` is rejected at session construction because `Session::export()` rejects it. Cross-profile token correlation within one `serve` process is intentional (see [pseudonymization-and-replay.md](../explanation/pseudonymization-and-replay.md)).

## Production tier

`production = true` marks a profile as pointing at production data. A production profile **must** configure an NER model (`[ner].model_dir` in its policy file); otherwise `serve`, `query`, and `check` refuse to build the session with a clear error before any data is read. This closes the nested-JSON person-name leak class (regex-only redaction misses arbitrary person names). Because the Gaze runtime makes NER fail-closed, a misconfiguration fails closed at startup rather than leaking at query time.

`production` is **escalate-only** across the project/user merge: if either file marks the profile `production = true`, it is production. A user file cannot downgrade a project's mandate. The mandate is opt-in; non-production profiles are unaffected.

For log profiles (`ssh_log` or `local_log`), prefer `mode: "keyword"` on `log_grep` for sensitive or production-tier logs. The default remains `regex`; on `production = true` profiles, a regex `log_grep` emits a runtime warning because regex mode is a raw-text presence oracle. See [mcp-tools.md](./mcp-tools.md#log_grep) and [threat-model.md](../explanation/threat-model.md).

## Snapshot retention

Two opt-in fields govern snapshot lifecycle. Both default to v0.1 behavior (unlimited retention, manifest as the audit log of record) when unset.

### `snapshot_retention_days`

`Option<u32>`. `None`/unset = unlimited. When set to `n`, snapshots whose ULID-embedded creation timestamp is older than `n` days are eligible for a startup sweep. The age is derived from the ULID, not filesystem mtime, so it is deterministic across laptops.

**Merge:** standard user-overrides-project.

### `auto_purge`

A three-state string enum governing what the sweep does when it finds expired snapshots:

| Value | Behavior |
|---|---|
| `"off"` (default) | No sweep. No manifest open, no FS scan, no warning. |
| `"warn"` | Read-only scan; emits a per-day-suppressed stderr listing of what would be purged. A `tracing` debug event fires on every sweep. |
| `"purge"` | Best-effort `remove_file` on each expired snapshot, writes a `purged_at_ms` tombstone on the `calls` row (manifest row preserved, only `snapshot_ref` cleared), and emits a non-suppressed info-level count. |

Replay of a tombstoned row returns `LensError::SnapshotPurged` citing the policy — never a silent miss.

**Merge (least-destructive `min`).** Ordering is `off < warn < purge`. The merged value is `min(project, user)`: the user may opt **down** to a less destructive mode but can never escalate above what the project authorizes. A profile defined **only** in the user file is forced to `off` regardless of any value set there — destructive operational policy must be expressed at the team-shared (project) layer. `init` gates writing `auto_purge` to `--scope project-auto-purge`.

> The earlier v0.2 SPEC text described `auto_purge` as a boolean (`true`/`false`) with a plain-conjunction merge. The shipped surface is the three-state string enum above; the least-destructive `min` merge subsumes that conjunction (project sets the ceiling, user can only opt down).

### Multi-profile process bound

When `gaze-lens serve` loads multiple profiles, snapshot retention is bound by the **most-restrictive** policy across the loaded set, computed in addition to each profile's project×user merge:

- `snapshot_retention_days = min(days)` over the loaded set (`None` treated as +∞).
- `auto_purge` = least-destructive value across loaded profiles (`min` over `off < warn < purge`).

The snapshot directory is shared, so the sweep affects all profiles' replay; most-restrictive is the only safe boundary.

## `init` merge semantics

`gaze-lens init` is additive. The writer preserves unrelated `[[profiles]]` entries verbatim (including `auto_purge` strings, `schema_allowlist` arrays, and `policy` paths) via `toml_edit`, refuses to overwrite a same-name entry without `--allow-overwrite`/`--write-all`, skips writes when rendered TOML matches on-disk bytes, and writes atomically. See [cli.md](./cli.md#init) for flags and [configure-profiles.md](../how-to/configure-profiles.md) for usage.

## See also

- [policy-schema.md](./policy-schema.md) — the `gaze-policy.toml` referenced by `policy`.
- [cli.md](./cli.md) — `init`, `check`, `serve` flags and config file destinations.
- [mcp-tools.md](./mcp-tools.md) — required `profile` argument and source-class compatibility.
- [spec.md](./spec.md) — snapshot retention surface contract and threat model.
