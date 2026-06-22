# Replay a session to restore original values

After an agent investigates with pseudonymized results, you replay its session
locally to turn tokens like `<EMAIL:Addr_1>` back into the original production
values. Replay reads the local manifest and snapshot files; it never touches the
source. This recipe runs a replay and reads its telemetry.

For *why* tokens are reversible and how the manifest/snapshot planes relate, see
[explanation/pseudonymization-and-replay.md](../explanation/pseudonymization-and-replay.md).

## Prerequisites

- The `gaze-lens` manifest at `~/.gaze-lens/manifest.sqlite` (written automatically
  by every MCP and CLI retrieval).
- The session ULID you want to restore — from the agent session, or by inspecting
  the manifest.
- Disk encryption already enabled (see operator controls below) — snapshots are
  raw PII vault material.

## Steps

1. **Run replay for the session.** Replay is whole-session only in v1:

   ```sh
   gaze-lens replay --manifest ~/.gaze-lens/manifest.sqlite <session_ulid>
   ```

   It walks the successful calls for that session, imports each referenced snapshot
   via `gaze::Session::import`, and prints restored call-history JSON. (Per-call
   selectors are rejected — they are a v1.x candidate.)

2. **Read the per-call telemetry.** Each call includes a `restore_telemetry` block
   from Gaze whole-text restore. Replay uses strict policy: known tokens are
   restored byte-exactly; token-shaped strings that are *not* in the snapshot are
   left in place and reported with a failed restore decision (rather than guessed).

3. **Read the session summary.** The session includes `restore_telemetry_summary`
   with success / partial / failed call counts, plus unknown-token, manifest-bypass,
   and fresh-PII counts. Use it to confirm every call restored cleanly; investigate
   any failed or partial entries.

## Snapshot operator controls (required)

Snapshots hold the raw token→value mappings. v1 does not encrypt them per-file
(it stores `0600` files under a `0700` directory), so the threat model assumes:

- **Run disk encryption** on the laptop — FileVault on macOS, LUKS on Linux.
- **Keep snapshot files local** — `gaze-lens` never auto-uploads them.
- **Exclude `~/.gaze-lens/snapshots/` from cloud backups** when your threat model
  treats backup providers or synced devices as out of scope for raw PII.

See [explanation/threat-model.md](../explanation/threat-model.md) for the full
at-rest threat boundary, and [reference/cli.md](../reference/cli.md) for all
`replay` flags.

## Done when

- `gaze-lens replay <session_ulid>` prints restored call history with original
  values in place of tokens.
- `restore_telemetry_summary` shows the expected success count and no unexpected
  failed/partial calls.
- Disk encryption is on and `~/.gaze-lens/snapshots/` is excluded from cloud
  backups.
