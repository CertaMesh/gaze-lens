# Replay

`gaze-lens replay <session_ulid>` restores a whole session's tokenized tool arguments from the local manifest and snapshot files.

v1 replay is whole-session only. Per-call selectors are rejected with a `not in v1; tracked as v1.x candidate` error because current Gaze replay APIs restore a session snapshot, not one call's token map.

## Usage

```sh
gaze-lens replay --manifest ~/.gaze-lens/manifest.sqlite <session_ulid>
```

The command reads the gaze-lens-local manifest, walks successful calls for the session, imports each referenced snapshot with `gaze::Session::import(snapshot)`, and prints restored call-history JSON.

## Snapshot Handling

Snapshots are local PII vault material. They contain the raw token mappings required to turn values like `Email_1` back into original production values.

Required operator controls:

- Run disk encryption on the laptop: FileVault on macOS or LUKS on Linux.
- Keep snapshot files local. `gaze-lens` does not auto-upload snapshots to backup services.
- Exclude `~/.gaze-lens/snapshots/` from cloud backups when your threat model treats backup providers or synced devices as out of scope for raw PII.

v1 stores snapshots as `0600` files under a `0700` directory. Per-snapshot encryption-at-rest and snapshot TTL/GC are v1.x hardening items.
