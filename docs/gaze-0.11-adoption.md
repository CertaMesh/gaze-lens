# Adopting gaze 0.11.x in gaze-lens

> **Status:** proposal / decision document. Not yet committed to a release.
> **Engine:** built on the Gaze pseudonymization runtime.
> **Scope:** adoption assessment for the gaze stack `0.9.0-rc.1` → `0.11.x`, plus the
> 0.11-era capabilities (token-bridge index-search, gaze-mcp-bridge discovery
> patterns, NER fail-closed, restore consolidation, #988 posture, #1490/#1492
> discovery & filtering).
> **Source:** distilled from a verified master-orchestrator research fan-out
> (6 agents, checked against gaze-lens source). This document is the maintainer-facing
> writeup; it makes no code changes and bumps no dependencies.

This note plans how gaze-lens adopts the gaze `0.11.x` runtime and the features that
ride on it. It is a planning artifact only: it does not edit `Cargo.toml`, source, or
tests, and does not run `cargo`. Each section states a recommendation, a rough effort,
and whether it touches the locked product SPEC.

## Update — decisions after this writeup

This document captured the research snapshot. Two things changed after it was drafted:

1. **T0 is merged.** The gaze stack is now pinned at `0.11` (PR #73); the bump was
   version-string-only with no source edits and no golden-snapshot drift. References below
   to "current pin is `0.9.0-rc.1`" are historical.
2. **T6 reframed — token-bridge dropped.** A follow-up spike (read of `gaze-token-bridge`
   `v0.11.1` + a throwaway prototype) found the bridge does **exact entity-bound HMAC
   lookup, not free-text/keyword grep**, and its `RedactionSession` is sealed to
   `Scope::Ephemeral` (non-exportable) so results are **not restorable by `gaze-lens
   replay`** ("R3" is upstream-sealed). The maintainer confirmed the actual need is
   **free-text/keyword** search. Conclusion: do **not** adopt token-bridge; deliver the
   need as **indexed `log_grep`** instead (merges into §3/T5 and #1492):
   - **T5a** — internal redacted-tail *cache* behind `log_grep`. Pure performance (removes
     repeated SSH round-trip). **No SPEC change.**
   - **T5b** — opt-in `mode: "keyword"` (+ optional `refresh`) on `log_grep`, backed by an
     inverted index over **redacted** text (index built post-`Pipeline::redact`, stored
     `0600`/`0700` under `~/.gaze-lens/`). **SPEC amendment** (changes `log_grep` semantics)
     but stays inside the locked 5-tool surface — no new tool or subcommand.

   §5 below is retained as the evaluation record for *why* token-bridge was rejected.

## Surface invariant (read first)

The gaze-lens v1 public surface is **locked**:

- **MCP tools (5):** `query`, `schema`, `list_tables`, `log_tail`, `log_grep`.
- **CLI subcommands (6):** `serve`, `init`, `query`, `replay`, `check`, `demo`.

Adding any MCP tool or CLI subcommand is a **SPEC-level change** — it requires a
[SPEC.md](../SPEC.md) amendment PR, not an implementation PR. Several capabilities below
are attractive but cross that line; they are explicitly flagged **SPEC-amendment** and
gated on a maintainer decision. Internal helpers and richer metadata on *existing*
responses are not SPEC changes; new agent-visible verbs are.

## 1. Overview & recommendation

The single prerequisite that gates almost everything is the version bump itself:

> **Bump the gaze stack `0.9.0-rc.1` → `0.11.x`.**

Every new capability in this document — token-bridge index-search, restore telemetry,
NER fail-closed, recognizer precision fixes — requires it. The bump is **near-mechanical**.
The public chokepoint surface gaze-lens depends on in `gaze-mcp-core`
(`PiiEnvelope::new` 7-arg, `dispatch` 4-arg, `Tool` / `ToolCtx` / `ToolResources` /
`ToolDescriptor` / `ManifestStore` / `DispatchError` / `ToolError`) is **byte-for-byte
identical** across the two tags. The only true breaking change in the stack is
`gaze-types` `Recognizer::detect` becoming fallible
(`-> Result<Vec<Candidate>, DetectError>`, v0.10.0, #908) — and gaze-lens implements
**no** custom `Recognizer` / `Detector`, so that change does not touch us.

Ranked adoptions:

1. **The version bump itself (T0).** Unlocks fail-closed NER and recognizer precision
   fixes (#923 token-boundary, #952 camelCase argv) for free, with zero source changes.
   Best ROI; ship first as its own track.
2. **Whole-text strict restore** (`Session::restore_strict_text` /
   `restore_with_policy_telemetry`). Replaces three hand-rolled per-token restore loops
   with one atomic, byte-exact call. Small, no SPEC change, improves `replay`.
3. **gaze-token-bridge indexed search of server log files.** Genuine fit, but the agent
   surface is a SPEC-level change. Decision-gated, medium effort.
4. **#988 (nested-JSON name leak) posture.** The bump does not auto-fix it, but NER is
   now fail-closed, which makes it *safe to mandate* a NER model in a production profile.
   Doc-note / behavior change, not a SPEC change.
5. **Restore-boundary DLP audit** (`enable_restore_boundary_dlp_audit`). Opt-in audit of
   manifest-bypass / fresh-PII at replay. Medium, doc-note, later.

**Skip:** `gaze-mcp-bridge` — it is a write/proxy/restore-on-egress axis, and gaze-lens
is read-only. Mine its discovery and policy patterns (§4) without adopting the crate.

## 2. T0 — the prerequisite version bump (foundational)

**Recommendation: adopt now. Effort: small. SPEC risk: none.**

T0 blocks every other track. It is a version-string change for compilation purposes; the
research verified **no required source edits** to compile against `0.11.x`.

**Required changes (T0):**

- Bump the three pins in `Cargo.toml:46-48` — `gaze` (the `gaze-pii` package alias),
  `gaze-recognizers`, `gaze-mcp-core` — to `"0.11"`. Caret semantics cover the published
  point releases (token-bridge publishes `0.11.1`; the core trio publishes `0.11.0` /
  `0.11.1`).
- `cargo update -p ...` the gaze crates; regenerate `Cargo.lock`.
- Run the local pre-push gate (`fmt --check` + `clippy -D warnings` + `test --all-targets`).
- Refresh golden / snapshot fixtures that drift from the precision fixes (see below).

**Verified non-breaking** against gaze-lens source (do not re-litigate these unless rustc
says otherwise after `cargo update`):

- `PiiClass` exhaustive match at `src/session/mod.rs:1103-1110` — no variant added.
- `DispatchError` match at `src/session/mod.rs:1113-1131` — no wildcard arm; this is safe
  **only because** the dispatch surface is byte-identical across tags. If rustc flags the
  match as non-exhaustive after the update, **investigate the new variant — do not paper
  over it with a blind wildcard.**
- `ToolError` / `ManifestError::Backend` unchanged.
- Pipeline construction is shape-identical at `src/policy.rs:170`, `src/demo.rs:119`,
  `src/session/mod.rs:1184-1190`.
- `RegexDetector` / `NerDetector::load_with_options` / `NerOptions` /
  `DEFAULT_NER_THRESHOLD` / `PolicyError::NerLoad` preserved.
- `Session::import` / `restore_strict` identical.

**Validate at build:**

- Golden / snapshot tests **will** drift from #923 (token-boundary) and the byte-exact
  adjacent-restore fix. That drift is *correct* — refresh the fixtures, don't suppress it.
- Confirm `Error::RecognizerDetect` propagates cleanly through `Session::dispatch_tool` /
  `PiiEnvelope`. Do not catch-and-continue (see §5).

**Acceptance:** suite green after fixture refresh, and `Error::RecognizerDetect` surfaces
rather than being swallowed.

## 3. Per-capability adoption

### 3.1 Restore-path consolidation (`restore_strict_text` / restore telemetry)

**Recommendation: adopt now (with T0). Effort: small. SPEC risk: none.**

gaze-lens currently hand-rolls three per-token restore loops:

- `src/mcp/tools/mod.rs:77-82`
- `src/session/mod.rs:968-973`
- `src/session/restore.rs:149`

gaze `0.11.x` offers `Session::restore_strict_text` (and a `_with_events` variant) for one
atomic, byte-exact restore call, plus `Pipeline::restore_with_policy_telemetry`, which
returns `(RestoredText, RestoreTelemetry)`. Consolidating onto these:

- Replaces three loops with one call, removing a class of per-token reassembly bugs.
- Gives `replay` structured success / partial / failed status plus restored-token counts,
  via `RestoreTelemetry`.
- Stays inside the locked surface — `replay` is an existing CLI subcommand; this enriches
  its output, it does not add a verb.

Follow-up doc update: `docs/replay.md` to describe the new telemetry in `replay` output.

### 3.2 NER fail-closed (#988 production posture)

**Recommendation: adopt the fail-closed behavior now (free with T0); mandate a model in a
production profile as a doc-note. Effort: trivial (behavior) / small (profile mandate).
SPEC risk: none (doc-note / behavior change).**

The bump does **not** auto-fix #988 (names leaking through arbitrary nested-JSON fields),
but it changes the *recommended* fix and makes the right fix safe to mandate.

- `gaze-assembly`'s `core` rulepack adds **cue-anchored** name detection
  (`AnchoredMatchRecognizer`: forward markers, recipient cues, footers). It catches names
  near deterministic structure, **not** arbitrary nested-JSON name fields — so it does not
  close #988 on its own. No default NER model is bundled; `register_ner` only fires when
  `policy.ner.model_dir` is set (expected family: Davlan mBERT).
- **The key shift:** at `0.9.0-rc.1`, `NerDetector::detect` swallowed backend errors into
  `Vec::new()` — a *silent leak*. At `0.11.x`, the `.detector()` adapter calls
  `try_detect`, so a `RecognizerRuntimeError` makes the pipeline **abort** with
  `Error::RecognizerDetect`. Long inputs are chunked into bounded 512-token windows. Our
  NER path (`src/policy.rs` `build_pipeline`, `NerDetector::load_with_options`) becomes
  fail-closed for free.

**Net recommendation:**

1. **Require `policy.ner.model_dir` in a "production" profile.** This is now safe to
   mandate: a model load/backend failure aborts redaction instead of leaking. This is the
   honest fix for general nested-JSON names.
2. *Optionally* column-rule known JSON name fields (deterministic, no model) as
   defense-in-depth.
3. *Optionally* adopt the `gaze-assembly` `core` rulepack (complementary, not a fix; note
   the `phone-parser` feature gate for `e164_phone`).
4. This is a doc-note / behavior change, **not** a SPEC amendment.

**Open decision (T2):** is "production" a distinct profile tier, or a per-profile flag
that requires `ner.model_dir`? See the track table.

> **Honest-uncertainty note:** the production-NER mandate depends on the operator
> supplying a model directory; gaze-lens cannot guarantee the model's recall on arbitrary
> nested-JSON shapes. The win is *fail-closed* behavior (no silent leak on backend error),
> not perfect name coverage.

### 3.3 Recognizer precision fixes (#923, #952) — free with the bump

**Recommendation: adopt now (free with T0). Effort: trivial. SPEC risk: none.**

- **#952 camelCase argv over-redaction fix** → cleaner `log_tail` / `log_grep` output
  (fewer spurious redactions of camelCase argv tokens).
- **#923 token-boundary + byte-exact adjacent restore** → fewer false-positive redactions
  and exact path / ID replay.

Both arrive with the version bump and require no source changes — only a golden-fixture
refresh, since correct output now differs from the old fixtures.

### 3.4 Restore-boundary DLP audit (`enable_restore_boundary_dlp_audit`)

**Recommendation: later. Effort: medium. SPEC risk: doc-note.**

`PipelineBuilder::enable_restore_boundary_dlp_audit` makes manifest-bypass and fresh-PII
events at replay auditable, routed into the SQLite manifest plane. Useful hardening, but
not on the critical path; sequence it after T0 + restore consolidation (T1). Doc-note only;
no SPEC change.

## 4. gaze-mcp-bridge — skip the crate, mine the patterns (#1490 / #1492)

**Recommendation: do not adopt the crate. Borrow its discovery and filtering patterns
inside the locked surface.**

`gaze-mcp-bridge` puts Gaze between an agent and downstream MCP servers and *restores*
tokens into side-effecting tool args on egress (`BridgeHost: DispatchHost`, not
`PiiEnvelope`). That is a write / proxy axis, orthogonal to read-only gaze-lens, and it
pulls heavy dependencies (`rmcp` client + `transport-child-process`, `chacha20poly1305`).
We do not adopt it. But two open issues map onto patterns it demonstrates:

**#1490 — capability discovery (SPEC-safe path).** The bridge pattern: enumerate
capabilities, tag each with metadata (name, JSON schema, description, and a stable
`discovery_hash` = SHA-256 over server + tool + schema), and an allow/deny disposition —
with **discovery decoupled from grant**. gaze-lens can deliver this **without a 6th tool**:

1. Enrich the `schema` / `list_tables` responses with optional observability metadata
   (the SPEC already reserves "optional observability metadata on existing responses"):
   profile name, source-class (db vs log), allowed columns, and a stable profile / schema
   hash; **or**
2. Add CLI-only discovery via flags on the existing `check` / `serve` subcommands (flags
   are not new subcommands, so this stays inside the surface).

**#1492 — richer log filtering (reference patterns).** Borrow:

- Layered deny-by-default policy resolution (`default → server → tool → arg`, monotonic
  guards).
- Resource / limit guards for bounded log output (`response_bytes` 1 MiB,
  `content_blocks` 64, `call_timeout` 30s).
- Path-only audit (`arg_paths_affected`, `raw_sha256` only) — audit filter decisions
  without logging the matched raw log content.

**Aside (snapshot encryption-at-rest):** the bridge encrypts session snapshots at rest
(`ChaCha20Poly1305`, `BridgeSessionStore`) over the same `Session::export` / `import` +
`SensitiveSnapshot` API gaze-lens already uses. Per-snapshot encryption-at-rest is a v1
non-goal (the threat model assumes FileVault / LUKS), but this is the reference if we ever
revisit it.

## 5. gaze-token-bridge — indexed search over server files (decision-gated)

**Recommendation: decision-gated. Effort: medium. SPEC risk: SPEC-amendment (the agent
surface).** Prototype before committing.

`gaze-token-bridge = "0.11.1"` (single published version, **pre-1.0**) provides owner-side
gated index-search over **redacted** corpora — the genuine fit for "index server log
files."

- **Redact-before-index:** `ingest::CorpusIngestor::ingest_text(doc_id, raw_text)` runs
  each document through `gaze::Pipeline` and rewrites clean text into stable HMAC-keyed
  domain aliases. The index key is `(domain_id, fingerprint_hex)`; raw text is never a key.
- **Owner-side gated search:**
  `bridge::TokenBridge::search(&session, &request) -> BridgeSearchOutcome::{Allowed|Denied}`
  resolves token → HMAC-project → default-deny `RegistryPolicyGate` → single-use,
  entity-bound, expiring `SearchHandle` → `SearchAdapter` (by fingerprint) →
  `SessionResponseTranslator` → mandatory output safety-net.
- **Output backstop is mandatory:** `.with_output_safety_net(pipeline, locale_chain)` — if
  absent, search fails closed (`DenyReason::TranslatorFailed`). It re-scans translated
  snippets via `Pipeline::scan_safety_nets`; `nets_run == 0` → deny. `IndexEntity` /
  `IndexSearchHit` are deliberately non-`Serialize`.
- **Persistence:** `FileCorpusIndexStore::load_or_create(dir, domain_id, classes)` writes
  `.gaze-index/index.json` (schema v1) under a `0700` dir with `0600` files and atomic
  save — the same on-disk posture as `~/.gaze-lens/snapshots/`. Entry point:
  `TokenBridge::from_policy_json_and_store(policy_json, store)`.

**Exposure options — both are SPEC amendments for the agent surface:**

- **Option A (not recommended literally).** The `chokepoint` cargo feature compiles
  `SearchDocumentsTool` (a `Tool` named `"search_documents"`) — a literal **6th MCP tool**.
  Worse, it owns its own per-principal `RedactionSession` registry, separate from our
  `gaze::Session` manifest. The crate defers manifest unification as **"R3"**, so search
  tokens would land in a *different namespace* and break unified replay.
- **Option B (recommended).** Wire the bridge *runtime* (default feature, no `chokepoint`)
  through our own `Session` / `Pipeline`, and surface it as a new MCP tool
  (`log_search` / `index_search`) **or** a CLI-only subcommand. This keeps tokens in our
  namespace. It is still a SPEC amendment (new agent-visible verb either way). Reference:
  the `gaze index` CLI (`gaze --features index`).

**Operational dependency:** the output backstop hard-requires the **Kiji DistilBERT**
safety-net model (`GAZE_KIJI_DISTILBERT_MODEL_DIR` / `..._COMMAND`). No model → search
fails closed.

**Risk (flag honestly):**

- **Pre-1.0.** The crate has a single published version (`0.11.1`); API churn is likely.
- **R3 namespace risk (load-bearing).** Until the crate unifies its manifest with ours,
  Option A's tokens live in a separate namespace and break unified replay — the central
  reason to prefer Option B.
- **Kiji model dependency.** The mandatory safety-net adds an operational model dependency
  the rest of gaze-lens does not currently require.

**Action:** prototype Option B on a throwaway log corpus *before* opening a SPEC PR.

## 6. Items explicitly skipped

| Skipped | Why |
|---|---|
| `gaze-mcp-bridge` (crate adoption) | Write / proxy / restore-on-egress axis; gaze-lens is read-only. Mine patterns only (§4). |
| Operator-tier MCP tools (`RestoreTool` / `ExportSessionTokensTool`) | Inject extra MCP tools into the agent surface — SPEC-amendment with no read-investigation payoff. `replay` already covers local raw-value verification. |
| token-bridge **Option A** (`chokepoint` `SearchDocumentsTool`) | Literal 6th tool **and** separate token namespace (R3) → breaks unified replay. Prefer Option B if we pursue §5 at all. |

## 7. Feature → effort → SPEC-risk summary

| Feature | Maps to | Effort | SPEC risk | Recommend |
|---|---|---|---|---|
| Version bump `0.9.0-rc.1` → `0.11.x` | `Cargo.toml:46-48` | small | none | **adopt now (T0)** |
| `Session::restore_strict_text` / `_with_events` | restore loops `mcp/tools/mod.rs:82`, `session/mod.rs:973`, `session/restore.rs:149` | small | none | **adopt now (T1)** |
| `Pipeline::restore_with_policy_telemetry` | structured `replay` status + counts | small | none | **adopt now (T1)** |
| NER fail-closed (`try_detect` → `Error::RecognizerDetect`) | `policy.rs`; #988 prod posture | trivial | none | **adopt now (free, T0)** |
| #952 camelCase argv over-redaction fix | cleaner `log_tail` / `log_grep` | trivial | none | **adopt now (free, T0)** |
| #923 token-boundary + byte-exact restore | fewer false positives; exact path/ID replay | trivial | none | **adopt now (free, T0)** — refresh goldens |
| `enable_restore_boundary_dlp_audit` | auditable bypass / fresh-PII at replay | medium | doc-note | later (T3) |
| #988 production NER mandate | require `policy.ner.model_dir` in prod profile | small | doc-note | adopt (T2), decision on profile shape |
| `gaze-assembly` `core` rulepack | #988 defense-in-depth (complementary) | medium | doc-note | later / decision-gated |
| `ToolDescriptor.output_schema` + `ToolRegistry::list()` | self-describing capability metadata | small | none (internal) | later (pattern for #1490) |
| #1490 discovery via `schema`/`list_tables` metadata or `check`/`serve` flags | SPEC-safe discovery | medium | doc-note | later / decision-gated (T4) |
| gaze-token-bridge indexed log search | index server log files | medium | **spec-amendment** | decision-gated (T6, §5) |
| operator-tier (`RestoreTool` / `ExportSessionTokensTool`) | — | trivial | spec-amendment | **skip** (§6) |
| `gaze-mcp-bridge` | actions / proxy axis | large | spec-amendment | **skip** (§4) |

## 8. Dependency-ordered tracks (T0–T6)

| Track | What | Depends on | Effort | SPEC | Decision-gated? |
|---|---|---|---|---|---|
| **T0 — gaze bump** | Bump pins (`Cargo.toml:46-48`), `cargo update`, regen lock, pre-push gate, refresh goldens. **Acceptance:** suite green + `Error::RecognizerDetect` surfaces. | — (foundational; blocks all) | small | none | no |
| **T1 — restore consolidation** | Replace three restore loops with `restore_strict_text` / `restore_with_policy_telemetry`; surface `RestoreTelemetry` in `replay`; update `docs/replay.md`. | T0 | small | none | no |
| **T2 — #988 production NER mandate** | Require `policy.ner.model_dir` in a production profile (safe now that NER fails closed). **Decision:** "production" profile tier vs per-profile flag. | T0 | small | doc-note | yes (profile shape) |
| **T3 — restore-boundary DLP audit** | Enable `enable_restore_boundary_dlp_audit`; route audit events to the SQLite manifest plane. | T0 + T1 | medium | doc-note | no (later) |
| **T4 — #1490 discovery** | SPEC-safe capability discovery: enrich `schema` / `list_tables` metadata and/or add `check` / `serve` flags. **Decision:** stay SPEC-safe (recommended) vs add a new `list_profiles` tool (SPEC amendment). | T0 | medium | doc-note (SPEC-safe path) | yes (surface) |
| **T5 — #1492 richer log filtering** | Borrow bridge patterns: layered deny-by-default filter policy, bounded caps (`response_bytes` / `content_blocks` / `call_timeout`), path-only audit. | T0 | medium | none (within `log_*` tools) | no |
| **T6 — indexed log search (gaze-token-bridge)** | Prototype Option B (bridge runtime through our `Session` / `Pipeline`). **Gates:** new tool vs CLI subcommand (SPEC either way); R3 namespace unification; Kiji model dependency; pre-1.0 risk. Spike on a throwaway corpus before a SPEC PR. | T0 | medium | **spec-amendment** | yes (multiple) |

**Removed from scope:** `gaze-mcp-bridge` adoption; enabling the token-bridge operator-tier
MCP tools.

## 9. Decision gates (what needs a maintainer call)

Three items cannot proceed on engineering judgment alone:

1. **token-bridge surface (T6).** Pursue indexed log search at all? If yes, confirm Option
   B (own-namespace) over Option A, and decide new MCP tool vs CLI-only subcommand — both
   are SPEC amendments. Gated additionally on R3 namespace unification landing, the Kiji
   model dependency, and pre-1.0 churn.
2. **#1490 discovery surface (T4).** Stay SPEC-safe (enrich existing responses / CLI flags
   — recommended) or open a SPEC amendment for a dedicated `list_profiles` tool.
3. **#988 production NER (T2).** Is "production" a distinct profile tier, or a per-profile
   flag mandating `ner.model_dir`? This shapes the profile schema in `docs/profiles.md`.

## References

- [SPEC.md](../SPEC.md) — locked product surface, threat model, anti-features.
- [CLAUDE.md](../CLAUDE.md) — non-negotiables (no raw SQL, chokepoint routing, snapshot
  encryption assumption, SSH argv construction, `LensValue` decode-failure policy).
- [docs/replay.md](./replay.md) — `replay` usage and snapshot operator controls (updated by T1/T3).
- [docs/profiles.md](./profiles.md) — profile schema (touched by T2).
- [CHANGELOG.md](../CHANGELOG.md) — current pin is `0.9.0-rc.1`; T0 moves it to `0.11.x`.
