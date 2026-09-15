# Split development assembly

These are extraction boundaries, not a complete split product. The server now
has a limited authenticated readiness development slice, documented in
[gaze-lens-server/docs/phase2-proof.md](gaze-lens-server/docs/phase2-proof.md).
It does not execute a source, restore a token or provide client release.
The normative contract is [server-client-split.md](../docs/reference/server-client-split.md).

- `gaze-lens-protocol`: pure closed DTOs, exact JSON/number carriers, shape/bounds
  checks and one-connection sequence checking. Dependencies: serde, serde_json
  raw values and base64 only. It intentionally has no feature flags.
- `gaze-lens-server`: nonpublished phase 2 auth/TLS/readiness slice with separate
  serve/check commands and durable identity history. Acquisition remains incomplete;
  no source execution, collectors or client dependency.
- `gaze-lens`: empty nonpublished client library scaffold for phase 3. Its
  temporary Cargo name is `gaze-lens-client-development`, because the unchanged
  legacy package still owns `gaze-lens`. Rename at the phase 4 cutover. There is
  no dependency on the server or legacy assembly, including optional features.

The root package is explicitly `publish = false` and retains the existing binary,
public DTOs, compiler, Gaze methods and tests during phases 1–3. Ordinary root
Cargo commands include all four development workspace members. Keeping the root
manifest avoids moving the legacy tests, examples and callers merely to rename
this temporary assembly. The final three-crate virtual workspace, two independent
binaries and release packaging require phase 4/6 cutover and proof. Do not release
this intermediate tree, even if the existing distribution tooling can build it.

Legacy query DTOs now live in `src/source/db/query_dto.rs`; `query.rs` reexports
them for its unchanged callers and retains the server-bound compiler. New wire
DTOs live only in the protocol crate. They intentionally do not reuse the legacy
schema serializers or the f64-based legacy scalar enum. Legacy `src/value.rs`
Gaze methods remain live and untouched; the independent wire conversion is
`gaze-lens-protocol/src/value.rs`. Removing those legacy modules waits for cutover.

## What phase 1 proves

`cargo test -p gaze-lens-protocol` tests wire shapes, exact typed values, operation
pairings, binding correlation, strict framing, fixed window DTOs and package page
invariants. `cargo tree -p <extracted-package> --all-features` proves the dependency
partitions of the extracted packages only. The empty client/server libraries
provide no execution proof. Normal workspace checks keep the existing binary green.

Framing callers must cap reads *before* buffering using `FrameBuffer` (or an
independently proven equivalent), use 4 KiB for Prepare/Prepared, apply deadlines,
and close on every error. `Sequence` is a grammar checker, not authenticated or
authorized state. An authenticated caller must compare the frozen destination
before restoration, pin the resolved resource and recheck grants/generations
before invoke and release. DTOs have no Debug implementation and are raw data.

ExactJson uses validated raw JSON bytes rather than serde_json::Value. This keeps
large integer/fraction/exponent lexemes and even dynamic keys resembling serde's
private number marker. It cannot recover digits already lost by a source adapter.
Native source conversion and per-driver pre-materialization memory/cancellation
proof remain phase 2 requirements. Pure framing bounds do not establish them.

## Frozen binding handoff to phase 2

The protocol keeps restored strings unchanged. The pinned server schema chooses
native types and the server validates **every operand before executing**:

- Integers: integral exact JSON numbers or strict base-10 integer strings; native
  width/sign checked, then native integer binds. No PostgreSQL TEXT fallback.
- Decimals: exact numbers or plain decimal strings; precision/scale checked
  without rounding, redundant fractional zeros may be removed losslessly.
- Floats: finite exact JSON number spelling or string; native-width round-trip
  precision, overflow and underflow checked before binding.
- Text: string only, unchanged (`00123`, `1e3`); bool: boolean only. UUID, bytes,
  native temporal and JSON strings validate the native semantic contract before
  native binding. JSON strings do not authorize object-valued query predicates.
- `eq/ne/in`: supported exact native binds; `in` is a nonempty scalar list.
  Ordering requires numeric/text/supported temporal types; `like` is text-only.
  Null tests omit operands; all other null operands reject.
- SQLite: supported declared type/affinity plus guarded storage-class comparisons;
  ambiguity denies, and JSON-in-TEXT requires the server allowlist.

Required driver success fixtures include same-row PostgreSQL BIGINT/NUMERIC
restored-string `eq` and each `in` element. No wire test claims this execution
proof. Native decimal `0.001`, `1.2300`, negative scale, real date/time/naive/zoned
semantics, canonical bytes and exact source JSON must succeed through adapters.

The log keyword request remains `{}` with a fixed maximum 1,000-line window.
Acquisition order, newest contiguous suffix, boundary omissions, smaller operator
ceilings and byte-pressure selection require source/client implementation later.
Package page checks prove bounded structural relationships, not inventory
completeness: every page must reauthorize and rescan within separate acquisition
and working-memory caps. No snapshot, cursor, cache or collector is implemented.

Truncation validation checks the closed reason set/order and observable count
relationships. It cannot attest that source data remains or that an operator's
lower ceiling caused a cut. A `rows`/`lines`/`records` cut may occur below the
requested limit when the authorized server ceiling is lower; the source owner
must prove exact selection/reason behavior in phase 2/5 fixtures.
