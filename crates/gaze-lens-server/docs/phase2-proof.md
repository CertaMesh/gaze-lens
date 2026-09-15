# Phase 2 development proof and unresolved acquisition gate

Status: incomplete, not an execution-server release. Phase 2 task Solo project 6
#3609 remains in progress until independently reviewed and merged. The normative
[contract](../../../docs/reference/server-client-split.md) is unchanged.

Base: `442fb4912e5f486976316dfcc5825913c3365c69`.
Base tree: `9dbffd6cb26454f619cadc66a043ce1ae7089714`.

## Implemented slice

Separate unpublished server `serve --config` and `check --config`. The only
implemented exchange is config-only readiness, over verified TLS in the synthetic
client fixture. Prepare authenticates a credential digest and exact readiness
grant, owns copies of the resolved principal/resource objects, and returns the
v2 binding. Call must preserve ID, binding and the operation pinned at Prepare;
that comparison reads `Pinned::operation`, not a repeated operation literal. The
authority file is reopened before invoke and release. Revoke, expiry, malformed
authority and object/generation changes deny. Other operations do not receive
Prepared or request executable values. No source adapter, source credentials,
Gaze, maps, manifest, replay, client fallback or inspection collector is here.

Resource class is a placeholder for the forthcoming source descriptor. Readiness
here establishes only that placeholder and its exact grant in configuration.
It does not establish source connectivity, usability or complete server setup.

Four active connections globally, two per principal, no waiting queue. TLS is
1.3 only. TLS and frame I/O have 10-second deadlines inside one 30-second
connection deadline; a deadline closes the connection without a Failure frame.
Frames use phase 1 admission buffers. A failed `accept` is classified rather
than fatal: a rejected peer retries at once, an exhausted resource pauses 100ms,
and only an unusable descriptor or 64 consecutive resource failures ends the
service.

One private-file reader admits every trusted operator file - configuration,
certificate, private key, authority and identity history. Each is capped at
64 KiB before parsing, and on Unix must be a regular file (settled on the link,
so a FIFO never reaches `open`), with no group or other permission bits, owned
by the process euid, matching the inspected device/inode, in a directory that is
itself a real directory with the same privacy and ownership. The history `.lock`
is created with `create_new` or reopened under the same checks. Non-Unix
permission validation is deliberately unavailable pending an ACL proof. No
logging subscriber or TLS key logger is installed; errors contain only protocol
codes; the PEM key text is zeroized after the TLS acceptor is built.

### Operator file handling

- **Apply authority-file edits by atomic rename.** Write the new file beside the
  old one, `0600`, in the same `0700` directory, then `rename(2)` it into place.
  The server rereads the authority before invoke and before release, so an
  in-place edit can be read half-written and will simply deny.
- **Credentials must be 256 bits of randomness, rendered as 64 lowercase hex
  characters** - for example `head -c 32 /dev/urandom | xxd -p -c 32`. The
  authority file stores only their SHA-256. A hex-encoded passphrase has the
  entropy of the passphrase, not of its 64 characters, and the digest is
  unsalted and fast; the length check is a shape check, never a strength check.
- Config, certificate, key, authority and history must all live in trusted
  operator-controlled `0700` directories owned by the server's user.

## Acquisition feasibility, locked SQLx 0.8.6

This is source inspection, **not measured allocation or driver execution proof**.
The ordinary adapters must not be copied into the new server as if streaming rows
alone established H. SQL guards may bound successful result cells while leaving
schema preparation, error/notice messages and connection setup unbounded.

### PostgreSQL

`PgStream::recv_unchecked` reads a source-provided 32-bit message length and asks
`BufferedSocket::try_read` for that complete message before decoding it. The
adapter cannot reject a large DataRow, RowDescription or error before that buffer
has grown. A bounded SELECT projection does not bound every message class.
[Locked driver source](https://github.com/launchbadge/sqlx/blob/v0.8.6/sqlx-postgres/src/connection/stream.rs).

Candidate: a reviewed driver receive-budget mechanism covering startup, metadata,
rows, errors and notices, or a separately contained worker with a hard memory limit
and mandatory kill/reap on cancellation. Length-guarded SQL remains useful but is
not the sole boundary. No such mechanism or measured cancellation proof is in this
slice. Local Yerd inventory has no installed/running PostgreSQL; no service was
installed or started. Positive same-row NUMERIC/BIGINT restored-string eq/IN and
schema/oversized-cell fixtures therefore remain unrun.

### MySQL

`recv_packet_part` reads the packet length; `recv_packet` allocates an assembly
buffer for full-size packets and appends continuation packets until a shorter
packet arrives. This occurs before the row decoder and adapter. A row count or
post-fetch size check cannot constrain that allocation.
[Locked driver source](https://github.com/launchbadge/sqlx/blob/v0.8.6/sqlx-mysql/src/connection/stream.rs).

Candidate: a receive/continuation and metadata budget enforced inside the driver,
plus bounded projection and native decode, or the contained-worker strategy.
Yerd MySQL 8.4.9 was already running; it was not probed. Synthetic database creation
and driver allocation/cancellation/pool cleanup fixtures are not implemented.

### SQLite

`SqliteRow::current` creates values for each result column; `SqliteValue::new`
calls `sqlite3_value_dup` before consumer checks. `row_buffer_size(1)` limits row
count, not SQLite native heap or an individual cell. Rust allocator counters alone
would miss SQLite's C allocations.
[Row source](https://github.com/launchbadge/sqlx/blob/v0.8.6/sqlx-sqlite/src/row.rs),
[value source](https://github.com/launchbadge/sqlx/blob/v0.8.6/sqlx-sqlite/src/value.rs).

Candidate: supported SQLite length/schema/heap controls established before open
and prepare, together with guarded projections, progress interruption and worker
cleanup. Alternatively use a contained worker. The safe public SQLx options
inspected do not establish the complete native-allocation contract. Any required
FFI/dependency adjustment needs scoped review; no unsafe production code or global
SQLite allocator change has been introduced. Local synthetic SQLite fixtures are
possible but the complete bound is not implemented or measured.

### Decision required before source execution

Choose one reviewed containment strategy and its supported platform before adapter
activation. A hard-limited worker needs a proven limit for native allocations,
process-group termination/reaping, bounded IPC, no secret/error logging and
connection disposal. A watchdog that samples RSS after allocation is insufficient.
A driver change needs independent preallocation limits for every inbound message
and metadata path, not only DataRows. Neither option is claimed complete here.
No upstream pin/publication change or new external service is authorized by this
note. Existing safety requirements remain the acceptance boundary.

### Contained-worker evaluation, design only

A concrete candidate for review is a Linux worker with both an address-space
limit established before driver initialization (`RLIMIT_AS`) and a delegated
cgroup v2 memory/process limit. Address-space limits cover reservations that an
RSS sampler can miss; cgroup controls constrain charged memory and descendants.
The kernel documents that `memory.max` can be exceeded temporarily, so it alone
is not an exact preallocation proof. See the [Linux resource-limit manual](https://man7.org/linux/man-pages/man2/getrlimit.2.html)
and [kernel cgroup documentation](https://docs.kernel.org/admin-guide/cgroup-v2.html).

The parent would admit at most one worker per active call, send only the fixed
validated operation over bounded IPC after pin/grant checks, require limits to be
confirmed before any driver starts, and kill/reap the entire worker group on the
five-second source deadline or cancellation. No source pool could survive that
worker; pooled cleanup would mean disposing of the worker's pool, not returning a
possibly desynchronized connection. Parent result admission and final authorization
would still precede release. No generic exec or extra public command is proposed.

This is a candidate, not a chosen/proven implementation. Required evidence is a
Linux fixture with delegated limits, measured address-space/native/Rust peaks,
startup and schema-message attacks, huge TEXT/BLOB/JSON, failed allocation handling,
positive typed conversions and verified child/pool teardown for each driver.
The current macOS host does not provide that Linux fixture. No worker, cgroup,
external service or new platform claim has been enabled in this change.

## Phase proof matrix

- **A, partial:** new server depends on protocol/TLS/config/auth libraries only;
  legacy modules and callers remain. Independent server checks and ordinary
  workspace checks are recorded in the delivery. Source driver trees are not yet
  present; this does not prove final ownership or package readiness.
- **B, partial:** real local synthetic TLS Prepare/Prepared/bound readiness passes.
  Query/schema/tables/log end-to-end execution is not implemented. Client phases
  remain outside this slice.
- **C, not done:** native decimal/time/bytes/exact JSON conversion and source
  success matrices are absent. Existing protocol fixtures are not adapter proof.
- **D, not done:** server compiler, omitted-column expansion, independent table/
  SELECT/WHERE/ORDER grants, native per-IN normalization and read-only-role proof
  are absent. Readiness permission never implies those operations.
- **H, partial:** exact readiness grants, expiry, revocation, malformed authority,
  changed Call binding, a Call for an operation the pin never authorized, a
  resource reusing a principal identity, fixed
  principal-limit rejection, global-limit saturation and release, handshake and
  frame deadlines closing without a Failure frame, an oversized Prepare frame,
  accept-error classification, private-file rejection (group-readable,
  symlinked, FIFO, shared parent) and durable restart/remap/retirement/history
  failure have synthetic tests. The Call-operation refusal is proved by a unit
  case over the pin comparison, not by the end-to-end fixture: while readiness
  is the only reachable pin, a mismatched Call is refused one frame later by
  the protocol layer's `ResultBody::validate_for` with the same code, so the
  synthetic exchange stays green with the service-layer comparison removed.
  That comparison is defense in depth behind `validate_for`, and the unit case
  separates them by pinning an operation whose reply is unimplemented. The certificate test proves client-side
  server-name validation only; there are no client certificates in this slice,
  so no server-side identity assertion is claimed. Per-driver measured peak
  memory, huge cells/schema/buffer acquisition, native cancellation and pool
  cleanup remain blocked/unproved. Adversarial TLS proof remains incomplete, as
  does a foreign-owner file fixture, which needs a second uid.

## Durable identity history

The configuration has a required `history` path. Before initial startup, the
operator creates a private `0600` file containing exactly
`{"version":1,"entries":[]}` in a private `0700` directory. This is explicit
first enrollment; the server never creates or resets a missing/corrupt registry.
Keep this file and its adjacent `.lock` path at the same trusted location across
restarts. Do not delete, edit, reset or restore an older history, including during
binary rollback. Copy the complete current registry when deliberately relocating
it. The trusted operator/filesystem can defeat history by replacing it, just as
it can replace server credentials; no disk-rollback attestation is claimed. Like
the authority file, a deliberate relocation or replacement must land by atomic
rename; the server's own writes already use a `0600` temporary plus file and
directory fsync.

`serve` holds an exclusive nonwaiting file lock for its lifetime, validates the
entire proposed principal/resource set against history, and atomically replaces
changed history using a `0600` temporary file, file fsync and directory fsync
before accepting connections. `check` validates the proposed transition without
writing or enrolling anything. Missing, malformed, oversized or locked history
does not silently reset.

**The operative registry limit is 64 KiB of serialized JSON.** A 1,024-entry
count cap also exists but is never the limit that binds: an entry is 216 bytes
at the current shape, so the byte ceiling is reached at about 300 entries -
roughly two full rotations of the 128-principal maximum. Exhaustion fails closed
with `cap_exceeded` and writes nothing; the file on disk is unchanged after a
refused enrollment, and no historical record is ever pruned to make room. An
operator approaching that ceiling must plan a reviewed registry migration, not a
reset.

Each entry retains the opaque identity, its never-reused generation, a SHA-256
fingerprint of the complete current serialized object, and active/retired state.
Unchanged definitions keep their exact binding across restart. Changed definitions
require a fresh generation. Prior generations cannot return, even with the same
old definition; IDs removed at a restart stay retired forever. Principal and
resource IDs share a nonoverlapping namespace. Generation reuse for a different
identity also denies. Operator-selected new identity/generation values must be
fresh opaque 128-bit values, not labels or counters.

All running-server identity definitions are frozen. Every Prepare/invoke/release
reload compares the current complete definition set with the enrolled set, then
checks the exact current grant. Identity edits require a deliberate restart;
grants/expiry/revocations stay live. The connection continues to own its originally
resolved principal/resource objects and cannot replace them by alias.

Current resource definitions contain alias, identity, generation and class only.
They remain readiness placeholders. Before source execution, adapters must extend
these objects with the actual resolved source definition/credentials and keep
that exact object pinned. Hashing a credential reference without resolving it is
not enough. This source-resolution gate is still incomplete; no actual source
Call is accepted by this slice.

## Review cleanup fields

- Verdict: implement.
- Opportunity: one connection-owned `Pinned` object that also owns the
  authorized operation, one shared private-file reader for every trusted
  operator file, a classified accept outcome, and bounded durable identity
  history.
- Why: authority checks compare the originally resolved objects and cannot
  silently replace them by alias; the pinned operation removes three duplicated
  `Operation::Readiness` literals; one reader removes two divergent copies of
  the file checks, so the authority file can no longer be admitted under weaker
  rules than the history; the accept classification removes the "any error ends
  the process" branch. Permit drop removes active counts on exit.
- Scope: extracted server auth/config/history/service modules and their tests
  only. Driver containment and resolved source objects remain incomplete phase
  work. The accept loop still delegates the connection exchange to keep
  lifecycle ownership clear.
- Validation: server unit and integration tests (authority, CLI surface,
  private files, durable history, real synthetic TLS exchanges), workspace
  clippy with `-D warnings`, `cargo fmt --check`, `cargo test --all-targets`
  and `git diff --check`; outcomes are reported in the PR.

## Verification commands

Run from the workspace with `CARGO_BUILD_JOBS=2`:

```sh
cargo test -p gaze-lens-server
cargo clippy --all-targets --no-deps -- -D warnings
cargo test --all-targets
cargo fmt --check
cargo tree -p gaze-lens-server --all-features --edges normal
cargo tree -p gaze-lens-protocol --all-features --edges normal
```

The 33 server cases cover authority, exact CLI command surface, private-file
admission, durable history, accept-error classification, deadline closure and
real synthetic TLS exchanges. The history fixture asserts that `check` does not
enroll or modify history, comparing the file byte for byte before and after. The ordinary suite retains the standalone
allocation-admission probe under Cargo's existing `harness = false` linkage.
Normal pre-push hooks remain enabled. Logs and exact final outcomes belong to
the PR/delivery, not a claim that unimplemented driver gates have passed.

No legacy source/test or feature-gated adapter changed. The previously reported
optional PostgreSQL `Profile.production` fixture repair was not required by this
slice and is not included. PostgreSQL/MySQL integration-feature execution, native
acquisition measurements and Linux worker limits remain unrun. No timeout value
was changed, and exit 124 is not accepted as success.
