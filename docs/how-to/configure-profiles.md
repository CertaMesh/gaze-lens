# Configure a profile

A profile tells `gaze-lens` what source to read (database or SSH log), which PII
policy to apply, and how to resolve the database secret. This recipe configures
one and validates it. It assumes you have a working binary (see the
[getting-started tutorial](../tutorials/getting-started.md)).

For the full field-by-field schema and merge-rule table, see
[reference/profile-schema.md](../reference/profile-schema.md). This page is the
task recipe.

## Prerequisites

- A `gaze-lens` binary on `PATH`.
- A read-only database user, **or** an SSH-reachable host with app logs.
- The database password available as an environment variable or in your OS
  keyring (never as a literal in TOML — `gaze-lens` rejects `password = "..."`).

## Decide the project/user split first

Profiles load from two files and merge by role:

- **Project file** `.gaze-lens.toml` — owns PII policy and identity: `policy`,
  `schema_tokenize`, `schema_allowlist`, database name, username, secret
  reference, `readonly_required`. Commit this so the team shares one policy.
- **User file** `~/.gaze-lens/profiles.toml` — owns laptop-specific transport:
  `host`, `port`, SSH host, local forwarded port, local SQLite path. Keep this
  out of version control.

When both files define a profile of the same name, project wins for policy/identity
and user wins for transport. This lets you commit policy while each operator keeps
their own connection details.

## Steps

1. **Generate the profile with guided init.** Run from the project you want the
   agent to investigate:

   ```sh
   gaze-lens init --profile prod
   ```

   `init` prompts for source kind (`mysql`, `postgres`, `sqlite`, `ssh-log`),
   connection details, and where to write the file. Use `--scope project` or
   `--scope user` to force a destination; init **refuses** to write transport
   overrides into a project file and **refuses** to write policy into a user file,
   enforcing the role split above.

2. **Set the database secret.** Pick exactly one backend (a profile that sets both,
   or neither for a DB source, is rejected at load):

   ```toml
   # legacy env backend
   password_env = "GAZE_LENS_DB_PASSWORD"

   # explicit env backend (preferred for new profiles)
   secret = { type = "env", var = "GAZE_LENS_DB_PASSWORD" }

   # native OS keyring backend
   secret = { type = "keyring", service = "gaze-lens", account = "prod" }
   ```

   Use `password_env` (or `secret.type = "env"`) on headless Linux, containers, or
   any host without an unlocked Secret Service — `check` reports the keyring as
   unavailable there rather than synthesizing a DBus session.

3. **(Optional) Discover a deployed `.env` over SSH.** For Laravel-style targets,
   `init` can read a deployed `.env` once at setup time to pre-fill `DB_*` keys:

   ```sh
   ssh-add ~/.ssh/id_ed25519
   ssh-keyscan -t ed25519 app01.example.invalid >> ~/.ssh/known_hosts
   gaze-lens init --profile prod \
     --discover-ssh-host deploy@app01.example.invalid \
     --discover-env-path /var/www/app/.env
   ```

   Discovery is **setup-time only** — it never re-reads the remote file during a
   query. Take the default path (keep host/port/database, enter a *separate*
   read-only credential for keyring storage) rather than copying the production
   credential verbatim. SSH runs with `BatchMode=yes` and strict host-key checking,
   so load your key and pin the host first as shown.

4. **Validate before granting access.** `check` parses the profile and policy,
   builds the Gaze pipeline, probes the secret backend, and does a read-only source
   ping — with no manifest or snapshot side effects:

   ```sh
   gaze-lens --project-config .gaze-lens.toml check --profile prod
   ```

   The secret line reads `secret: ok`, `NOT FOUND`, `ACCESS DENIED`, or
   `BACKEND UNAVAILABLE` (locator only — never password bytes). Add
   `--explain-risk` for a local trust report of what the profile exposes.

## Re-running init is safe (merge semantics)

`init` is additive. It preserves unrelated `[[profiles]]` entries verbatim
(comments and formatting survive), **refuses** to overwrite a same-named entry
unless you pass `--allow-overwrite`, skips the write entirely when the rendered
TOML matches what's on disk (prints `no changes`), and writes atomically. So you
can re-run `init` to add a second profile without disturbing the first.

`auto_purge` is project-only: it is written solely under `--scope project-auto-purge`,
and a profile defined *only* in the user file is forced to `off` regardless of any
value set there.

## Notes

- **MySQL `DATETIME`** has no timezone; `gaze-lens` normalizes it to UTC RFC3339
  by default. Convert source-side if you need a different interpretation.
- To hide sensitive *schema* names, set `schema_tokenize = true` — see
  [set-up-production-NER.md](./set-up-production-NER.md) for the production tier
  and [reference/policy-schema.md](../reference/policy-schema.md) for the policy
  file. For *why* the project owns policy while the user owns transport, see
  [explanation/threat-model.md](../explanation/threat-model.md).

## Done when

- `gaze-lens check --profile prod` reports `secret: ok` and a successful source
  ping (or, for `ssh_log`, successful command-construction validation).
- The PII policy lives in the committed project file and your laptop-specific
  transport lives in `~/.gaze-lens/profiles.toml`.
