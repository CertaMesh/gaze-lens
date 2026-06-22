# CLI reference

`gaze-lens` ships exactly six CLI subcommands: `serve`, `init`, `query`, `replay`, `check`, `demo`. The set is locked at v1; adding a subcommand requires a [SPEC](./spec.md) amendment, not an implementation change.

This page documents every subcommand's synopsis, arguments, flags, and side effects. For task-oriented walkthroughs see the how-to guides linked under each subcommand.

## Invocation

```text
gaze-lens [GLOBAL OPTIONS] <SUBCOMMAND> [SUBCOMMAND OPTIONS]
```

`gaze-lens --version` prints the package version (propagated to every subcommand). `gaze-lens --help` and `gaze-lens <subcommand> --help` print generated usage.

### Global options

These apply to every subcommand and are passed **before** the subcommand name.

| Option | Env var | Default | Description |
|---|---|---|---|
| `--project-config <PATH>` | `GAZE_LENS_PROJECT_CONFIG` | `.gaze-lens.toml` (cwd) | Path to the project profile file. |
| `--user-config <PATH>` | `GAZE_LENS_USER_CONFIG` | `~/.gaze-lens/profiles.toml` | Path to the user profile file. |
| `--log <FILTER>` | — | unset | Sets `RUST_LOG` for this invocation (tracing filter, e.g. `gaze_lens=debug`). |

Profile resolution merges the project and user files; see [profile-schema.md](./profile-schema.md) for field-level precedence.

---

## `serve`

Run the MCP stdio server. This is the primary product surface: an agent connects over stdio and calls the [5 MCP tools](./mcp-tools.md).

### Synopsis

```text
gaze-lens serve [--profile <NAME>]... [--manifest <PATH>] [--snapshot-dir <PATH>] [--print-discovery]
```

### Options

| Option | Env var | Default | Description |
|---|---|---|---|
| `--profile <NAME>` | — | (none = all) | Restrict-list. Repeatable. With no `--profile`, every configured profile is exposed. Listing one or more names exposes only those. |
| `--manifest <PATH>` | `GAZE_LENS_MANIFEST` | `~/.gaze-lens/manifest.sqlite` | SQLite manifest path. |
| `--snapshot-dir <PATH>` | `GAZE_LENS_SNAPSHOT_DIR` | `~/.gaze-lens/snapshots` | Snapshot blob directory. |
| `--print-discovery` | — | off | Print the configured profile discovery inventory as JSON to stdout and exit **without** starting the MCP server. |

### Behavior

- **Eager parse, lazy connect.** At startup, all selected profile TOML, policy files, and Gaze pipelines are parsed and validated. DB pools and SSH log sources are constructed on the first tool call referencing the profile, then cached for the process lifetime. Profile edits require a server restart.
- A loaded-profiles banner is written to stderr on boot.
- Every MCP tool call must carry a `profile` argument naming one of the loaded profiles. Empty or unknown profile is rejected as MCP `InvalidParams`. See [mcp-tools.md](./mcp-tools.md).
- The single server process shares one `gaze::Session` across profiles with a per-profile pipeline registry; cross-profile token correlation is intentional (see [pseudonymization-and-replay.md](../explanation/pseudonymization-and-replay.md)).

### Side effects

- Opens the manifest and (lazily) source connections.
- Runs the snapshot-retention sweep before constructing the session when any loaded profile sets `snapshot_retention_days`; with multiple profiles, the **most-restrictive** retention applies (`min(days)`, `auto_purge` conjunction). See [profile-schema.md](./profile-schema.md#snapshot-retention).
- Writes a manifest row and snapshot per successful tool call.
- Emits process diagnostics to stderr/tracing.

See [wire-up-mcp-clients.md](../how-to/wire-up-mcp-clients.md) for client configuration.

---

## `init`

Guided profile setup. Writes a profile, optional MCP client config, and an optional agent primer. Interactive by default; `--non-interactive` runs scripted.

### Synopsis

```text
gaze-lens init [--profile <NAME>] [--source-kind <KIND>] [--scope <SCOPE>] [connection flags]
               [--client <CLIENT>]... [discovery flags] [write-control flags]
```

### Options

**Profile identity and source**

| Option | Default | Description |
|---|---|---|
| `--profile <NAME>` | (prompted) | Profile name. Required when `--non-interactive`. |
| `--source-kind <KIND>` | (prompted) | One of `mysql`, `postgres`, `sqlite`, `ssh-log`. Required when `--non-interactive`. |
| `--scope <SCOPE>` | (prompted) | Where to write the profile: `project` → `<cwd>/.gaze-lens.toml`; `user` → `~/.gaze-lens/profiles.toml`; `project-auto-purge` → project file with `auto_purge` opt-in. |

**Connection details**

| Option | Applies to | Description |
|---|---|---|
| `--source-host <HOST>` | db, ssh-log | DB or SSH host (required for `ssh-log`). |
| `--source-port <PORT>` | db | DB port. |
| `--source-database <NAME>` | db | Database name. |
| `--source-username <USER>` | db | DB username. |
| `--source-password-env <VAR>` | db | Env var holding the DB password (env backend). |
| `--secret-backend <env\|keyring>` | db | Password backend. Default `env`. |
| `--source-password-keyring-service <NAME>` | db | Keyring service name. |
| `--source-password-keyring-account <NAME>` | db | Keyring account name. |
| `--no-keyring-write` | db | Do not write the keyring entry during init. |
| `--source-ssh-host <HOST>` | mysql, postgres | SSH tunnel jump host. |
| `--source-local-port <PORT>` | mysql, postgres | SSH tunnel local forwarded port. |
| `--source-path <PATH>` | sqlite, ssh-log | SQLite DB path or remote log path. |
| `--source-json-text-columns <A,B,...>` | sqlite | SQLite TEXT-as-JSON column allowlist (comma-separated). |

**MCP client and agent files**

| Option | Default | Description |
|---|---|---|
| `--client <CLIENT>` | (none) | MCP client to configure. Repeatable. One of `codex`, `claude-code`, `cursor`. Empty = none. |
| `--no-mcp-config` | off | Skip writing any MCP client config. Conflicts with `--client`. |
| `--no-agents-md` | off | Skip patching `AGENTS.md`. |
| `--also-claude-md` | off | Also patch `CLAUDE.md` if it exists in cwd. |

**Discovery (setup-time only)**

| Option | Description |
|---|---|
| `--discover-ssh-host <USER@HOST>` | SSH login host used once to read a remote Laravel-style `.env`. Requires `--discover-env-path`. |
| `--discover-env-path <PATH>` | Absolute remote `.env` path to read via SSH. Requires `--discover-ssh-host`. |
| `--accept-prod-rw <HOST>` | Non-interactive consent to store the discovered production credential as-is. Must exactly match `--discover-ssh-host`. |
| `--allow-new-ssh-host` | Opt into trust-on-first-use (`StrictHostKeyChecking=accept-new`) for first-contact SSH host keys. |

**Write control**

| Option | Description |
|---|---|
| `--allow-overwrite` | Allow overwriting an existing profile / MCP entry of the same name. |
| `--non-interactive` | Run without prompts. Missing required input exits 1. |
| `--print-only` | Render preview only; performs no writes; exits 0. Conflicts with `--write-all`. |
| `--write-all` | Skip per-step confirmation prompts but still validate and write. Conflicts with `--print-only`. |
| `--smoke-check` | Run an in-process `check` after the batch write (opt-in). |

### Behavior and side effects

- **Additive writes.** Unrelated `[[profiles]]` entries are preserved verbatim (merged via `toml_edit`, comments/formatting retained). A same-name entry is refused unless `--allow-overwrite`/`--write-all`. Writes are skipped when rendered TOML matches on-disk bytes (`no changes`, exit 0).
- **Atomic writes.** Each destination is staged as `<dest>.tmp.<pid>`, fsynced, renamed, and the parent directory fsynced. Mid-batch failure surfaces as `LensError::BatchPartial { applied, pending, failed, source }`.
- **Never writes a literal password.** Profiles rely on `password_env` or `source.secret`. With `--secret-backend keyring`, init performs a keyring round-trip preflight, refuses to replace a differing entry without `--allow-overwrite`/`--write-all`, verifies via read-back, then writes the profile; an orphaned keyring locator is reported if the keyring write succeeds but the profile commit fails.
- **Scope role split.** `--scope project` refuses transport overrides (`host`/`port`); `--scope user` refuses policy and `auto_purge`; `auto_purge` is gated to `--scope project-auto-purge`.
- **MCP config destinations.** `--client claude-code` writes `.mcp.json`; `--client codex` writes `~/.codex/config.toml`; `--client cursor` writes `.cursor/mcp.json`.

See [configure-profiles.md](../how-to/configure-profiles.md) and [wire-up-mcp-clients.md](../how-to/wire-up-mcp-clients.md).

---

## `query`

Run one canned structured DB query from the CLI through the **same** audit + redaction path as the MCP `query` tool. Intended for human dry-runs.

### Synopsis

```text
gaze-lens query --profile <NAME> --table <TABLE> [--column <COL>]... [--where-json <JSON>]
                [--where-combinator <and|or>] [--order-by-json <JSON>] [--limit <N>]
                [--format <pretty-json|json>] [--manifest <PATH>] [--snapshot-dir <PATH>]
```

### Options

| Option | Env var | Default | Description |
|---|---|---|---|
| `--profile <NAME>` | — | `default` | Profile selecting the DB source. |
| `--table <TABLE>` | — | (required) | Raw configured table name. |
| `--column <COL>` | — | (all) | Projected column. Repeatable; omit for all columns. |
| `--where-json <JSON>` | — | (none) | JSON array of where clauses: `[{"col","op","val"?}]`. Operators: `eq`, `ne`, `gt`, `gte`, `lt`, `lte`, `in`, `like`, `is_null`, `is_not_null`. |
| `--where-combinator <and\|or>` | — | (none) | Combinator joining where clauses. |
| `--order-by-json <JSON>` | — | (none) | JSON array of order terms: `[{"col","dir"}]` where `dir` is `asc` or `desc`. |
| `--limit <N>` | — | (none) | Row cap (`u32`). |
| `--format <pretty-json\|json>` | — | `pretty-json` | `pretty-json` = indented (human); `json` = compact (scripts). |
| `--manifest <PATH>` | `GAZE_LENS_MANIFEST` | `~/.gaze-lens/manifest.sqlite` | SQLite manifest path. |
| `--snapshot-dir <PATH>` | `GAZE_LENS_SNAPSHOT_DIR` | `~/.gaze-lens/snapshots` | Snapshot blob directory. |

The canned query shape is `{table, columns?, where?, order_by?, limit?}`; the CLI maps `--column`/`--where-json`/`--order-by-json` onto it. No raw SQL strings are accepted at v1.

### Side effects

- Tokenizes query args and result rows through `Pipeline::redact`, writes a manifest row and snapshot, and prints the tokenized result. Identical audit semantics to the MCP `query` tool (operator-error retrieval bypass is closed by sharing the path).
- Runs the snapshot-retention sweep before session construction when the profile sets `snapshot_retention_days`.

---

## `replay`

Restore a whole session's tokenized tool arguments and results from the local manifest and snapshot files.

### Synopsis

```text
gaze-lens replay <SESSION_ULID> [--profile <NAME>] [--manifest <PATH>] [--call-id <ID>]
```

### Options

| Option | Env var | Default | Description |
|---|---|---|---|
| `<SESSION_ULID>` | — | (required) | Positional. The session ULID to replay. |
| `--profile <NAME>` | — | `default` | Profile context for replay. |
| `--manifest <PATH>` | `GAZE_LENS_MANIFEST` | `~/.gaze-lens/manifest.sqlite` | SQLite manifest path. |
| `--call-id <ID>` | — | (none) | Per-call selector. **Rejected at v1** with a `not in v1; tracked as v1.x candidate` error; current Gaze replay APIs restore a session snapshot, not one call's token map. |

### Behavior

- Walks successful calls for the session, imports each referenced snapshot via `gaze::Session::import(snapshot)`, and prints restored call-history JSON.
- Uses strict restore policy: known tokens are restored byte-exactly; token-shaped strings absent from the snapshot are left in place and reported with a failed restore decision.
- Each call carries `restore_telemetry`; the session carries `restore_telemetry_summary` (success/partial/failed counts plus unknown-token, manifest-bypass, and fresh-PII counts).
- Replay of a tombstoned row (snapshot purged by `auto_purge`) returns `LensError::SnapshotPurged` citing the policy rather than a silent miss.

`replay` reads only; it writes no manifest rows or snapshots. See [replay-a-session.md](../how-to/replay-a-session.md).

---

## `check`

Validate a profile and its source **before** giving an agent access. Writes no manifest row or snapshot.

### Synopsis

```text
gaze-lens check --profile <NAME> [--explain-risk] [--format <text|json>]
```

### Options

| Option | Default | Description |
|---|---|---|
| `--profile <NAME>` | `default` | Profile to validate. |
| `--explain-risk` | off | Emit the trust report: exposed surfaces, redaction posture, and residual risks. |
| `--format <text\|json>` | `text` | Output format for `--explain-risk`. Requires `--explain-risk`; ignored otherwise. |

### Behavior

Validates profile parsing, policy parsing, Gaze pipeline construction, secret-backend reachability, and the source connection. Database profiles perform a read-only connection and table-list ping. SSH log profiles validate command construction without tailing remote logs.

Secret validation prints a distinct status before the source ping, naming the backend and locator only (never password bytes):

- `secret: ok (...)` — backend resolves.
- `secret: NOT FOUND (...)` — referenced env var or keyring entry absent.
- `secret: ACCESS DENIED (...)` — keyring rejected access.
- `secret: BACKEND UNAVAILABLE (...)` — platform keyring backend unreachable.

### Trust report (`--explain-risk`)

A local-only report describing what the profile exposes and the residual risks. It does **not** connect to the source, read keyring/env password values, invoke SSH, or write the manifest. It validates profile, policy, and Gaze pipeline, then reports the input, process, output, at-rest, and operator-handoff surfaces.

- **Text mode** writes status lines and the report to stdout.
- **JSON mode** (`--format json`) writes status lines to **stderr** and emits exactly one JSON object on stdout. The object includes `report_version: 1`; the v1 field set is closed — any field addition, removal, or rename requires `report_version: 2` with a deprecation note.

`check` has no manifest or snapshot side effects.

---

## `demo`

Self-contained PII-redaction demonstration. Tokenizes a small canned in-memory dataset and inline-restores it in the same process.

### Synopsis

```text
gaze-lens demo
```

### Behavior and side effects

- Takes no arguments.
- Builds a tempdir manifest and snapshot directory, dispatches a canned in-memory query through the same `Session::dispatch_tool` entry as `query`/`serve`, then calls `gaze::Session::import` against the just-written snapshot to restore the tokenized result.
- Prints the tokenized section and the restored section side by side.
- Writes **nothing** to `~/.gaze-lens/`; the tempdir is wiped on exit. No follow-up `replay` is required.

`demo` is a CLI-only inline-replay helper; it does not extend the MCP tool list and does not introduce a new data source. See [getting-started.md](../tutorials/getting-started.md).

---

## See also

- [mcp-tools.md](./mcp-tools.md) — the 5 MCP tools and their argument schemas.
- [profile-schema.md](./profile-schema.md) — profile TOML fields, defaults, and merge rules.
- [policy-schema.md](./policy-schema.md) — `gaze-policy.toml` fields.
- [architecture.md](./architecture.md) — implementer spine and dispatch flow.
- [spec.md](./spec.md) — the locked product spec.
