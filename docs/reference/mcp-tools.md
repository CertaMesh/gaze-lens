# MCP tools reference

`gaze-lens` exposes exactly five MCP tools over stdio: `query`, `schema`, `list_tables`, `log_tail`, `log_grep`. The set is locked at v1; adding a tool requires a [SPEC](./spec.md) amendment, not an implementation change. Argument-schema growth under the locked tool list is allowed.

The server is started with [`gaze-lens serve`](./cli.md#serve). For client wiring see [wire-up-mcp-clients.md](../how-to/wire-up-mcp-clients.md).

## Common contract

### Required `profile` argument

**Every** tool call must include `profile: string` selecting one configured profile. There is no default at the MCP layer. The value must match the pattern:

```text
^[a-z0-9][a-z0-9_-]{0,63}$
```

An empty or unknown profile is rejected as MCP `InvalidParams`, with the loaded profile list included in the error message.

### Source-class compatibility

Each profile has a source class — `database` or `log`. Tools are class-specific:

| Class | Tools |
|---|---|
| `database` | `query`, `schema`, `list_tables` |
| `log` | `log_tail`, `log_grep` |

Calling a tool with the wrong profile class is rejected with a structured `LensError::ProfileMismatch`, rendered as:

```text
profile `<name>` is a <actual> source; tool `<tool>` requires a <required> profile
```

(`<actual>`/`<required>` are `database` or `log`.) This surfaces to the agent as `InvalidParams`.

### `serve` restrict-list

`gaze-lens serve` exposes all configured profiles by default. `serve --profile A --profile B` exposes only the listed profiles. A legacy single-profile entry (`serve --profile prod`) behaves as a one-element restrict-list. The tool schemas still require the `profile` argument in every mode.

### Dispatch chokepoint (redact → manifest → return)

Every tool result routes through `Session::dispatch_tool` before leaving the process. `dispatch_tool` builds a `gaze_mcp_core::PiiEnvelope` whose sealed `ToolCtx` makes the redact → manifest → return ordering a compile-time invariant rather than a hand-rolled runtime guard. The ordering is:

1. The `profile` field is extracted **raw** for routing, but remains in the args passed downstream.
2. `manifest.begin_call(...)` runs first (fail-closed).
3. The matching `Tool::invoke` runs through the sealed `ToolCtx`; the adapter returns raw values/text into the envelope.
4. `Pipeline::redact` produces tokenized output.
5. `manifest.finish_call(...)` stores **tokenized** args, status, result summary, and the snapshot reference.

Tool arguments (table/column names, where AST, grep patterns) are tokenized through the same redaction path **before** the manifest is written — the manifest never stores raw arguments. If begin/finish fails, no raw output is returned. See [architecture.md](./architecture.md#sessionmanifestrestore) for the full flow and [pseudonymization-and-replay.md](../explanation/pseudonymization-and-replay.md) for the rationale.

### Observability metadata

Existing tool responses may carry an optional `observability` object (ambiguity counts, validator-veto counts, collision/anchor outcomes, locale safety-net dispatch). It is produced only after `Pipeline::redact`, is bounded, and never contains raw PII or reconstructable offsets. It is not a separate tool. See [spec.md](./spec.md) §v0.9 observability amendment.

---

## `query`

Run a canned structured DB query. **No raw SQL.** Class: `database`.

### Arguments (`CannedQuery`)

| Field | Type | Required | Description |
|---|---|---|---|
| `profile` | string | yes | Configured DB profile name (pattern above). |
| `table` | string | yes | Raw configured table name. |
| `columns` | string[] | no | Projected columns; omit for all. |
| `where` | WhereClause[] | no | Filter clauses (see below). |
| `where_combinator` | `and` \| `or` | no | Combinator joining `where` clauses. |
| `order_by` | OrderBy[] | no | Sort terms (see below). |
| `limit` | u32 | no | Row cap. |

**WhereClause** = `{ col: string, op: WhereOp, val?: scalar | scalar[] }`, where `WhereOp` ∈ `eq`, `ne`, `gt`, `gte`, `lt`, `lte`, `in`, `like`, `is_null`, `is_not_null`.

**OrderBy** = `{ col: string, dir: "asc" | "desc" }`.

The shape compiles to safe parameterized SQL with bound parameters. Canned queries use raw configured table/column names and are compiled only against columns whose `ColumnInfo.allowed` is true; presentation tokenization does not grant query access. See [policy-schema.md](./policy-schema.md).

### Example

```json
{"tool": "query", "args": {"profile": "prod", "table": "users", "limit": 5}}
```

---

## `schema`

Describe one raw configured table schema. Class: `database`.

### Arguments (`SchemaArgs`)

| Field | Type | Required | Description |
|---|---|---|---|
| `profile` | string | yes | Configured DB profile name. |
| `table` | string | yes | Raw configured table name to inspect. Requests use raw table names from profile policy even when `schema_tokenize = true` changes presentation output. |

### Behavior

Table and column labels are shown **raw by default**. Set `schema_tokenize = true` on the profile to tokenize presentation output; `schema_allowlist` then keeps selected labels raw (presentation only — not query access). Profile edits require restarting/reloading the MCP server. See [profile-schema.md](./profile-schema.md#schema-presentation).

---

## `list_tables`

List table names. Class: `database`.

### Arguments (`ListTablesArgs`)

| Field | Type | Required | Description |
|---|---|---|---|
| `profile` | string | yes | Configured DB profile name. |

### Behavior

Names are raw by default; `schema_tokenize`/`schema_allowlist` apply the same presentation rules as `schema`. Profile edits require restarting/reloading the MCP server.

---

## `log_tail`

Tail a configured SSH log source. Class: `log`.

### Arguments (`LogTailArgs`)

| Field | Type | Required | Description |
|---|---|---|---|
| `profile` | string | yes | Configured `ssh_log` profile name. |
| `lines` | u32 | no | Number of trailing lines to fetch. |

### Behavior

`gaze-lens` shells out from the laptop using the fixed `ssh -- <host> tail -n <N> -- <quoted_path>` form with validated host arguments (no shell-string interpolation; `-`-prefixed hosts rejected). Output is redacted per Gaze policy before return.

---

## `log_grep`

Search a configured SSH log source. Class: `log`.

### Arguments (`LogGrepArgs`)

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `profile` | string | yes | — | Configured `ssh_log` profile name. |
| `pattern` | string | yes | — | Search expression. In `regex` mode, a Rust regex applied to the log window. In `keyword` mode, split into literal terms and AND-matched. |
| `level` | string | no | (none) | Optional log-level filter. |
| `limit` | u32 | no | (none) | Cap on returned matching lines. |
| `mode` | string | no | `regex` | `regex` or `keyword`. Unknown modes fail closed as invalid args. |
| `refresh` | bool | no | `false` | `true` busts the keyword cache and re-tails the bounded SSH window. |

### `regex` mode (default)

Byte-identical to v0.4. The match predicate runs over the **raw** log text while only displayed lines are redacted. No raw value is ever returned, but the boolean match result is a **one-bit-per-query oracle** over raw data (an agent can confirm presence/absence of a raw substring by crafting a regex). The searched bounded window is fully manifested (audit records data accessed, not only data returned). This residual risk is preserved deliberately for v0.4 compatibility.

### `keyword` mode

The `pattern` is interpreted as whitespace-separated keyword terms. Matching is case-insensitive, ANDs across all terms, returns matching lines in original order, and honors `limit`. The predicate runs over the **same redacted text the agent sees**, so it cannot probe raw values. A token-shaped term matches only the redacted token already present; token searches require the complete `<hash:Name_N>` token minted for the current session (partial fragments such as `Email_1` return 0 hits, by design). Keyword terms are never restored to raw values.

The keyword index is an in-memory derived cache over redacted text only, scoped to the process and bounded by a short TTL. A cache hit reuses the prior fetch's snapshot; `refresh: true` or TTL expiry forces a new bounded-window fetch and snapshot. The keyword-mode manifest and snapshot intentionally cover the full redacted bounded window searched (a superset of the matched response), governed by the existing snapshot retention / `auto_purge` controls.

Prefer `keyword` mode for sensitive or `production`-tier logs. The tool description surfaces this caveat to agents. See [search-logs.md](../how-to/search-logs.md) and [threat-model.md](../explanation/threat-model.md).

---

## See also

- [cli.md](./cli.md) — the `serve` subcommand and its flags.
- [profile-schema.md](./profile-schema.md) — profile fields including `schema_tokenize`/`schema_allowlist`.
- [policy-schema.md](./policy-schema.md) — `ColumnInfo.allowed` query-access governance.
- [architecture.md](./architecture.md) — `Session::dispatch_tool` and the `PiiEnvelope` chokepoint.
- [spec.md](./spec.md) — the locked product spec and MCP server contract.
