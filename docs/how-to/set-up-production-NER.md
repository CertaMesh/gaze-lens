# Set up production NER (fail-closed)

A profile that points at production data should mark itself `production = true`.
A production profile **must** configure a named-entity-recognition (NER) model;
without one, `gaze-lens` refuses to start the session — before any data is read.
This recipe enables that posture.

This is the honest fix for the nested-JSON person-name leak class. For *why* it
works and what it does and does not guarantee, see
[explanation/threat-model.md](../explanation/threat-model.md) and
[explanation/gaze-0.11-adoption.md](../explanation/gaze-0.11-adoption.md) §3.2.

## Prerequisites

- A working database or `ssh_log` profile (see
  [configure-profiles.md](./configure-profiles.md)).
- A NER model directory on the laptop — the expected family is a Davlan mBERT
  model. `gaze-lens` bundles no default model; you supply the directory.

## Why a model is mandatory for production

Without an NER model the redaction pipeline only catches regex-detectable PII
(emails). Arbitrary person names — including names nested inside JSON column
values — would pass through unredacted. The Gaze runtime makes NER **fail-closed**:
a model load or backend error aborts redaction rather than silently passing raw
text. Mandating a model therefore turns a misconfiguration into a *startup
failure* instead of a *query-time leak*.

The mandate is opt-in: non-production profiles are unaffected. Mark prod-data
profiles `production = true` to enforce it.

## Steps

1. **Mark the profile production.** In the profile's project file:

   ```toml
   [[profiles]]
   name = "prod"
   production = true
   policy = "./gaze-policy.toml"   # this policy file MUST set [ner].model_dir

   [profiles.source]
   kind = "mysql"
   host = "db.example.invalid"
   port = 3306
   database = "app"
   username = "gaze_ro"
   password_env = "GAZE_LENS_DB_PASSWORD"
   readonly_required = true
   ```

2. **Point the policy at the NER model.** In the policy file referenced above:

   ```toml
   # ./gaze-policy.toml
   [ner]
   model_dir = "/opt/gaze/models/ner"

   [policy.database]
   ```

   See [reference/policy-schema.md](../reference/policy-schema.md) for the full
   `[ner]` and `[policy.database]` field set.

3. **Verify it fails closed.** With `production = true` but no `[ner].model_dir`,
   confirm the refusal:

   ```sh
   gaze-lens --project-config .gaze-lens.toml check --profile prod
   ```

   `check` (and `serve` / `query`) refuse to build the session with a clear error,
   before any data is read. Add the `model_dir` and re-run — `check` now builds the
   pipeline and pings the source.

## Know the merge rule: production is escalate-only

`production` cannot be downgraded across the project/user file merge. If **either**
the project or the user file marks a profile `production = true`, it is production.
A user-file profile cannot silently turn off a project's production mandate. So an
operator's local override can make a profile *more* strict, never less.

## Caveat (honest uncertainty)

The win is *fail-closed behavior* — no silent leak on a backend error — not perfect
name coverage. `gaze-lens` cannot guarantee the supplied model's recall on
arbitrary nested-JSON shapes. As defense-in-depth, consider column-ruling known
JSON name fields in the policy file.

## Done when

- The profile sets `production = true` and its policy file sets `[ner].model_dir`.
- `gaze-lens check --profile prod` **refuses** when the model is missing and
  **succeeds** once it is configured.
- For `ssh_log` production profiles, `log_grep` defaults to the safer keyword mode
  in your workflow — see [search-logs.md](./search-logs.md).
