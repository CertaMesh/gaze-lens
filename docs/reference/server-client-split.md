# Server/client split contract and implementation plan

## Status and precedence

**Approved next contract, not implemented runtime.** This normative amendment to
[spec.md](./spec.md) authorizes a two-binary product with local Gaze and bounded
server inspection. It changes no runtime, dependency pin, test, or release in this
PR. Current main at `89423b75ec40d7af5d4e385d553829adb0b6596e` still has five
agent tools, direct sources and a separate log-tail-only service mode.

For the next split implementation, this document supersedes the older spec's
laptop-owned DB/SSH execution, no-server-install rule, five-tool ceiling,
unrestricted cross-profile session sharing, request-redaction/recoverable-argument
contract, and log-tail-only private protocol. Other safety requirements remain.
The current reference pages describe today's runtime until explicitly migrated.

> PII safety wins. Always. We sacrifice ergonomics for safety; we never sacrifice safety for ergonomics.

This priority is not a zero-leakage guarantee. Detection has limited coverage.
Raw responses reach trusted client memory; snapshots contain raw mappings on the
operator's disk. TLS authenticates transport, not the truth or privacy of content.

## 1. Product and dependency ownership

Ship three crates and exactly two product binaries:

- `gaze-lens-protocol`: closed wire types, versions, fixed errors and pure
  shape/bounds checks. No Gaze, SQLx, rusqlite, RMCP, collectors, source credentials
  or SQL compiler. Serialization libraries are sufficient.
- `gaze-lens-server`: binary and server library; configured DB credentials,
  SQLx MySQL/Postgres/SQLite source drivers, structured-query compilation,
  source access policy, logs and trusted inspection collectors. No Gaze/model,
  token namespace, snapshots, client manifest or replay.
- `gaze-lens`: client binary and library; agent stdio MCP, six human CLI commands,
  authenticated TLS, remote validation, local Gaze/policies/models, sessions,
  request restoration, audit, snapshots and offline replay. Local manifest
  `rusqlite` is allowed. No SQLx, source drivers/credentials, direct local DB/log
  reads, SSH, collector processes, SQL compilation or direct-source fallback,
  including behind optional features.

The server may be on the production host or a trusted adjacent host with access
to configured sources. Installation is now required. The agent never contacts
the raw server directly; its credentials belong to the trusted client process.
A server credential is access to raw data and must not be placed in agent args.
No Rama, server-side Gaze option, generic plugin framework, monitoring,
remediation, writes, arbitrary shell/exec or raw SQL in this release.

```text
agent / human query
  -> local client: validate + trust-domain check + existing-token restore
  -> authenticated TLS
  -> server: independent grants + bounded DB/log/inspection execution
  -> authenticated TLS, raw closed result
  -> client: validate -> response codec -> Gaze envelope
  -> durable snapshot + manifest -> decode protected result -> agent
operator replay -> local manifest + snapshots only
```

## 2. Public surface and CLI

The approved agent surface is exactly six tools: `query`, `schema`, `list_tables`,
`log_tail`, `log_grep`, `inspect`. Each requires the existing configured `profile`
name; profile/resource class mismatches fail with fixed errors. Existing query
and log argument grammar remains except for documented remote bounds below.
A profile resolves an operator-configured resource, never caller-selected DSNs,
hosts, paths, executables or credentials. No replay/export tool exists.

Retain exactly six client commands:

- `serve`: local agent stdio gateway. `--profile` remains a restrict-list. One
  process may expose several domains, but uses separate local sessions/maps for
  distinct domains. No server-service mode remains in this binary.
- `init`: configure client server identity, resource aliases, client credential
  references and local privacy/model policy. Preserve explicit model provisioning;
  never discover production `.env` or DB credentials through local/SSH probes.
- `query`: remote query through the same local envelope and audit as MCP.
- `check`: local profile/policy/model/storage checks plus authenticated server
  readiness. Remove current direct DB connect/list-tables and SSH tail probes.
  Optional source checks use ordinary authorized private operations and the local
  response boundary; readiness alone proves neither DB access nor model coverage.
- `replay`: offline operator restoration of stored output, never request execution.
- `demo`: canned in-memory, offline, ephemeral inline replay. Its old `Database`
  classification does not justify retaining a real database dependency.

Minimal separate server CLI, no agent-facing discovery or human query command:

```text
gaze-lens-server serve --config <PATH>
gaze-lens-server check --config <PATH>
```

`serve` loads server configuration, binds TLS and runs the private protocol.
`check` validates config, TLS material, grants, configured collector availability
and local permissions without querying a DB, tailing a log, running a PHP binary
or contacting production. Both use fixed diagnostics without secrets or source
payloads. `--help` and `--version` are flags, not extra commands. Resource/grant
creation uses operator-managed files; no extra init/admin command is approved.

## 3. Configuration, authority and session domains

Client profiles retain local policy/model, production tier, schema presentation,
retention and credential-reference settings. Their source becomes a closed
`remote` configuration: server identity, TLS endpoint/server name/trust roots,
credential reference, configured resource ID and resource class
(`database`, `log`, `inspection`). Secret values are env/keyring only. Connection
configuration is private operator input, not part of public tool schemas.

Server configuration owns resource definitions, DB read-only roles and secret
references, table/column access allowlists, SQLite `json_text_columns`, fixed
log paths/SSH targets, inspection collectors, budgets and grant file location.
Client policy can restrict further but cannot grant source access. Retain the
current remote privacy startup requirement: explicitly tokenize or redact detected
spans, including every class override derived from column policy; reject preserving
overrides and implicit preserve defaults. Production NER/model validation remains
mandatory locally; no runtime model fetch or fallback to weaker policy. In particular,
`schema_allowlist` is only a presentation exception when `schema_tokenize=true`;
it neither grants query columns nor enables tokenized mode. Raw schema-name
presentation remains the explicit default exception.

Bind every local session/mapping namespace to an immutable trust domain containing
exact authenticated server identities, resource IDs and principal bindings.
Default: **one server/resource destination per session**. Multi-destination domains
require explicit operator permission that restored original values may be disclosed
among every listed destination. Permission to query a destination's own DB does
not authorize disclosure of values learned elsewhere. No live domain widening.

Resolve/check membership before restoration and network send, then verify the
resolved destination is unchanged before sending. A profile alias, credential
principal, resource binding or TLS-valid replacement server must not enlarge or
rebind a live session. Changed membership/identity requires a new session; a
reconnect to the same authenticated binding may retain the existing namespace.
Rotation must preserve an explicitly trusted binding or require a new session.
No cross-domain map import or automatic token sharing. Process-wide retention
continues to respect the existing least-destructive rules for shared storage.

Grants are an explicit set of `(principal, operation, resource)` tuples, with
`inspect` additionally bound to `(view, collector)`. No wildcard operations or
implicit permission inheritance. Exact operations are defined in section 6;
`schema` or `list_tables` permission never implies `query`. Each grant has an
enabled flag, expiry, and operator ceiling for budgets. Credential digests and
principal IDs are server-owned; unrecognized/duplicate/ambiguous grants deny.

The server reopens/revalidates the current grant set before execution and before
remote release, including table/column and inspection scope. The final check is
the revocation linearization point; it cannot recall bytes already released.
Malformed/unavailable grant state denies, including after a successful collection.
Opaque call IDs correlate server access metadata with client audit. Neither log
stores request values, SQL binds, raw source text, tokens or driver error text.

## 4. Request preparation and audit omission

For executable data fields: validate the public shape and bounds; resolve the
frozen destination; restore only existing local session tokens in one frozen
read-only transaction; validate expanded shape, destination and cumulative escaped
JSON byte budget; send; independently authorize and validate on the server.
No request PII detection, redaction or new mapping creation. Known raw literals
stay unchanged. Restoration is one pass, not recursive expansion of restored text.
Structural fields such as operation, profile, mode and enum tags are not restored.
Keyword terms are the local-only exception in section 7 and are never restored.

Use the signed upstream bounded helper contract
`SessionTransaction::restore_strict_text_bounded`, backed by
`strict_restore_tokens`. Preserve `UnknownToken`, `CapExceeded`, `MalformedToken`;
allocation failure maps to `CapExceeded`. Unknown future failures also deny without
formatting their payloads. A post-allocation size test alone is insufficient:
count expansion and JSON escaping cumulatively before allocation/reservation.
Foreign/unknown/malformed tokens and amplification fail with fixed client errors
and no network send. The transaction creates no request-derived mappings.

New argument audit records are versioned metadata-only omission, never raw args,
restored args, a hash of secret args, or a misleading empty reconstructed request.
Persist fixed omission reason `metadata_only`, call/session IDs, locally known
operation and bounded code-owned status/counts. No request-value echo in response
headers, errors or telemetry; clamp framework logging that dumps requests.
Response-only invocation data must never inhabit an accessor documented safe to
re-emit. Preserve the sealed upstream envelope and its default protected mode for
other adopters; do not fork a bypass around it.

Legacy rows with captured arguments retain their legacy restore behavior. New rows
show **arguments unavailable (metadata-only audit)**. Mixed histories decode each
row by its recorded representation; never fabricate missing arguments or execute
them. Output replay stays available subject to existing missing/purged snapshot
errors. Offline replay may read legacy retention settings without constructing a
source or requiring a server/model/network connection.

## 5. Returned data and the local release boundary

The wire is not a clean-output type. Explicit server conversion creates closed
wire DTOs; explicit client conversion admits untrusted data to the local envelope.
Do not serialize internal `TableSchema`/`ColumnInfo` as a wire contract:
`TableSchema.table` and `ColumnInfo.name` have `serde(skip)`, whereas
`ColumnInfo.allowed` has `serde(default)`, **not** skip. Wire schema names must be
explicit. No received or client-supplied `allowed` flag is authorization.

Preserve the pending `c5933af9fc6b010f80ed67a08c0c339da9a1aa32`
`src/session/response_codec.rs` behavior: dynamic keys and source numbers enter
protected text carriers, then decode only after the envelope. The old
`LensValue::redact_with` helper is not this contract. Exact unmodified values can
retain their type; protected numeric values may become tokens. Key collisions,
unknown variants, bad row decoding and invalid codec output fail, never overwrite
keys, silently coerce types or substitute empty strings.

Apply this boundary to DB values, nested JSON keys/numbers, schema presentation,
log content/metadata, host facts, package versions, collector provenance and every
other accepted source-derived field. Only locally generated structural tags,
validated counts and fixed statuses can use the documented carrier exception.
Names/versions are source text, even when they resemble trusted inventory data.
No upstream MCP text blocks, prompts/instructions, annotations, notifications,
stderr or free-form error details are forwarded. Deny undeclared fields rather
than dropping potentially meaningful data silently. Fixed remote error codes map
to fixed local errors, with no payload excerpt.

Gaze success plus durable snapshot/manifest completion precedes release. Gaze,
write/fsync, manifest begin/finish or codec failure releases no result content.
Never stream partial raw results or debug-format raw DTOs. An audit row proves
protection/persistence, not agent delivery. Interrupted later snapshot replacement
must preserve already committed replay. Maintain the remote privacy process's
non-queuing admission and native watchdog protection against cancellation/panic;
timeout must not leave an unsupervised raw response task running.

Snapshots remain client-only `0600` files in `0700` directories, with operator
FileVault/LUKS disk encryption required and existing retention/purge semantics.
Local keyword-window snapshots cover the whole accessed window, not only matches.
Server operational logs are metadata-only; raw results are not persisted there.

## 6. Private protocol v2

Use explicit private version **`gaze-lens-source/2`**, independent of the public
MCP protocol version. Replace the legacy private `2025-11-25` log-tail-only
handshake deliberately. Verified TLS is mandatory, with server-name/trust-root
validation and per-principal 256-bit credentials in protected transport metadata (exactly 64 ASCII hex characters,
from an operator env/keyring reference).
No plaintext, insecure verification, redirect, capability guessing, downgrade or
silent legacy fallback. Keep remote content outside the RMCP runtime.

Freeze these closed semantic DTOs in the protocol crate before transport work.
All objects reject unknown/duplicate keys at every depth. `?` below means optional;
all other fields are required. Enum strings are exact and unknown values fail.
Use bounded newline-delimited JSON over TLS, one operation per connection:

```text
Hello   {version: "gaze-lens-source/2", privacy: "client_gaze"}
Ready   {version: "gaze-lens-source/2", privacy: "client_gaze"}
Call    {version, id, credential, resource, operation, args}
Success {version, id, operation, result}
Failure {version, id, code}
```

`version` always equals the exact version above; `id` is a client-generated opaque
ULID echoed exactly. Credentials are never accepted inside `args`. Sequence is
Hello, Ready, one Call, one Success/Failure, then close. Authentication/parse errors
before a valid call close the connection with a fixed local failure; no guessed
ID. No unsolicited frames, batches, streaming, extra negotiation, notification
messages or generic tool enumeration. Reject mismatched ID/operation/result.

Exact operation/args/result pairs and independently required grants:

- `query`: `{table, columns?, where?, where_combinator?, order_by?, limit?}` →
  `query_rows`. Keep existing scalar/list predicates: clauses `{col,op,val?}`, ops
  `eq/ne/gt/gte/lt/lte/in/like/is_null/is_not_null`, combinator `and/or`, ordering
  `{col,dir:asc|desc}`. No nested object predicate values or new SQL grammar.
  Normalize legacy public `value` alias to `val` locally. Server alone compiles
  bound SQL and expands omitted columns from its allowlist. Check selected,
  WHERE and ORDER columns, table and read-only role, including after restoration.
- `schema`: `{table}` → `table_schema`; `list_tables`: `{}` → `table_list`.
  Both are filtered by server presentation/access scope. Metadata cannot widen
  subsequent query authorization.
- `log_tail`: `{lines}` → `log_window`.
- `log_window`: `{lines}` → `log_window`, specifically for local keyword search.
  No pattern, terms or level filter is sent. A tail grant does not imply this grant.
- `log_regex`: `{pattern,level?,limit}` → `log_matches`. Preserve raw-regex
  semantics, production warnings and any stricter operator restriction. No keyword
  substitution or external caller-chosen executable. Regex is an explicit grant.
- `inspect`: `{view,collector,limit}` → `inspection`; closed views in section 8.
  Public callers choose only a view and limit; the profile selects collector ID.
- `readiness`: `{}` → `readiness`. Reports authorized resource availability in
  server configuration only; no source access or probe. This is a private
  operation for client `check`, not a seventh public tool.

Closed result variants (`kind` discriminator, no extra fields):

```text
query_rows   {kind, columns: [text], rows: [[Value]], truncated: [Reason]}
table_schema {kind, table: text, columns: [{name: text, data_type: text, nullable: bool}], truncated: [Reason]}
table_list   {kind, tables: [text], truncated: [Reason]}
log_window   {kind, lines: [text], truncated: [Reason]}
log_matches  {kind, lines: [text], matched_lines: uint, truncated: [Reason]}
inspection   {kind, view, collector: text, status, evidence, records, truncated: [Reason]}
readiness    {kind, status: "configured"}
```

Rows must have exactly the declared column count; duplicate column/table labels
and truncation reasons reject. `matched_lines` counts only the bounded scanned
window, not the whole source, and is bounded by scanned lines. `Reason` is exactly
`bytes`, `rows`, `line_bytes`, `scan_bytes` or `records`; operation-specific reasons
only, with deterministic source-order prefixes/suffixes documented in fixtures.
Bounds reached between complete records can return explicit truncation; malformed
or oversized individual values/records fail without clipped values. A log window
contains complete lines only; incomplete boundary lines are omitted with a reason.

`Value` uses a `kind` tag and exactly the associated fields: `null` (none), `bool`
(`value`), `i64`/`u64` (`value`: canonical decimal string in exact range), `f64`
(`value`: finite round-trippable decimal string), `decimal` (`value`: decimal
string, `precision`: 1..255, `scale`: 0..precision), `string` (`value`), `bytes`
(`base64`, `len`: exact decoded byte count), `datetime` (`value`: validated ISO-8601
text preserving offset/precision), `uuid` (`value`: canonical UUID text), `json`
(`value`: bounded JSON). Decimal digits must fit declared precision/scale; JSON
numbers retain their exact parsed representation without an f64 detour. Reject
NaN/infinity, invalid encoding/type and length mismatches. Unsupported native DB
types reject the row. Server SQLite JSON-in-TEXT remains allowlist-only.

`Failure.code` is exactly `invalid_request`, `unauthorized`, `unsupported_version`,
`unsupported_operation`, `unavailable`, `cap_exceeded`, `timeout` or
`internal_failure`. No message/details field. Local request-restoration failures
are client-only, not upstream payloads. Never expose driver or collector errors.

## 7. Budgets and logs

These conservative v2 hard ceilings may be lowered by operator grants/local
policy; exceeding one fails or explicitly truncates only where specified above.
Increasing hard ceilings requires protocol review and boundary tests.

- Frames: 1 MiB including newline; executable request: 64 KiB including escaped
  restored JSON. Result body: 128 KiB. Depth: 32; logical nodes: 16,384; scalar
  source text/decoded bytes: 8 KiB; identifiers: 256 UTF-8 bytes; configured IDs:
  1..64 ASCII letters/digits/underscore/hyphen, public profile grammar unchanged.
- Queries: default 100/max 1,000 rows; max 128 columns, 64 predicates, 16 order
  fields, 1,024 total binds including IN lists. Schema/table lists: 1,000 entries.
  No pagination/cursor is added. No unbounded materialization before these checks.
- Logs: default 100/max 1,000 returned lines, 8 KiB per line, 1 MiB scan bytes,
  16,384 scanned lines. Regex expression max 4 KiB plus bounded compiled-regex
  memory (1 MiB). No unlimited source scan to satisfy a result limit.
- Inspection: default 100/max 500 package records; host one record; PHP at most
  32 configured runtimes/services. Collector input/output and combined stdout +
  stderr acquisition max 1 MiB; result caps still apply. Inventory scan max
  100,000 records. Do not buffer an entire host inventory to return a prefix.
- TLS/connect and frame read/write: each 10 seconds; DB/collector work: 5 seconds;
  one remote call total: 30 seconds. Local response admission/protection/durability:
  60 seconds under the watchdog. One active call per local session, server default
  4/max 16 active calls, max 2 per principal, no waiting queue or automatic retry.
  Bound subprocess count, terminate and reap process groups on timeout/cancel,
  cancel DB work and discard unhealthy pooled connections. No background orphan.
- Account allocations before reserve/read/parse/restore and while streaming bounded
  source acquisition. Cap client codec expansion at 8 MiB and final protected
  output at 1 MiB in addition to node/depth bounds. Do not trust source-provided
  counts to allocate. Measure transient copies and enforce admission together.

Keyword `log_grep` fetches the raw bounded window using `log_window`, protects and
durably audits it locally, then indexes/filters using the original keyword/token
spelling and optional local level filter. Preserve case-insensitive AND matching,
complete issued-token matching, structured key/value terms, source ordering, limit
and truncation. Raw keyword literals cannot become a server-side presence oracle.
Metadata headers are not records; preserve the pending c5933af9 classifier fix
that does not require omitted `pattern`/`level` fields. New structured windows must
not recreate the old header-as-record bug or echo request values in headers.

Initial remote path has **no cross-call keyword cache**. Build/discard an index
per call; `refresh` remains accepted and every call fetches afresh. A future cache
requires session/principal/server/resource/policy identity and current server grant
revalidation on every hit before local audit/release; TTL alone is insufficient.
Regex remains a raw-source presence oracle with regex default preserved. Production
profiles retain the current warning recommending keyword, production NER mandate
and any explicit denial configured by the operator. Do not claim regex is safe
merely because returned lines are protected.

## 8. Closed Linux inspection views

Public `inspect` args are exactly `{profile, view, limit?}`; `view` is `host`,
`packages` or `php`. Profile configuration selects the trusted collector target;
no caller path, binary, shell, unit name or arbitrary filter. Default/max limits
are section 7. For `host`, require limit 1 if supplied; client defaults it to 1.
For `php`, default/max limit is 32, with records ordered by configured runtime ID.

Initial collection supports **Linux with dpkg** for package inventory/PHP package
evidence. No RPM, macOS or Windows collector is silently substituted. Non-Linux inspection
returns `unsupported` with no records; Linux host facts do not require dpkg. A
Linux system without dpkg reports packages `unsupported` and PHP package evidence
`unsupported`, independently of configured CLI/FPM evidence. Unavailable configured binary,
permission failure or unprovable evidence produces the explicit state below;
never successful empty inventory inferred from an execution failure.

Every inspection envelope has `status`: `ok`, `unavailable`, `unsupported` or
`unknown`, and `evidence`: `os_release`, `dpkg_database`, `configured_php` or
`none`. Envelope `collector` is the configured ID. Non-ok envelopes have empty
records; partial inventories use `ok` plus explicit truncation. Nested PHP
observations can independently be unknown/unavailable. No free-form diagnostics.
Exact records, with all fields required and no additions:

- `host`, evidence `os_release`, one record:
  `{os_id: text, version_id: text|null, architecture: text}`. Fixed OS release
  fields and architecture only. No hostname, usernames, addresses, machine IDs,
  environment, process list, uptime or generic monitoring measurements.
- `packages`, evidence `dpkg_database`, records:
  `{name: text, version: text, architecture: text}`. Installed dpkg entries only,
  ordered by `(name, architecture)` under a bounded collector strategy. No package
  description, maintainer/contact data, file lists, config, install scripts or
  install/update/removal action. A capped scan must be flagged, not called complete.
- `php`, evidence `configured_php`, records:
  `{runtime_id: text, installed_package: PackageObservation,
  cli: CliObservation, fpm: FpmObservation}`.
  `PackageObservation` is `{status: present|absent|unavailable|unsupported,
  name: text|null, version: text|null}` for a configured exact dpkg package.
  `CliObservation` is `{status: observed|unavailable|unknown, version: text|null}`.
  `FpmObservation` is `{status: observed|unavailable|unknown,
  version: text|null, active: bool|null, app_binding: proven|unknown}`.
  Only `present`/`observed` observations carry non-null version evidence, which is
  required in those states; other versions are null. Package name is non-null only
  when present. Non-observed FPM has `active: null` and `app_binding: unknown`.
  An observed FPM process may have `app_binding: unknown`.
  `app_binding: proven` requires current evidence connecting the configured app
  target to that running FPM instance and version; configuration alone is not proof.

A PHP package being installed does not establish the configured CLI version, and
neither establishes the active application/FPM runtime. Collect these independently.
If the initial collector cannot establish active app binding, return `unknown`;
do not infer it or add production probes to make the field look complete.

Collectors are compiled, fixed implementations selected from a closed registry.
Server operators configure trusted fixed paths/targets; validate ownership,
permissions, symlinks and executable identity before use. Use fixed argv, minimal
sanitized environment and bounded output/time, never a shell command string.
Prefer bounded metadata reads and package queries that do not execute package
scripts. CLI version collection must avoid loading app scripts, php.ini or dynamic
extensions (for example, a trusted configured PHP binary with fixed `-n -v`, parsed
into version only). Never call `phpinfo`, dump env/config, execute app routes,
accept caller-selected code or expose raw collector output. Supporting a new
collector/view/platform requires explicit contract and fixture review.

## 9. Breaking migration and release gate

Recommend **gaze-lens 0.6.0** for this intentional breaking migration from current
0.5.5; if 0.6 is already allocated at implementation time, use the next pre-1.0
minor, never disguise it as a patch. This is a recommendation, not a tag/version
edit or release authorization. Wire versioning is separate from package SemVer.

Reject direct `mysql`, `postgres`, `sqlite`, `ssh_log`, `local_log` client profiles
and the old `remote_mcp_log` configuration before opening a source, with fixed
migration guidance: **“Profile requires migration: configure gaze-lens-server
resources and grants, then replace the client source with a v2 remote profile.”**
Remove client `serve --remote-service-config`; direct operators to the separate
server binary. No automatic secret transfer, legacy protocol retries, or fallback.
Offline `replay` and canned `demo` remain usable without migrated remote profiles.

Migration order: provision server resources/read-only credentials/TLS/grants;
configure client identities and explicit domains; move source allowlists to the
server while retaining stricter client privacy policy; validate both configs;
run synthetic remote proof; then switch agent entries to the new client. Leave
old binary/config available only for deliberate operator rollback, never automatic
runtime fallback. Back up manifests/snapshots privately before migrations; use
versioned readable rows and preserve legacy replay. Rollback must not run an old
writer against a newer manifest it cannot read. No data migration against production
is authorized by this docs PR.

Matching **published** Gaze family APIs remain an implementation integration/release
prerequisite. Signed upstream `dca134215c5a687a49252f9da055cc51ccb31771` is a
behavioral reference, not proof that the API is available on crates.io. Preserve
the response-only invocation, bounded strict restore and metadata-only omission
contract during upstream reland; reconcile any published API differences before
Lens runtime integration. Scoped local patches may aid approved development but
are not registry-build/release proof. Do not copy pending Lens implementation
wholesale into this documentation amendment.

## 10. Ordered implementation plan

Paths below are proposed ownership destinations; current paths identify the work
to move. Each phase is a separately reviewable forward PR. No phase may claim the
whole product works before the end-to-end gates pass. A phase owner owns its
immediate callers and the listed proof, not an unrestricted refactor.

### Phase 0: contract approval (this PR)

Owner: spec/plan writer, followed by independent plan reviewers. Files:
`docs/reference/server-client-split.md`, normative `spec.md` link and scoped
current/next notices in AGENTS, CONTRIBUTING and reference/remote guide pages.
Dependencies: accepted local-Gaze synthesis and current main. Proof: document
links/consistency, `git diff --check`, unchanged runtime/tests/hooks, normal
repository hooks. Acceptance A-J below is future work, not evidence from this PR.

### Phase 1: protocol types and dependency partitions

Owner: protocol/extraction implementer. Depends on phase 0. Create
`crates/gaze-lens-protocol/src/{lib,wire,value,bounds}.rs`; establish server/client
crate manifests/workspace packaging. Split `src/source/db/query.rs` into pure DTO
and server compiler; split `src/value.rs` wire conversion from Gaze methods.
Do not reuse skipped internal schema serializers. Define all operation/result
pairings, strict framing, budget accounting and error enums from sections 6-8.
Proof: protocol fixtures, duplicate/unknown-field and typed-value tests, independent
crate builds and normal/supported-feature dependency-tree assertions (A, C, H).
No transitional client dependency feature may become a release configuration.

### Phase 2: standalone authenticated execution server

Owner: server implementer. Depends on phase 1. Move
`src/source/db/{mysql,postgres,sqlite,query,schema,runtime}.rs`,
`src/source/log/{local_log,ssh_log}.rs`, `src/source/ssh_tunnel.rs` and execution
parts of `src/source/remote/{service,mod}.rs` to
`crates/gaze-lens-server/src/{source,auth,config,service,main}.rs`.
Replace log-tail-only grants with exact operation/resource/column grants and
pre/post-read revocation checks. Preserve SSH argument validation/quoting, reject
`-`-prefixed hosts, and never move compiler/source execution into protocol helpers.
Add separate server serve/check CLI and readiness without source probes.
Proof: synthetic DB/log fixtures, grant/column/read-only denial, revocation,
cancellation, acquisition budgets and server no-Gaze dependency proof (A-D, H).

### Phase 3: preserve local request, response and replay boundary

Owner: client privacy-boundary implementer. Depends on phase 1 and matching
published upstream API; uses pending c5933af9 as read-only behavioral reference.
Move/adapt `src/session/{mod,request,response_codec,boundary,restore,manifest}.rs`
(the request/codec/boundary modules are pending, not on the base),
`src/manifest/gaze_mcp_adapter.rs`, `src/policy.rs` and remote watchdog into
`crates/gaze-lens/src/{session,manifest,policy,transport}`.
Implement immutable session/domain registry, frozen restore, metadata-only audit,
closed upstream admission, numeric/key carriers and durable release. Preserve
legacy/new/mixed replay before removing old request-capture assumptions.
Proof: no request detection/maps, bounded restore, domain/alias rejection,
all release fault injections, logging canaries and offline replay (C, E, G, J).

### Phase 4: route every client entry point remotely

Owner: client integration implementer. Depends on phases 2-3. Adapt
`src/frontend/mcp.rs`, `src/mcp/tools/*`, `src/cli/{serve,query,check,replay,demo}.rs`,
`src/cli/init/*`, `src/profile.rs`, `src/source/remote/{mod,wire,watchdog}.rs`
into client crate modules. Keep all six client commands; remove direct source
construction, SSH init discovery and check probes. Add v2 remote profiles,
immutable profile-to-domain routing and fixed direct-profile migration errors.
Keep keyword matching/index local, disable cross-call cache, preserve the header
fix and regex caveat. Public inspect discovery waits for phase 5 support.
Proof: real client→TLS→server→local-envelope fixture calls; server-off query cannot
succeed; client no-driver/source dependency checks; local keyword semantics,
production restrictions, offline demo/replay and obsolete flag/profile denial
(A, B, D-F, H, J).

### Phase 5: bounded Linux/dpkg inspection

Owner: collector implementer. Depends on phases 2-4. Add
`crates/gaze-lens-server/src/source/inspect/{mod,host,packages,php}.rs` and
`crates/gaze-lens/src/mcp/tools/inspect.rs`; register only the closed collectors
and six-tool public schema. Fixtures must distinguish installed PHP, configured
CLI, FPM and unproven application binding. No generic exec helper exposed through
protocol or public args. Proof: malformed/caller path/executable denial before
execution, unsupported/unavailable states, hostile names/versions through Gaze,
process cleanup, inventory bounds and exact discovery (B, G-I, J).

### Phase 6: migration, packaging and independent release review

Owner: integration/release coordinator with separate adversarial reviewers.
Depends on all earlier phases and published upstream API. Update actual
`Cargo.toml`/workspace include lists, lockfile, distribution config/workflows,
README, examples, setup skill and all current CLI/profile/MCP/architecture guides
only once behavior exists. Package both binaries independently; include this
contract in package docs. Linux server/collector targets need explicit CI proof;
do not inherit the old client-only target claim. Run acceptance A-J on synthetic
fixtures, affected tests and repository mandatory gates, including feature suites
where adapters changed. Record checks, platforms and residual limits.

No production probe, merge, tag, release or deployment follows automatically.
Release owner must establish exact artifact/target/proof/rollback and authority.
Forward correction versions preserve immutable releases; deliberate binary/config
rollback must respect manifest compatibility. Outstanding A-J failures block split
release, rather than becoming undocumented follow-ups.

## 11. Acceptance matrix (required future checks)

**A. Ownership.** Build/package each binary independently. Inspect normal and every
supported-feature dependency tree: client no SQLx/source drivers/credentials/SSH,
server no Gaze, protocol neither. Trace all six CLI commands and library entry
points; manifest SQLite is the sole client DB exception.

**B. End-to-end.** Synthetic query/schema/tables, log window/regex and each supported
inspect view traverse client→TLS→server→source→local envelope→durable audit→agent.
Stopping the server makes a new query impossible; no direct fallback or hidden probe.

**C. Typed data.** i64/u64 extremes, decimals, finite/nonfinite float, bytes mismatch,
UUID/date, nested JSON dynamic keys and numeric canaries, unknown variants,
invalid rows, key collisions and SQLite JSON allowlist. Exact valid representations
survive or fail explicitly; protected source numbers may become tokens.

**D. Query authority/schema.** Omitted-column expansion, denied table/SELECT/WHERE/
ORDER columns, forged allowed metadata and schema raw/tokenized/allowlist modes.
Client hints and schema presentation never widen server authorization.

**E. Request trust.** Literal unchanged; owned token restored once; unknown/foreign/
malformed token and repeated huge mappings fail fixed at N/N+1 bounds; zero request
detector calls/new maps. Cross-domain destination and alias/principal rebinding
send zero restored bytes. Explicit multi-host fixture permits only intended disclosure.

**F. Keywords.** Original complete issued-token term matches protected local window;
raw literal gains no remote keyword oracle. No terms/level in window request;
headers excluded; ordering/limits/truncation preserved. Revoke/policy/resource
change cannot release stale-authorized cache content; initial path has no cache.

**G. Release failures.** Gaze, snapshot write/fsync, manifest begin/finish and codec
faults release no content. Driver error, stderr, malformed content, request echo
and tracing canaries are absent from agent errors/audit/telemetry. Prior committed
replay survives later interruption; watchdog cancellation/panic remains fail-closed.

**H. Protocol/authority.** Wrong identity/cert, expired/revoked/other-operation grant,
mid-read revoke, duplicate fields, unknown version/ID/result, oversize/deep JSON,
slow reads/writes, amplification, concurrency and cancellation fail bounded.
No downgrade or hidden source execution; final grant check is exercised.

**I. Inspection truth.** Installed PHP differs from configured CLI and FPM;
application binding can remain unknown. Unavailable is not inferred success.
Caller path/executable/unknown view rejects before collector execution; platform,
collector identity, stdout/stderr limits and process cleanup are fixture-tested.

**J. Migration/replay.** Old direct and legacy remote profiles reject actionably;
legacy/new/mixed snapshots replay offline with zero server/DB calls. New args show
omission, old captured args restore, purged output errors stay explicit. Demo is
offline/ephemeral; discovery exactly matches six agent tools, six client commands
and two server commands. Public docs describe only implemented behavior.
