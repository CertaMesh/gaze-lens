# Policy schema reference

A profile's `policy` field points at a `gaze-policy.toml` file that configures the Gaze redaction pipeline: the NER model, per-column PII rules, log strip rules, and the default action for otherwise-unmatched detections. This page documents the policy file's structure.

A profile without a `policy` path uses a built-in fallback equivalent to an empty `[policy.database]` section (non-sensitive fields preserved, default email detector enabled). See [profile-schema.md](./profile-schema.md) for how `policy` is referenced and merged.

## File structure

```toml
[ner]
model_dir = "/opt/gaze/models/ner"
locale = "en"

[session]
scope = "conversation"

[policy]
default_action = "tokenize"

[policy.database]
[[policy.database.columns]]
column = "email"
class = "email"
action = "tokenize"

[policy.logs]
strip_patterns = ["(?i)authorization: bearer \\S+"]
action = "redact"
```

## `[ner]`

Named-entity recognition configuration.

| Field | Type | Default | Description |
|---|---|---|---|
| `model_dir` | path | (none) | Directory of the NER model. **Required** when the owning profile sets `production = true`. |
| `locale` | string | (none) | Locale hint for recognizers/validators. Unknown/absent locale increases safety-net coverage; it never loosens redaction. |

### Production NER mandate

A profile marked `production = true` MUST configure `[ner].model_dir`. Enforcement runs at session build (`enforce_production_ner`): `serve`, `query`, and `check` refuse to construct the session, before any data is read, with:

```text
profile `<name>` is marked `production = true` but its policy has no `[ner].model_dir`;
production profiles require a configured NER model so person names cannot leak unredacted.
Configure `[ner].model_dir` in the profile's policy file, or remove `production = true`.
```

Without an NER model the pipeline catches only regex-detectable PII (e.g. emails), so arbitrary person names — including names nested in JSON column values — would pass through. Because the Gaze runtime makes NER fail-closed (a model load or backend error aborts redaction rather than passing raw text), requiring a model means a misconfiguration fails closed at startup instead of leaking at query time. See [set-up-production-NER.md](../how-to/set-up-production-NER.md).

## `[session]`

| Field | Type | Default | Description |
|---|---|---|---|
| `scope` | string | `"conversation"` | Gaze session scope. Only `conversation` is supported at v0.1. |
| `ttl_secs` | u64 | (none) | Gaze session TTL in seconds. |

> `[session]` here is the Gaze session config inside the policy file. It is distinct from the profile-layer `snapshot_retention_days` / `auto_purge` fields, which deliberately live at the profile layer (not under `[session]`) to avoid colliding with `ttl_secs`. See [profile-schema.md](./profile-schema.md#snapshot-retention).

## `[policy]`

The redaction policy root.

| Field | Type | Default | Description |
|---|---|---|---|
| `default_action` | string | (none) | Action applied to detected spans that have no more-specific column/class rule. Opt into deny-by-default by setting this. |
| `[policy.database]` | table | (required) | Per-column DB rules (below). |
| `[policy.logs]` | table | (none) | Log strip rules (below). |

### `[policy.database]`

Contains a `columns` array (TOML `[[policy.database.columns]]`). Each entry is a `ColumnRule`:

| Field | Type | Default | Description |
|---|---|---|---|
| `column` | string | (required) | Raw column name the rule applies to. |
| `class` | string | (required) | PII class (see [Classes](#pii-classes)). |
| `action` | string | `"tokenize"` | Redaction action (see [Actions](#actions)). |

### `[policy.logs]`

| Field | Type | Default | Description |
|---|---|---|---|
| `path` | path | (none) | Log path the policy applies to. |
| `strip_patterns` | string[] | `[]` | Regex patterns whose matches are redacted from log lines before return. |
| `action` | string | `"redact"` | Action applied to `strip_patterns` matches. Restricted to `redact` or `tokenize`. |

## Actions

Valid `action` values for column rules:

| Action | Effect |
|---|---|
| `tokenize` | Replace with a session-stable token (e.g. `<EMAIL_001>`). |
| `redact` | Remove the value. |
| `format_preserve` | Replace while preserving the value's format. |
| `generalize` | Replace with a generalized form. |
| `preserve` | Leave the value unchanged. |

Log `strip_patterns` accept only `redact` (default) or `tokenize`.

## PII classes

Valid `class` values:

| Class | |
|---|---|
| `email` | Built-in. |
| `name` | Built-in (NER-backed). |
| `location` | Built-in. |
| `organization` | Built-in. |
| any non-empty string | Treated as a custom PII class. |

## `[connection]`

Optional `[connection.<name>]` tables (`ConnectionConfig`) carry auxiliary connection metadata (`kind`, `ssh_host`, `local_port`, `remote_host`, `remote_port`, `database`, `user`, `password_env`). Profile-layer `[profiles.source]` is the canonical source-connection surface; see [profile-schema.md](./profile-schema.md#source-spec-profilessource).

## Query access vs. schema presentation

Two distinct mechanisms govern columns. They must not be conflated.

### Query access — `ColumnInfo.allowed`

Each column carries a `ColumnInfo` with an `allowed` flag:

```text
ColumnInfo { name, name_token, data_type, nullable, allowed }
```

Canned-query validation rejects any projected or filtered column whose `allowed` is false; only `allowed` columns are compiled into the parameterized SQL. This is the **query-access** control. A column that is displayed (raw, tokenized, or allowlisted) is **not** thereby queryable — access is governed solely by `ColumnInfo.allowed`.

### Schema presentation — `schema_tokenize` / `schema_allowlist`

These are **profile** fields (not policy-file fields), and they affect **presentation only** in `schema`/`list_tables` output:

- `schema_tokenize = true` tokenizes displayed table/column labels.
- `schema_allowlist` keeps selected labels raw in tokenized mode.

Presentation tokenization does not grant or revoke query access. Canned queries always use raw configured table/column names regardless of presentation mode, and are still gated by `ColumnInfo.allowed`. See [profile-schema.md](./profile-schema.md#schema-presentation) and [mcp-tools.md](./mcp-tools.md).

## See also

- [profile-schema.md](./profile-schema.md) — the profile fields `policy`, `production`, `schema_tokenize`, `schema_allowlist`.
- [mcp-tools.md](./mcp-tools.md) — `query`/`schema`/`list_tables` behavior.
- [spec.md](./spec.md) — audit + restore and schema-name leak threat.
- [architecture.md](./architecture.md) — redaction pipeline and dispatch flow.
