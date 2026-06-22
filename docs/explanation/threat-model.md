# Why the threat model looks the way it does

This is an *explanation* of the gaze-lens threat model: the reasoning behind which
threats it defends against, which it deliberately pushes onto the operator, and why the
line is drawn where it is. It is not the authoritative list.

> **The canonical, locked threat model lives in
> [reference/spec.md → Threat model](../reference/spec.md#threat-model).** That list — its
> in-scope items, out-of-scope items, and `v1 stop-gates` — is the single source of truth.
> This page exists to give you the *why* without copying the *what*; if the two ever seem
> to disagree, the spec wins. Read them together: the spec for the contract, this page for
> the intent.

## The one boundary that matters

Strip everything else away and gaze-lens defends a single line: **raw production data must
not cross into the model provider.** An engineer debugging a live incident has two bad
options today — give the agent raw database and log access and leak names, emails, and
addresses to whoever hosts the model; or give the agent nothing and burn an hour
eyeballing `psql` while the incident runs hot. gaze-lens is the third option:
pseudonymized agent access with an auditable, reversible token mapping the operator can
replay locally.

Almost every in-scope threat is a different way that raw data could sneak across that one
boundary, and almost every mitigation is the same move: **force the data through the
redaction chokepoint before it can become output or an audit row.** Understanding the
threat model is mostly understanding why that single chokepoint is enough to cover so many
distinct attack shapes — and being honest about the handful of cases it cannot cover.

## Why these threats are *in scope*

The in-scope list is the set of leaks gaze-lens can structurally prevent on its own,
because they all funnel through code it controls. The common mechanism is that every
retrieval — DB query, schema lookup, log tail, even the human CLI `query` — routes through
`Session::dispatch_tool` and the `gaze_mcp_core::PiiEnvelope`, which runs
`gaze::Pipeline::redact` *before* any value reaches the agent or the manifest. (How that
ordering works, and why it is enforced at the type level rather than by discipline, is the
subject of the sibling page on
[pseudonymization and replay](./pseudonymization-and-replay.md).)

That one property explains the shape of the in-scope reasoning:

- **Raw data reaching the LLM through any path.** This is the headline threat and the
  reason the chokepoint exists. "Any path" is the important phrase: there is deliberately
  no second, direct-to-source code path that could bypass redaction. The CLI does not get
  a fast lane; it dispatches through the same envelope the MCP server does.

- **Leaks via SQL string-injection or vendor side effects.** The defensive choice here is
  to *not accept the dangerous input at all*. v1 takes only a canned structured query
  shape (`{table, columns?, where?, order_by?, limit?}`) compiled to parameterized SQL —
  there is no raw-SQL string for an agent (or a confused operator) to smuggle a payload
  through. Raw SQL behind an opt-in profile flag is a deliberately deferred v1.x candidate,
  not a v1 feature, precisely because it reopens this surface.

- **Remote command injection through the SSH log tooling.** Log tail/grep shells out over
  SSH, which is exactly where naive string interpolation gets you a shell-injection bug.
  The mitigation is structural: a fixed `ssh -- <host> tail -n N -- <quoted_path>` argv
  form with validated host arguments (a `-`-prefixed host is rejected), never an
  interpolated command string. The argument vector is the boundary, not a string the
  remote shell re-parses.

- **Operator-error retrieval bypass.** The subtle one. If the human-facing CLI took a
  shortcut around redaction "because it's just for the operator," an operator could
  accidentally page raw PII into a terminal that later gets shared, screenshotted, or
  pasted to the agent. So the CLI is held to the same audit-and-redaction path as MCP by
  design (D4) — there is no privileged human path.

- **Schema-name leak.** Column and table names like `customer_email_unhashed` can
  themselves be sensitive. Schema names are shown raw *by default* for agent utility, but a
  profile can opt into presentation tokenization (`schema_tokenize = true`) with an
  explicit allowlist. This is **presentation privacy, not access control** — tokenizing a
  label does not grant or revoke query access; that is governed separately per column. The
  separation is intentional, so nobody mistakes a redacted label for a permission boundary.

- **Raw-PII presence probing via regex `log_grep`.** This one is genuinely interesting and
  gets its own section below, because it is the rare in-scope item that is mitigated by a
  *recommendation and an alternative mode* rather than fully closed — and being honest
  about that is part of the threat model.

## Why these threats are *out of scope*

The out-of-scope list is not a list of things gaze-lens forgot. It is a deliberate,
honest trust boundary. gaze-lens is a laptop-side tool whose job is to defend the
LLM-egress boundary — not to re-implement disk encryption, a keyring, OS process
isolation, or SSH authentication. Each out-of-scope item is a place where the *right* layer
to defend is something other than gaze-lens, and pretending otherwise would be security
theater. The guiding principle: **state the assumption loudly, fail closed where we can,
and defer to the correct layer where we can't.**

- **Laptop disk compromise.** Snapshot files contain the raw token-to-PII mappings —
  they *are* the vault. A stolen laptop with an unencrypted disk leaks them. v1 does not
  implement per-snapshot encryption-at-rest; instead it *assumes* FileVault (macOS) or LUKS
  (Linux) and says so explicitly. This is the single most load-bearing assumption in the
  product, which is why it is repeated in the spec, the snapshot docs, and the CLAUDE.md
  non-negotiables. Per-snapshot encryption is a tracked v1.x hardening item, not a silent
  gap.

- **Keyring availability.** Native secrets reuse the operator's platform keyring rather
  than gaze-lens caching passwords in-process. The honest consequence: headless Linux,
  containers, locked keyrings, and machines without a Secret Service provider report
  `BACKEND UNAVAILABLE` instead of gaze-lens synthesizing a DBus session to paper over the
  gap. Operators in those environments use the env secret backend. Refusing to fake a
  keyring is the fail-closed choice.

- **Same-uid or root attacker after process compromise.** Snapshot files are `0600` in a
  `0700` directory, which stops *other-user* attackers. It does not stop root or a same-uid
  attacker who has already compromised the process — and no file-permission scheme could.
  The threat model says so rather than implying `0600` is stronger than it is.

- **SSH-side credential compromise** and **database write privilege.** gaze-lens reuses
  `~/.ssh/config` and the SSH agent, and it never writes to the database. SSH auth is the
  operator's responsibility, and the DB user *must* be configured read-only at the database
  side — gaze-lens cannot enforce read-only from the client; the database is the right
  place for that boundary.

- **Backups.** Snapshots are never auto-uploaded, but gaze-lens cannot police the
  operator's backup software. If your threat model treats a cloud backup provider as out of
  scope for raw PII, excluding `~/.gaze-lens/` is your job.

- **Cross-profile token correlation.** A single `serve` process running multiple profiles
  shares one Gaze session, so an entity appearing in profile A and profile B redacts to the
  *same* token in both. That correlation is intentional (Conversation-scope semantics) and
  is the same thing you get from two CLI `query` calls in one session — it is what makes
  replay coherent. Operators who genuinely need disjoint token spaces run separate `serve`
  processes per profile group until v2 introduces per-profile scoping. The reasoning behind
  *why* the correlation is deterministic, and why that is a feature for replay rather than a
  bug, is covered in [pseudonymization and replay](./pseudonymization-and-replay.md).

## The regex `log_grep` oracle — an honest residual risk

The most instructive item in the whole model is the one threat gaze-lens *documents rather
than fully closes*. It is worth understanding because it shows how a leak can exist even
when no raw value is ever returned.

`log_grep` in its default `mode: "regex"` evaluates the match predicate over the **raw**
log text, while only the lines it returns are redacted before display. That asymmetry —
predicate-on-raw, display-redacted — is exactly how ordinary `grep` works, and it is
preserved byte-for-byte for v0.4 compatibility. But it means an agent can craft a regex
that would only match a specific raw substring (an email local-part, an account id) and
then learn, from whether *any* line matched and the reported match count, whether that raw
value is present in the log — **without the value ever appearing in the tokenized output.**
The displayed lines are redacted; the boolean answer is not. That makes a crafted regex a
**one-bit-per-query oracle** over raw data.

Why keep it at all? Because the leak is intrinsic to predicate-over-raw search semantics —
you cannot have real regex matching against raw lines without the match result being a
function of the raw lines — and because the searched window is still fully manifested
(audit records *data accessed*, not merely data returned), so the access is never silent.

The mitigation is a different mode rather than a filter on the output:
**`mode: "keyword"` runs the predicate over the *same* redacted text the agent sees.** A
token-shaped term can only match the redacted token already present in the window; it can
never probe a raw value, because the raw value is not on the side of the boundary the
predicate runs on. This is the general lesson worth carrying away: the fix for an oracle is
not to scrub the output harder, it is to move the predicate to the safe side of the
redaction boundary.

Because the risk is real but bounded, the posture is a strong recommendation, not a hard
block: operators handling sensitive logs — and `production`-tier profiles in particular —
should prefer keyword mode, and the `log_grep` tool description surfaces the caveat to
agents directly. (For how to actually run keyword searches, see the
[search-logs how-to](../how-to/search-logs.md); for why a production tier mandates an NER
model in the first place, see [set up production NER](../how-to/set-up-production-NER.md).)

## Where to go next

- **The authoritative, locked list:** [reference/spec.md → Threat model](../reference/spec.md#threat-model).
- **How redaction, audit, and replay actually fit together:** [pseudonymization and replay](./pseudonymization-and-replay.md).
- **The implementer-facing stop-gates and chokepoint enforcement:** [reference/architecture.md](../reference/architecture.md).
