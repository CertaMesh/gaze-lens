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
  -> local client: validate + authenticated TLS Prepare (no executable values)
  <- server: Prepared, authenticated principal/resource binding pinned to connection
  -> client: compare frozen domain -> existing-token restore -> bound Call
  -> server: recheck binding/grants + bounded DB/log/inspection execution
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
configured TLS identity pin, credential reference, configured resource ID and class
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

Before any executable-value disclosure, authenticate with `Prepare` (section 6).
The server resolves the credential principal and resource and returns their stable
opaque identities/generations. Compare this `DestinationBinding` and the configured
TLS identity with the frozen domain **before strict restoration or Call**. Endpoint,
alias, credential bytes and resource name alone do not establish that binding.
The server pins the same resolved resource object and principal for the connection
and call; it never looks up a replacement object by alias after accepting Prepare.

Initial enrollment starts with an empty local session and operator-configured TLS
server-name/trust roots plus an explicit server identity pin (SPKI SHA-256). Only
that authenticated server's Prepared binding may populate the configured domain;
complete every explicitly permitted destination's enrollment before issuing maps.
Persist identities/generations across server restarts; never reuse them for another
principal/resource. Changing a principal mapping or resolved resource definition
changes its generation. A reconnect may retain maps only on exact binding equality.
TLS certificate renewal with the same pinned key preserves identity. Key rotation
requires a new session unless the replacement pin was explicitly enrolled as the
same server identity in the original frozen domain; no live pin/domain widening.
Resource/principal rebinding requires a new session. Never automatically copy maps
on rotation/rebinding or import them across domains. Process-wide retention keeps
the existing least-destructive rules for shared storage.

Prepared is the authenticated trusted server's assertion, not cryptographic
attestation of its source or honesty. A malicious trusted server can lie about
bindings or content; local output protection remains the response boundary, with
limited detector coverage. Fixed opaque binding fields are private routing control,
not arbitrary server metadata or content forwarded to the agent.

Grants are an explicit set of `(principal, operation, resource)` tuples, with
`inspect` additionally bound to `(view, collector)`. No wildcard operations or
implicit permission inheritance. Exact operations are defined in section 6;
`schema` or `list_tables` permission never implies `query`. Each grant has an
enabled flag, expiry, and operator ceiling for budgets. Credential digests and
principal IDs are server-owned; unrecognized/duplicate/ambiguous grants deny.

Prepare checks the current operation/resource grant (for inspect, at least one
explicit view/collector grant on that resource). The server reopens/revalidates
current grants, credential-to-principal and alias-to-resource mappings, and pinned
generations before invoke and remote release, including exact table/column and
inspection scope from Call. Removed or changed mappings/generations reject even if
an equivalent grant now exists; these checks cannot replace the pinned object.
The final check is the revocation linearization point; it cannot recall bytes
already released.
Malformed/unavailable grant state denies, including after a successful collection.
Full package-inventory access authorizes presence/count observations across pages;
redacting names/versions does not hide package existence or inventory size.
Opaque call IDs correlate server access metadata with client audit. Neither log
stores request values, SQL binds, raw source text, tokens or driver error text.

## 4. Request preparation and audit omission

For executable data fields: validate the public shape and bounds; resolve the
frozen destination through authenticated Prepare/Prepared; restore only existing
local session tokens in one frozen read-only transaction; validate expanded shape,
destination and cumulative escaped JSON byte budget; send the bound Call;
independently authorize and validate on the server.
No request PII detection, redaction or new mapping creation. Known raw literals
stay unchanged. Restoration is one pass, not recursive expansion of restored text.
Structural fields such as operation, profile, mode, offset, limits and enum tags
are not restored.
Keyword terms are the local-only exception in section 7 and are never restored.

Use the signed upstream bounded helper contract
`SessionTransaction::restore_strict_text_bounded`, backed by
`strict_restore_tokens`. Preserve `UnknownToken`, `CapExceeded`, `MalformedToken`;
allocation failure maps to `CapExceeded`. Unknown future failures also deny without
formatting their payloads. A post-allocation size test alone is insufficient:
count expansion and JSON escaping cumulatively before allocation/reservation.
Foreign/unknown/malformed tokens and amplification fail with fixed client errors
and no Call or executable-value send; Prepare contains none. The transaction creates
no request-derived mappings. Restored predicate strings remain strings, unchanged
on the wire. Only the server normalizes them against its source schema for typed
SQL binds (section 6); the client never guesses a number from its spelling.

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
Implement raw/default and allowlisted schema presentation through typed name-only
paths: `table_schema.table`, `table_schema.columns[].name` and `table_list.tables[]`.
Never restore the whole schema document after Gaze. In particular `data_type` can
contain sensitive source text and must remain protected in every presentation mode.

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
validation, the configured identity pin, and per-principal 256-bit credentials in
protected transport metadata (exactly 64 ASCII hex characters from an operator
env/keyring reference).
No plaintext, insecure verification, redirect, capability guessing, downgrade or
silent legacy fallback. Keep remote content outside the RMCP runtime.

Freeze these closed semantic DTOs in the protocol crate before transport work.
All objects reject unknown/duplicate keys at every depth. `?` below means optional;
all other fields are required. Enum strings are exact and unknown values fail.
Use bounded newline-delimited JSON over TLS, one operation per connection:

```text
Prepare {version: "gaze-lens-source/2", privacy: "client_gaze", id, credential, resource, operation}
Prepared {version, privacy: "client_gaze", id, operation, binding: DestinationBinding}
Call    {version, id, operation, binding: DestinationBinding, args}
Success {version, id, operation, binding: DestinationBinding, result}
Failure {version, id, code}
DestinationBinding {principal, principal_generation, resource, resource_generation}
```

`version` always equals the exact version above; `id` is a client-generated opaque
ULID echoed exactly. Each binding field is exactly 32 lowercase ASCII hex characters,
an opaque server-owned 128-bit identity or generation, never source labels. Prepare's
`resource` is the configured alias; binding's `resource` is its durable opaque identity.
Credentials occur only in Prepare transport metadata, never `args`. Prepare contains
no table, predicates, terms or other executable values. Sequence is Prepare, Prepared,
one bound Call, one Success/Failure, then close, all on the same TLS connection.
Prepared authenticates the principal and pins the resolved resource object; client
binding comparison precedes restoration. No Call may precede Prepared or change its
ID, operation or binding. Readiness uses this same exchange with empty Call args.
Authentication/parse errors before a valid authenticated Prepare close with a fixed
local failure; no guessed ID. Later failures use its ID. Reject mismatched response
binding/ID/operation/result, unsolicited frames, batches, streaming, additional
negotiation, notifications and generic tool enumeration. Binding state is per
connection only; reconnect starts a fresh Prepare, not a cached authorization.

Exact operation/args/result pairs and independently required grants:

- `query`: `{table, columns?, where?, where_combinator?, order_by?, limit?}` →
  `query_rows`. Keep existing scalar/list predicates: clauses `{col,op,val?}`, ops
  `eq/ne/gt/gte/lt/lte/in/like/is_null/is_not_null`, combinator `and/or`, ordering
  `{col,dir:asc|desc}`. No nested object predicate values or new SQL grammar.
  Normalize legacy public `value` alias to `val` locally. Server alone compiles
  bound SQL and expands omitted columns from its allowlist. Check selected,
  WHERE and ORDER columns, table and read-only role, including after restoration.
  Apply server-schema-directed normalization below to every predicate operand.
- `schema`: `{table}` → `table_schema`; `list_tables`: `{}` → `table_list`.
  Both are filtered by server presentation/access scope. Metadata cannot widen
  subsequent query authorization.
- `log_tail`: `{lines}` → `log_window`.
- `log_window`: `{}` → `log_window`, specifically for the fixed local keyword window.
  No pattern, terms or level filter is sent. A tail grant does not imply this grant.
- `log_regex`: `{pattern,level?,limit}` → `log_matches`. Preserve raw-regex
  semantics, production warnings and any stricter operator restriction. No keyword
  substitution or external caller-chosen executable. Regex is an explicit grant.
- `inspect`: `{view,collector,limit,offset?}` → `inspection`; closed views in section 8.
  Offset is package-only; public callers choose view, limit and package offset,
  while the profile selects collector ID. Other views reject any offset.
- `readiness`: `{}` → `readiness`. Reports authorized resource availability in
  server configuration only; no source access or probe. This is a private
  operation for client `check`, not a seventh public tool.

### Server-owned predicate normalization

Resolve column types from the pinned source's authoritative schema, never schema
presentation text or client hints. Preserve the scalar/list query grammar and wire
literals; normalize only inside the server. Use each adapter's existing supported
native types and native SQL bind types, with no client compiler or new general type
system. All operands, including **each IN element**, validate before executing the
query. Range, precision, format, operator or type mismatch is `invalid_request` with
no payload. Do not rely on backend permissive casts, truncation or coercion.

- Integer columns accept integral JSON numbers or strict base-10 integer strings
  (`-?(0|[1-9][0-9]*)`); unsigned columns reject negatives. Enforce the actual column
  width/sign, then bind that native integer type, including PostgreSQL INT2/4/8.
- Decimal columns accept exact JSON numbers or plain decimal strings (integer grammar
  plus optional `.[0-9]+`, no whitespace/plus/exponent). Validate native precision,
  scale and range without rounding; fractional zeros beyond declared scale may be
  removed only without changing value. Bind native decimal, never TEXT or f64.
  Finite float columns accept JSON-number grammar in a number or string and bind the
  native width; reject nonfinite, overflow/underflow and precision loss beyond that
  width's round-trip representation. Exact JSON lexemes must survive request parsing.
- Text columns accept strings only and bind unchanged text, including `00123`,
  `1e3` and restored numeric-looking text. Boolean columns accept JSON booleans only.
  UUID and supported temporal columns accept strings validated in their native
  semantic representation below, then native binds, without added dates/timezones.
  Bytes accept canonical base64 strings with the decoded-length cap. JSON columns
  accept strings containing bounded exact JSON, parsed without f64 loss and bound
  natively; this does not permit object-valued predicate args.
- `eq/ne/in` use these supported bindings; `in` requires a nonempty scalar list.
  `gt/gte/lt/lte` require numeric, text or supported temporal columns; `like` requires
  text. `is_null/is_not_null` omit `val` and need no bind. Null operands otherwise
  reject, as do lists on scalar operators and values on null-test operators.
  Native types/operators the adapter cannot support exactly reject before execute.
  SQLite uses its supported declared type/affinity and guarded storage-class
  comparisons; ambiguous/unsupported declarations reject rather than guessing from
  literal spelling or relying on SQLite coercion. JSON-in-TEXT remains allowlist-only.

A protected PostgreSQL numeric value restored as a string must successfully find
the same row with `eq` and as an `in` element. Rejecting all converted operands is
not compliance; phase 1 freezes these semantics and phase 2 supplies driver proof.

Closed result variants (`kind` discriminator, no extra fields):

```text
query_rows   {kind, columns: [text], rows: [[Value]], truncated: [Reason]}
table_schema {kind, table: text, columns: [{name: text, data_type: text, nullable: bool}], truncated: [Reason]}
table_list   {kind, tables: [text], truncated: [Reason]}
log_window   {kind, lines: [text], window: Window, truncated: [Reason]}
log_matches  {kind, lines: [text], window: Window, matched_lines: uint, truncated: [Reason]}
inspection   {kind, view, collector: text, status, evidence, records, page: PackagePage|null, truncated: [Reason]}
readiness    {kind, status: "configured"}
Window       {scope: "tail_window", scanned_bytes: uint, scanned_lines: uint, admitted_lines: uint}
PackagePage  {offset: uint, returned: uint, total: uint, next_offset: uint|null}
```

Rows must have exactly the declared column count; duplicate column/table labels
and truncation reasons reject. Window counters describe acquisition and admitted
complete lines, never the whole source. `matched_lines` is window-scoped and cannot
exceed admitted lines; the client derives keyword counts after local matching.
`Reason` is exactly `bytes`, `rows`, `lines`, `scan_bytes`, `scan_lines`, `boundary`
or `records`. Section 7 defines the only valid operation-specific reasons and
retention order. Reaching a bound between complete records may truncate there;
malformed or oversized individual values/lines/records fail without clipping.
Package paging/count relationships and fixed local structural presentation are
defined in section 8; other views require `page: null`.

`Value` uses a `kind` tag and exactly the associated fields: `null` (none), `bool`
(`value`), `i64`/`u64` (`value`: canonical decimal string in exact range), `f64`
(`value`: finite round-trippable decimal string), `decimal` (`value`: decimal
string, `precision`: 1..255, `scale`: 0..precision), `string` (`value`), `bytes`
(`base64`, `len`: exact decoded byte count), `datetime` (`semantic`: exactly
`date|time|naive|zoned`, `value`: validated semantic text below), `uuid`
(`value`: canonical UUID text), `json`
(`value`: bounded JSON). Decimal digits must fit declared precision/scale; JSON
numbers retain their exact parsed representation without an f64 detour. Reject
NaN/infinity, invalid encoding/type and length mismatches. Unsupported native DB
types reject the row. Server SQLite JSON-in-TEXT remains allowlist-only.

### Native-to-wire conversion matrix

This corrects legacy normalization for already-supported types; it does not add
native type coverage. Phase 1 requires positive wire fixtures and phase 2 positive
adapter fixtures for each supported case, not a blanket "survive or reject" test.

- Native signed/unsigned integers become exact `i64/u64`; supported finite floats
  become round-trippable `f64` text without changing their decoded value. Booleans,
  null and text retain their semantics. Bytes use canonical padded RFC 4648 base64
  plus exact decoded length; UUID uses lowercase hyphenated canonical text.
- Decimal coefficient/exponent becomes plain decimal text without rounding. Set
  scale to fractional digit count and precision to `max(1, significant integer
  digits + scale)`, counting fractional leading/trailing zeros. Thus `0.001` has
  precision 3/scale 3, `1.2300` has 5/4, and coefficient 12 with native scale -2
  becomes `1200`, 4/0. Check expansion before allocation; values exceeding 255
  precision/scale or other caps fail. Do not clamp native metadata or lose zeros.
- Supported DATE becomes `datetime(date, YYYY-MM-DD)`; PostgreSQL TIME becomes
  `datetime(time, HH:MM:SS[.fraction])`; PostgreSQL TIMESTAMP/MySQL DATETIME become
  `datetime(naive, YYYY-MM-DDTHH:MM:SS[.fraction])`. No fabricated midnight, epoch
  date, UTC or offset. Supported zoned timestamps become `datetime(zoned, RFC3339)`
  preserving the decoded instant, offset and available precision. PostgreSQL may
  supply UTC rather than the original input zone; MySQL TIMESTAMP needs an explicit
  server DB-session timezone for decoding. Never infer an original timezone the
  source did not retain. Existing unsupported temporal ranges/types still reject.
- Native JSON/JSONB and allowlisted SQLite JSON text decode through an exact-number
  path from the source bytes, preserving the source-decoded numeric representation
  (JSONB may already normalize it). Ordinary `serde_json::Value` via f64 is
  insufficient; a later serializer cannot recover lost digits. Include integers
  beyond 2^53, high-precision fractions and exponents, plus dynamic keys, in positive
  source-to-wire-to-protected fixtures. Parsing remains depth/node/byte bounded.

`Failure.code` is exactly `invalid_request`, `unauthorized`, `unsupported_version`,
`unsupported_operation`, `unavailable`, `cap_exceeded`, `timeout`, `binding_changed`
or `internal_failure`. `binding_changed` closes the call on any pinned
identity/generation change; no reprepare/retry with restored values. No message/details
field. Local request-restoration failures are client-only, not upstream payloads.
Never expose driver or collector errors.

## 7. Budgets and logs

These conservative v2 hard ceilings may be lowered by operator grants/local
policy; exceeding one fails or explicitly truncates only where specified above.
Increasing hard ceilings requires protocol review and boundary tests.

- Frames: 1 MiB including newline; executable request: 64 KiB including escaped
  restored JSON. Result body: 128 KiB. Depth: 32; logical nodes: 16,384; scalar
  source text/decoded bytes: 8 KiB; identifiers: 256 UTF-8 bytes; configured IDs:
  1..64 ASCII letters/digits/underscore/hyphen, public profile grammar unchanged.
  Prepare/Prepared each max 4 KiB including framing; validate fixed binding fields
  before allocation/comparison. Their work counts within the same 30-second call.
- Queries: default 100/max 1,000 rows; max 128 columns, 64 predicates, 16 order
  fields, 1,024 total binds including IN lists. Schema/table lists: 1,000 entries.
  No query/schema pagination or cursor is added. No unbounded materialization
  before these checks.
- Logs: default 100/max 1,000 returned lines, 8 KiB per line, 1 MiB scan bytes,
  16,384 scanned lines. Regex expression max 4 KiB plus bounded compiled-regex
  memory (1 MiB). No unlimited source scan to satisfy a result limit.
- Inspection: default 100/max 500 package records; host one record; PHP at most
  32 configured runtimes/services. Collector input/output and combined stdout +
  stderr acquisition max 1 MiB; result caps still apply. Inventory scan max
  100,000 records; package offset default 0/max 100,000. Each package page requires
  a complete bounded rescan. A bounded inventory buffer/sort is permitted with a
  1 MiB working-memory ceiling for records, sort indexes and transient copies,
  measured independently of the 1 MiB acquisition cap. Failure to complete within
  either cap returns `cap_exceeded`, never a falsely complete prefix or suffix.
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

### Fixed log windows and exact truncation

Capture a source end position per call and acquire backward from that end within
1 MiB and 16,384 scanned lines. Concurrent append is outside this call; detected
rotation/invalidation fails `unavailable`. Admit complete lines only, in original
source order. A partial line at the oldest scan boundary or unterminated end is
omitted with `boundary`; encountering a complete line over 8 KiB (or exceeding that
length while locating its boundary) rejects `cap_exceeded`, never skips the line.

- `log_tail` retains the newest whole-line suffix of at most requested `lines`
  (default 100/max 1,000) that fits the 128 KiB result body including metadata/JSON
  escaping. `log_window` always uses the same rule with **1,000**, independent of
  the public keyword match limit. A lower operator window ceiling applies to every
  keyword call equally. Under byte pressure drop oldest whole lines; stop at the
  first line that cannot fit when extending backward, never skip holes to add older
  smaller lines. This is a deliberate reduction from the legacy 10,000-line keyword
  tail to at most 1,000 lines, possibly fewer under byte pressure.
- `log_regex` filters the complete bounded scan window by regex and optional level,
  counts every match in that window, then returns a source-order prefix of matches
  up to `limit` and the result-body byte cap. Stop before the first match that cannot
  fit, with no skipped holes. Result truncation does not stop window match counting.
  No operation scans beyond acquisition limits to fill a result limit.

For logs, `scan_bytes`/`scan_lines` mean older source remains beyond the respective
acquisition ceiling; `boundary` means an incomplete boundary line was excluded.
`lines` means complete candidates were excluded by the tail/window line ceiling or
regex match limit; `bytes` means the result-body cap excluded complete candidates.
Report every applicable reason once, in the enum order in section 6, and no reasons
for bounds merely equal to a complete result. `line_bytes` is not a truncation reason:
an oversized line rejects. Window `scanned_bytes`/`scanned_lines` cover acquisition
(including boundary fragments), and `admitted_lines` is the complete retained window
before matching: equal to `lines.len()` for log_window, full scan for log_matches.
Counts describe only this window, never a whole-file total.

For `query`, retain a result-order prefix; only `rows` (known further rows beyond
the effective limit) and `bytes` apply. No stable query ordering is promised without
`order_by`. Schema/table lists use a UTF-8 label-order prefix, with `records` and/or
`bytes`. Inspection uses configured-ID order for PHP and package ordering/paging in
section 8, with `records` and/or `bytes`; host/readiness never truncate. Detecting
further records uses bounded lookahead, never unlimited acquisition. Oversized
individual records reject. Inventory scan limits fail, not truncate. No other
operation/reason pairing is valid.

Keyword `log_grep` requests `log_window` with empty args, protects and durably audits
the **entire admitted window** locally, then indexes/filters using original
keyword/token spelling and the optional local level filter. Count `searched_lines`
as admitted protected lines and `matched_lines` only after both level filtering and
case-insensitive AND matching, including complete issued tokens and structured
key/value terms. Count across that entire window **before** truncating returned
matches to the requested limit or final protected-output byte budget. Return a
source-order prefix without holes; derive counts locally and union window reasons
with `lines`/`bytes` for local truncation. If protection itself exceeds its hard
codec/output caps, fail rather than protecting only a match prefix. Terms and level
never go remotely; raw keyword literals cannot become a server-side presence oracle.
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

Public `inspect` args are exactly `{profile, view, limit?, offset?}`; `view` is `host`,
`packages` or `php`. Profile configuration selects the trusted collector target;
no caller path, binary, shell, unit name or arbitrary filter. Default/max limits
are section 7. For `host`, require limit 1 if supplied; client defaults it to 1.
For `php`, default/max limit is 32, with records ordered by configured runtime ID.
`offset` is a nonnegative integer, default 0/max 100,000, accepted **only for packages**;
even an explicit zero on another view rejects. There is no query pagination,
general filter, cursor, cache, new tool or server pagination session.

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
records and `page: null`; successful package pages use `ok` with a complete scan
and explicit paging below. Nested PHP observations can independently be
unknown/unavailable. No free-form diagnostics.
Exact records, with all fields required and no additions:

- `host`, evidence `os_release`, one record:
  `{os_id: text, version_id: text|null, architecture: text}`. Fixed OS release
  fields and architecture only. No hostname, usernames, addresses, machine IDs,
  environment, process list, uptime or generic monitoring measurements.
- `packages`, evidence `dpkg_database`, records:
  `{name: text, version: text, architecture: text}`. Installed dpkg entries only,
  ordered by `(name, architecture)` using ascending UTF-8 byte order, independent
  of locale; duplicate pairs reject. No package description, maintainer/contact
  data, file lists, config, install scripts or
  install/update/removal action. An incomplete capped scan fails `cap_exceeded`.
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

### Package-only stateless pages

Reauthorize and rescan the entire installed inventory on **every page**, within
1 MiB acquisition, 100,000 scanned records and the bounded working-memory/time
budgets. Buffer/sort only within those limits. An acquisition, parsing or sorting
cap failure returns fixed `cap_exceeded` without records or paging metadata;
never sort an incomplete scan and present its suffix as complete.

After a complete scan, `total` is the installed-record count (max 100,000). Skip
exactly `offset` sorted records, then return a contiguous prefix from there bounded
by `limit` (default 100/max 500) and 128 KiB result body. If offset >= total, return
an empty page with `next_offset: null`. Otherwise at least one whole record must
fit or the call fails. `returned` equals records length; `next_offset` is exactly
`offset + returned` only when that is less than total, otherwise null. `records`
marks a page-limit cut with remaining records; `bytes` marks a byte-limit cut.
No opaque continuation or retained server state exists. Package changes between
calls can shift pages, duplicate or omit observations; no snapshot consistency is
claimed. Repeating an offset rescans and reauthorizes rather than replaying a cache.

The client derives `offset` from the validated request and `returned` from admitted
records, validates bounded `total` and these `next_offset` relationships, and emits
only fixed structural paging fields/counts. It must not allocate from server counts.
All source labels, versions, architectures and collector IDs still pass through
local Gaze. Page structure and validated counts are the limited structural exception,
not proof of a malicious trusted collector's completeness. An inventory grant
authorizes these presence/count observations; redaction does not hide existence.

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
configure pinned client identities and enroll empty explicit domains using Prepare;
move source allowlists to the server while retaining stricter client privacy policy;
validate both configs;
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
Keep the existing **unpublished development assembly** and its legacy modules/callers
building through phases 1-3 while extracting the new crates. It is temporary workspace
scaffolding, never a split release or a new client-to-server-library dependency.
Every phase PR must retain an ordinary green workspace build. Early A dependency
checks apply to the crates actually extracted; remove the legacy assembly, duplicate
source modules and direct callers at phase 4 cutover, then require full A at phases
4 and 6. Do not move files away from still-live legacy callers without that bridge.

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
Freeze authenticated Prepare/Prepared binding fields and sequence, native-to-wire
conversion success cases, schema-directed bind semantics, fixed log windows and
package-only page DTOs before independent transport/adapter work.
Proof: protocol fixtures, duplicate/unknown-field and positive typed-value tests,
independent extracted-crate builds and their normal/supported-feature dependency
trees plus the green legacy assembly (A, C, H).
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
Pin authenticated principal/resource objects before disclosure, enforce current
generations/grants at invoke/release, and implement source-schema-directed typed
binds, including every IN operand. Replace legacy decimal/time/JSON conversion with
the positive semantic matrix, without broadening supported native types.
Choose and prove a bounded acquisition strategy **per driver** before declaring
the phase complete: cover huge single TEXT/BLOB/JSON cells, schema acquisition,
driver buffering and JSON parsing **before DTO allocation**. Evaluate bounded SQL
projection/length guards, supported driver limits or an isolated bounded worker;
post-fetch size checks, `fetch_all` replacement or row counts alone are not proof.
Use synthetic oversized-cell/driver-buffer fixtures with measured peak allocation,
cancellation and pooled-connection cleanup evidence for MySQL, PostgreSQL and SQLite.
If an adapter cannot establish the acquisition bound, its phase gate remains blocked.
Proof: synthetic DB/log fixtures, grant/column/read-only denial, revocation,
cancellation, acquisition budgets, restored-string numeric eq/IN binds and extracted-server
no-Gaze dependency proof, while legacy callers still build (A-D, H).

### Phase 3: preserve local request, response and replay boundary

Owner: client privacy-boundary implementer. Depends on phase 1 and matching
published upstream API; uses pending c5933af9 as read-only behavioral reference.
Move/adapt `src/session/{mod,request,response_codec,boundary,restore,manifest}.rs`
(the request/codec/boundary modules are pending, not on the base),
`src/manifest/gaze_mcp_adapter.rs`, `src/policy.rs` and remote watchdog into
`crates/gaze-lens/src/{session,manifest,policy,transport}`.
Implement immutable authenticated DestinationBinding/session registry, enrollment,
pre-restoration comparison, frozen restore, metadata-only audit,
closed upstream admission, numeric/key carriers and durable release. Preserve
legacy/new/mixed replay before removing old request-capture assumptions.
Replace the legacy whole-schema post-Gaze restoration with typed name-only
presentation; canary `data_type` stays protected even in raw-name mode.
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
fix and regex caveat. Enforce the fixed 1,000-line maximum window, newest byte-bounded
suffix and window-scoped counts before match truncation. Remove legacy development
assembly/direct-source modules at this cutover; full A now applies to the workspace.
Public inspect discovery waits for phase 5 support.
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
protocol or public args. Implement package-only stateless offsets with complete
bounded rescan/sort, deterministic pages and validated structural counts; test
reachability after record 500, cap failure and package changes between calls.
Proof: malformed/caller path/executable denial before execution,
unsupported/unavailable states, hostile names/versions through Gaze,
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
points; manifest SQLite is the sole client DB exception. Early extracted-crate
checks coexist with a green unpublished legacy assembly; full A at phase 4/6 proves
its removal. Typed predicate compilation and inventory acquisition remain server-only.

**B. End-to-end.** Synthetic query/schema/tables, log window/regex and each supported
inspect view traverse client→TLS→server→source→local envelope→durable audit→agent.
Stopping the server makes a new query impossible; no direct fallback or hidden probe.
Exercise Prepare/Prepared before Call, numeric-token eq/IN round trips and a package
inventory with more than 500 records reached through successive authorized pages.

**C. Typed data.** i64/u64 extremes, decimals, finite/nonfinite float, bytes mismatch,
UUID and date/time/naive/zoned semantics, nested JSON dynamic keys and numeric
canaries, unknown variants, invalid rows, key collisions and SQLite JSON allowlist.
Require successful supported matrix cases: `0.001`, trailing zeros, negative decimal
scale, no invented dates/zones, exact >2^53 integers/fractions/exponents from source
decode and canonical bytes. Separately assert invalid/unsupported cases reject.
Protected source numbers may become tokens; no reject-everything compatibility pass.

**D. Query authority/schema.** Omitted-column expansion, denied table/SELECT/WHERE/
ORDER columns, forged allowed metadata and schema raw/tokenized/allowlist modes.
Client hints and schema presentation never widen server authorization.
Test same-row PostgreSQL BIGINT/NUMERIC token-to-eq/IN success, each IN element's
native bind, numeric-looking TEXT unchanged, and range/scale/format/type mismatch
before execute. Cover each supported adapter/operator contract, including SQLite
storage-class guards. Raw-name presentation never restores a sensitive `data_type`.

**E. Request trust.** Literal unchanged; owned token restored once; unknown/foreign/
malformed token and repeated huge mappings fail fixed at N/N+1 bounds; zero request
detector calls/new maps. Prepare contains no executable values. Unchanged local alias,
credential and TLS config with a changed remote principal/resource generation sends
zero restored bytes; unchanged authenticated reconnect succeeds. Exercise empty-session
enrollment, pin rotation rules, no automatic map copying and in-call generation
changes. Explicit multi-host fixture permits only enrolled, intended disclosure.

**F. Keywords.** Original complete issued-token term matches protected local window;
raw literal gains no remote keyword oracle. No terms/level in window request;
headers excluded. A match followed by a nonmatching line still returns for limit 1;
level-excluded hits do not count. Cover 1,000 versus legacy 10,000-line migration,
1 MiB scan/128 KiB result boundaries, newest whole-line suffix with no holes,
oversized-line failure, exact operation reasons and whole-window counts before match
limit/bytes truncation. Revoke/policy/resource change cannot release stale-authorized
cache content; initial path has no cache.

**G. Release failures.** Gaze, snapshot write/fsync, manifest begin/finish and codec
faults release no content. Driver error, stderr, malformed content, request echo
and tracing canaries are absent from agent errors/audit/telemetry. Prior committed
replay survives later interruption; watchdog cancellation/panic remains fail-closed.
Inject sensitive schema `data_type` and package name/version/architecture canaries;
only typed schema name exceptions and fixed validated paging/count fields bypass
text carriers. Whole-document post-Gaze restoration must fail the schema fixture.

**H. Protocol/authority.** Wrong identity/cert, expired/revoked/other-operation grant,
mid-read revoke, duplicate fields, unknown version/ID/result, oversize/deep JSON,
slow reads/writes, amplification, concurrency and cancellation fail bounded.
No downgrade or hidden source execution. Prepare/Prepared oversize and malformed
binding fields, Call before Prepare, changed Call binding, server rebind between
prepare/invoke/release and final grant checks are exercised. Per-driver huge-cell
and buffer fixtures prove bounded acquisition before DTO allocation, with measured
peak memory and cancellation. A row limit or post-fetch cap alone cannot pass H.

**I. Inspection truth.** Installed PHP differs from configured CLI and FPM;
application binding can remain unknown. Unavailable is not inferred success.
Caller path/executable/unknown view rejects before collector execution; platform,
collector identity, stdout/stderr limits and process cleanup are fixture-tested.
Package pages cover default/max/invalid offsets, offset rejection on other views,
deterministic name/architecture order, byte-limited contiguous pages, offsets beyond
total, and validated returned/total/next_offset relationships. Every page reauthorizes
and completely rescans; >1 MiB/>100,000-record/working-memory failure cannot produce
a success page. Measure bounded buffer/sort peak memory. Mutation-between-pages
fixtures allow shifts, not a snapshot claim; inventory grants expose presence/counts.

**J. Migration/replay.** Old direct and legacy remote profiles reject actionably;
legacy/new/mixed snapshots replay offline with zero server/DB calls. New args show
omission, old captured args restore, purged output errors stay explicit. Demo is
offline/ephemeral; discovery exactly matches six agent tools, six client commands
and two server commands. Public docs describe only implemented behavior.
Document pinned empty-session enrollment/rebinding, the smaller keyword window and
package-only offset semantics without adding query pagination or claiming runtime
tests from this spec PR. Preserve offline replay across the temporal wire changes.
