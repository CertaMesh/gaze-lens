# Getting started with gaze-lens

This tutorial walks you from an empty terminal to your **first pseudonymized
query and its replay** — the full gaze-lens loop — in about ten minutes.

By the end you will have:

1. Installed the `gaze-lens` binary and confirmed it works.
2. Watched it turn real-looking PII into reversible tokens, and back again.
3. Pointed it at your own database, run a redacted query, and replayed that
   session to recover the original values.

You don't need to understand the internals yet. Follow each step in order,
check the **✓ Success looks like** note, and you'll arrive at a working setup.
When you want to go deeper, the [Where to next](#where-to-next) links at the end
point to the how-to guides and reference.

> **What you need:** an Apple Silicon Mac (other platforms build from source —
> see [step 1](#step-1--install-gaze-lens)), a terminal, and the `sqlite3`
> command (preinstalled on macOS). About ten minutes.

---

## Step 1 — Install gaze-lens

Download the latest prebuilt binary and unpack it:

```sh
curl -L https://github.com/CertaMesh/gaze-lens/releases/latest/download/gaze-lens-aarch64-apple-darwin.tar.xz | tar -xJ
```

This drops a `gaze-lens` executable in the current directory. Move it onto your
`PATH` if you like (`mv gaze-lens /usr/local/bin/`); this tutorial just calls
`./gaze-lens`.

> **Not on Apple Silicon?** Build from source instead — you need stable Rust
> 1.89 or newer:
>
> ```sh
> git clone https://github.com/CertaMesh/gaze-lens
> cd gaze-lens
> cargo build --release
> ```
>
> The binary lands at `./target/release/gaze-lens`. On Linux, install
> `pkg-config` and `libdbus-1-dev` (Debian/Ubuntu names) first for the keyring
> backend.

**✓ Success looks like:** `./gaze-lens --version` prints a version number.

---

## Step 2 — See it work with `demo`

Before touching any real data, run the built-in demonstration. `demo`
tokenizes a tiny canned dataset and restores it again, all in one process. It
writes **nothing** to disk — everything lives in a tempdir that is wiped on
exit — so it's completely safe to run.

```sh
./gaze-lens demo
```

You'll see two panels. First, **what an AI agent would see** — every email,
phone number, and SSN replaced by a token:

```json
=== Tokenized output (what an agent sees) ===
{
  "Rows": {
    "rows": [
      {
        "email": "<30a2acc2:Email_1>",
        "id": 1,
        "note": "primary contact",
        "phone": "<30a2acc2:Custom:phone_1>"
      },
      {
        "email": "<30a2acc2:Email_2>",
        "id": 2,
        "note": "ssn on file: <30a2acc2:Custom:ssn_1>",
        "phone": "<30a2acc2:Custom:phone_2>"
      },
      {
        "email": "<30a2acc2:Email_3>",
        "id": 3,
        "note": "archived",
        "phone": "Null"
      }
    ],
    "truncated_at": []
  }
}
```

Then, **what you can recover locally** — the original values restored from the
token map:

```json
=== Restored output (what `gaze-lens replay` would show on a real session) ===
{
  "Rows": {
    "rows": [
      {
        "email": "alice@example.com",
        "id": 1,
        "note": "primary contact",
        "phone": "555-123-4567"
      },
      ...
    ]
  }
}
```

That's the entire idea of gaze-lens in one command: **the agent sees tokens, you
keep the key.** A few things to notice:

- `<30a2acc2:Email_1>` is a *token*. The `Email_1` part tells you it's the first
  email; the `30a2acc2` prefix is a per-session salt — **yours will be a
  different hex string, and that's expected.**
- The same original value always maps to the same token within a session, so an
  agent can still reason about the data ("these two rows share an email")
  without ever seeing the email.
- The restored panel is exactly what `replay` reconstructs from a real session.
  You'll do that yourself in [step 7](#step-7--reveal-the-originals-with-replay).

**✓ Success looks like:** two panels printed side by side, ending with
`(demo state lived in a tempdir and was cleaned up; nothing was written to
~/.gaze-lens/)`.

---

## Step 3 — Point gaze-lens at your own database

Now let's do the same thing against a real source. We'll use a small local
SQLite database so you can follow along with zero infrastructure. Create one:

```sh
sqlite3 app.sqlite "CREATE TABLE users(id INTEGER PRIMARY KEY, email TEXT, phone TEXT, note TEXT);
INSERT INTO users VALUES
 (1,'alice@example.com','555-123-4567','primary contact'),
 (2,'bob@beta.io','555-987-6543','beta tester'),
 (3,'carol@example.net',NULL,'archived');"
```

> Already have a SQLite database? Point the next step at that file instead and
> use one of its tables.

Create a profile with guided `init`:

```sh
./gaze-lens init --profile dev
```

`init` asks a few questions. For this tutorial:

- **Source kind:** choose `sqlite`.
- **Path:** enter `./app.sqlite` (the file you just created).
- **Claude Code `.mcp.json` / AGENTS.md:** you can decline these for now — we
  wire an agent up in a separate guide. (`init` only writes Codex or Cursor
  config when you ask for it explicitly.)

`init` writes a `.gaze-lens.toml` in the current directory that looks like this:

```toml
[[profiles]]
name = "dev"
source = { kind = "sqlite", path = "./app.sqlite" }
```

**✓ Success looks like:** a `.gaze-lens.toml` file exists and names a `dev`
profile pointing at your database.

---

## Step 4 — Tell gaze-lens which columns hold PII

gaze-lens is **fail-closed**: it will not tokenize a database column you haven't
declared. Out of the box, a database profile detects email-shaped text but
*preserves* it — so it would hand your agent raw data. The next step (`check`)
would warn you about exactly that. Let's close the gap before we ever run a
query.

Create a policy file named `gaze-policy.toml` next to your profile:

```toml
[policy.database]

[[policy.database.columns]]
column = "email"
class = "email"
action = "tokenize"

[[policy.database.columns]]
column = "phone"
class = "phone"
action = "tokenize"
```

Each rule says: "values in this column are this kind of PII — tokenize them."
Then point the profile at the policy by adding one line to `.gaze-lens.toml`:

```toml
[[profiles]]
name = "dev"
policy = "./gaze-policy.toml"
source = { kind = "sqlite", path = "./app.sqlite" }
```

**✓ Success looks like:** both `.gaze-lens.toml` (now with a `policy = …` line)
and `gaze-policy.toml` exist in your project directory.

---

## Step 5 — Run the safety check

`check` is the gate you run *before* letting any agent near a profile. It
validates that the profile parses, the policy builds, the source is reachable,
and the redaction pipeline is wired — without writing anything to your audit
log.

```sh
./gaze-lens check --profile dev
```

```text
profile: ok (dev)
policy: ok
secret: ok (none not required)
source: ok
pipeline: ok
```

Every line says `ok`, and there's no `WARNING`. (If you skipped step 4, `check`
would instead warn that the profile "uses email-regex-only detection" and would
pass PII through raw — that warning is gaze-lens telling you the door is open.)

**✓ Success looks like:** five `ok` lines and no warnings.

---

## Step 6 — Run your first redacted query

Now ask gaze-lens for data the same way an agent would — through a canned,
structured query (gaze-lens accepts no raw SQL):

```sh
./gaze-lens query --profile dev --table users --limit 5
```

```json
{
  "clean": {
    "Rows": {
      "rows": [
        {
          "email": "<4c47ef92:Email_1>",
          "id": 1,
          "note": "primary contact",
          "phone": "<4c47ef92:Custom:phone_1>"
        },
        {
          "email": "<4c47ef92:Email_2>",
          "id": 2,
          "note": "beta tester",
          "phone": "<4c47ef92:Custom:phone_2>"
        },
        {
          "email": "<4c47ef92:Email_3>",
          "id": 3,
          "note": "archived",
          "phone": "Null"
        }
      ],
      "truncated_at": []
    }
  },
  "snapshot_ref": {
    "path": "~/.gaze-lens/snapshots/01KVQD7WFN28582RFK6EDFJJFK.snap"
  }
}
```

The emails and phone numbers from *your* database came back as tokens — exactly
what an agent investigating production would receive. The `note` column stays
raw because you didn't declare it as PII; that's the fail-closed design working
as intended.

Two things to note in the output:

- Your token prefix (`4c47ef92` here) will differ — it's the per-session salt
  again.
- **`snapshot_ref.path` ends in your session id.** The filename before `.snap`
  — here `01KVQD7WFN28582RFK6EDFJJFK` — is the **session ULID** you'll need in
  the next step. Copy it.

**✓ Success looks like:** every `email` and `phone` value is a `<…>` token, and
the output shows a `snapshot_ref.path`.

---

## Step 7 — Reveal the originals with `replay`

The tokens you just saw are reversible — but only locally, by you. `replay`
walks the recorded session from your local manifest, imports the matching
snapshot, and restores the tokenized values to their originals.

Pass the session ULID you copied from `snapshot_ref.path`:

```sh
./gaze-lens replay 01KVQD7WFN28582RFK6EDFJJFK
```

```json
{
  "lens_session_id": "01KVQD7WFN28582RFK6EDFJJFK",
  "calls": [
    {
      "call_id": "01KVQD7WFWWP9V2KWJY6PVXQZZ",
      "tool_name": "query",
      "restored_args_json": "{\"columns\":null,\"limit\":5,\"profile\":\"dev\",\"table\":\"users\",...}",
      "restore_telemetry": {
        "restore_policy": "strict",
        "restore_decision": "success"
      }
    }
  ],
  "restore_telemetry_summary": {
    "success_calls": 1,
    "partial_calls": 0,
    "failed_calls": 0,
    "unknown_token_count": 0
  }
}
```

This is the operator's view: gaze-lens reconstructs what the agent did and
confirms the mapping is fully reversible. `restore_telemetry_summary` reports
`success_calls: 1` and `failed_calls: 0` — every token resolved back to a real
value, just like the restored panel you saw in the `demo`. The difference is
that this came from the durable manifest, so you can replay a session **days
later**, long after the agent's conversation has ended.

**✓ Success looks like:** the JSON shows your `lens_session_id` and
`restore_telemetry_summary.success_calls` is at least `1`.

---

## What you've learned

You ran the complete gaze-lens loop:

- An agent-facing tool (`query`) that returns **pseudonymized** data —
  `<…:Email_1>` instead of `alice@example.com`.
- A **fail-closed** posture, where `check` refuses to let undeclared PII through
  silently.
- A local, **reversible** audit trail that `replay` turns back into the original
  values whenever you need them.

That's the whole promise: your agent can investigate production data without
ever seeing the real names, emails, or phone numbers — and you keep the key.

## Where to next

- **[Configure profiles](../how-to/configure-profiles.md)** — project vs. user
  profiles, database secrets, schema tokenization, and SSH log sources.
- **[Wire up your MCP client](../how-to/wire-up-mcp-clients.md)** — connect
  Claude Code, Codex, or Cursor so your agent calls these tools directly over
  `gaze-lens serve`.
- **[CLI reference](../reference/cli.md)** — every subcommand and flag in full.
