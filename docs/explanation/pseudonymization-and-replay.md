# How pseudonymization, audit, and replay fit together

This page explains the idea at the heart of gaze-lens: **pseudonymize on the way out,
reverse locally later.** An agent sees `<EMAIL_001>` instead of `alice@example.invalid`;
the engineer who ran the session can later replay it on their own laptop to recover the
original values. Everything else — the chokepoint, the manifest, the snapshot files, the
restore telemetry — exists to make that one trick both *safe* (raw data never crosses to
the model) and *reversible* (the operator can always get the truth back).

It is worth understanding *why the pieces are ordered the way they are*, because the
safety of the whole scheme rests on the ordering, not on any single component.

## The chokepoint and the redact → manifest → return ordering

Every retrieval — an MCP `query`, a `log_tail`, even the human CLI `query` — flows through
one function, `Session::dispatch_tool`, which builds a `gaze_mcp_core::PiiEnvelope` and
dispatches through it. There is deliberately no second path. The envelope enforces a strict
order:

1. **`begin_call`** records that a call is starting (fail-closed: if this write fails, the
   call does not proceed).
2. The tool runs and the adapter hands back **raw** values *inside* the envelope — never
   to the agent.
3. **`Pipeline::redact`** turns those raw values into clean, tokenized output.
4. **`finish_call`** durably stores the tokenized args, status, a result summary, and a
   snapshot reference — *before* the redacted result is returned to the caller.
5. If begin or finish fails at any point, **no raw output is returned.**

Two properties make this safe, and both come from the ordering rather than from vigilance:

- **Redact-before-anything-persists.** Tool arguments — the `where` AST, grep patterns,
  table and column names — are tokenized through the *same* `redact` path as result data
  *before* the manifest is written. The manifest therefore never stores raw arguments. The
  raw form is reconstructable only later, on operator replay.
- **Manifest-first means fail-closed audit.** Because `begin_call` precedes the tool and
  `finish_call` precedes the return, there is no way to produce an agent-visible result
  without a durable audit row. Audit is not a side effect you might forget to write; it is
  on the critical path to producing output at all.

The crucial detail is that this is not enforced by code review or convention. The envelope
hands each tool a **sealed `ToolCtx`**, and the type system makes "raw output escapes
without going through redaction" a *compile error* rather than a runtime check. You cannot
write the bypass; it does not type. (The implementer-facing description of the dispatch
flow lives in [reference/architecture.md](../reference/architecture.md).)

## Tokenization is deterministic on purpose

When Gaze redacts a value it does not produce a random placeholder — the *same* input maps
to the *same* token within a session. `alice@example.invalid` becomes `<EMAIL_001>` and
stays `<EMAIL_001>` every time it appears. This determinism is a feature, not an
implementation accident, and it is what makes the agent's view coherent: an agent
correlating "the user with email `<EMAIL_001>` also placed order `<ORDER_007>`" is doing
real reasoning over stable handles, even though it never sees a raw value.

gaze-lens runs a single shared Gaze session per `serve` process (one `lens_session_id`)
with a per-profile pipeline. The session scope is `Scope::Conversation`; gaze-lens refuses
`Scope::Ephemeral` at construction because an ephemeral session cannot be exported, and
without export there is nothing to replay.

A direct consequence is **cross-profile token correlation**: if an entity appears in
profile A and profile B under one `serve` process, it tokenizes to the same value in both.
That is intentional — it is the same correlation you would get from two CLI `query` calls
in one conversation, and it is what keeps replay coherent across a multi-profile
investigation. Operators who need *disjoint* token spaces run separate `serve` processes
per profile group until v2 introduces per-profile scoping. Whether that correlation is a
risk you care about is a threat-model question, discussed from the boundary side in
[the threat-model explanation](./threat-model.md).

## Two data planes: the index and the vault

gaze-lens keeps the audit trail in two deliberately separate places, and the separation is
the reason the scheme is both auditable and reversible without the audit log itself
becoming a PII liability.

- **The manifest** (`~/.gaze-lens/manifest.sqlite`) is the *index*. It holds tokenized call
  metadata — what was accessed, when, with what status, and a reference to the snapshot. It
  stores no raw values. Because it is just tokenized metadata, it can be opened "at rest"
  with no Gaze pipeline and no source connections (this is how snapshot-retention sweeping
  works without spinning up a session). This manifest is gaze-lens's own data plane; it
  *coexists with* Gaze's separate metadata-only redaction log but is not the same thing.
- **The snapshot** is the *vault*. Gaze's `SensitiveSnapshot` holds the raw token-to-PII
  mappings needed to turn `<EMAIL_001>` back into the original value. Snapshots are stored
  **out-of-row** as `0600` files in a `0700` directory, *referenced* from the manifest
  rather than inlined into it (decision D9).

Splitting the index from the vault is what lets the audit log be freely inspectable while
the sensitive material stays in tightly-permissioned files that the operator's disk
encryption protects. It is also why the threat model's most load-bearing assumption is
disk encryption (FileVault / LUKS): the snapshot files *are* the recoverable PII.

## Replay: whole-session, strict, and self-reporting

`gaze-lens replay <session_ulid>` walks the manifest's call history for a session, imports
each referenced snapshot via `gaze::Session::import`, and prints the restored call history.
Three design choices are worth understanding:

- **Whole-session only at v1.** You replay an entire session, not one call. This is not a
  missing feature so much as a reflection of the current Gaze restore API, which restores a
  *session snapshot*, not a single call's token map. Per-call selectors are rejected
  explicitly (`not in v1; tracked as v1.x candidate`) rather than silently approximated.
- **Strict restore — never guess.** Known tokens are restored *byte-exactly*. A
  token-shaped string that is *not* in the snapshot is left in place and reported with a
  failed-restore decision — gaze-lens never falls back to a blank or a fabricated value.
  The principle mirrors the rest of the system: surfacing "we could not restore this" is
  always safer than guessing.
- **Restore telemetry.** Each call carries restore telemetry, and the session carries a
  summary with success / partial / failed counts plus unknown-token, manifest-bypass, and
  fresh-PII counts. This turns replay from "did it work?" into a measurable statement about
  exactly how much was restored and what wasn't. (For the actual command, flags, and
  operator snapshot controls, see the
  [replay-a-session how-to](../how-to/replay-a-session.md).)

## Observability: explaining *that* a token is safe, without exposing what it hides

There is a deeper question lurking under all of this: *how do you trust a tokenized result
you can't see the inside of?* If the agent only ever sees `<EMAIL_001>`, how does an
operator know the redaction was correct — that the pipeline did not, say, leave a
half-redacted address in a nested JSON field, or pass through something it should have
caught?

The v0.9 observability work answers this by letting gaze-lens explain *that* a result is
safe to return — where the pipeline had to choose between competing interpretations, and
which safety nets fired — **without ever exposing the raw PII it is reasoning about.** It is
deterministic instrumentation of the pseudonymization itself, not a model-behavior policy,
and it rides entirely on the existing surfaces (it adds no MCP tool and no CLI subcommand;
the metadata is produced *after* `Pipeline::redact` and attaches to existing responses,
manifest rows, and diagnostics).

The trick is that every observability signal is *token-safe metadata* — counts, family
labels, reason codes — and never a raw span, a rejected candidate, or enough offset detail
to reconstruct a value. Four signal families illustrate the idea:

- **Ambiguity counts.** When recognizers disagree or a candidate sits near a policy
  threshold, the pipeline can record something like `ambiguous.email_or_username = 3` — the
  *count* and the *kind families*, never the raw spans. This is a tuning side-channel, not
  a raw-evidence channel.
- **Validator-veto.** When a validator rejects a candidate (a checksum failure, an
  impossible date, an unsupported number plan), it records the recognizer family, validator
  class, and a veto count. The rejected raw value stays out of the record. This is for
  operator trust and regression triage — it is explicitly *not* an access-control decision
  and must never cause raw output to bypass the envelope.
- **Collision-family and anchor resolution.** When two raw values map into a shared token
  family, observability records the family name, the anchor strategy, and whether an
  existing anchor was reused or a new one allocated — expressed in token ids and family
  counters only. This is what lets replay *explain* why `<EMAIL_001>` stayed stable across
  profiles without ever revealing the value underneath it.
- **Locale-aware safety-net dispatch.** When locale hints are weak or absent, the pipeline
  can dispatch fallback recognizers and record that a locale safety net fired — the locale
  policy and recognizer family, not the matched text. The default is conservative: an
  unknown locale *increases* safety-net coverage, it never loosens redaction.

The throughline connecting observability back to the rest of this page is the same property
the chokepoint guarantees: you can learn a great deal about *how* a value was handled while
the value itself stays on the safe side of the boundary. That is the whole point of
pseudonymization with reversible, auditable mapping — the agent gets a coherent,
PII-free view; the operator gets a measurable, replayable account of exactly what happened.

## Where to go next

- **Why the trust boundary is drawn where it is:** [the threat-model explanation](./threat-model.md).
- **Actually running a replay (command, flags, snapshot handling):** [replay-a-session how-to](../how-to/replay-a-session.md).
- **The implementer spine — dispatch flow, manifest versioning, the observability spine:** [reference/architecture.md](../reference/architecture.md).
- **The locked audit + restore and observability contract:** [reference/spec.md](../reference/spec.md).
