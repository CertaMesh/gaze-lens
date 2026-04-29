# Changelog

## [0.2.0] — 2026-04-29

### Added
- `gaze-lens demo` subcommand: inline-replay PII demonstration in a single process; tokenizes seeded canned data through `Session::dispatch_tool` and inline-restores via `gaze::Session::import`. Tempdir-isolated; no persistent state. (#15)
- Snapshot retention policy via profile fields `snapshot_retention_days: Option<u32>` and `auto_purge: AutoPurge` with `Off` / `Warn` / `Purge` variants. Default: unlimited retention (D3 audit-of-record preserved). (#16)
- `ManifestMaintenance` type for sweep operations; runs synchronously before session construction. Sweep tombstones expired snapshots via `UPDATE calls SET purged_at_ms = ?, snapshot_ref = NULL` (audit row preserved per D3). (#16)
- Manifest schema bump: `user_version 2 → 3` adds `purged_at_ms INTEGER` column. Forward-compatible additive migration. (#16)
- `LensError::SnapshotPurged` carries concrete `retention_days` for honest replay error messages. (#16)
- README install snippet pointing at v0.2.0 binary release + §"Building from source" fallback. (#15)
- SPEC.md / CLAUDE.md / ARCHITECTURE.md amended: v0.2 permits 6th CLI subcommand `demo`; manifest schema versioning subsection. (#13, #15)

### Changed
- TOML profile-load errors now include file path + parser span. Required-field-missing errors name the missing field + profile name. (#14)
- D7 invariant tightened: CLI `Display`/`eprintln!` paths now sanitize WHERE clause contents and grep patterns equivalently to MCP frontend. (#14)
- `auto_purge` enforces destructive-default-deny merge rule: `merged_auto_purge = project.auto_purge && user.auto_purge`. User-only profiles with `auto_purge != Off` are downgraded to `Off` with a stderr warning naming the profile. (#16)

### Locked decisions (carried forward)
- Public surface remains 5 MCP tools + 6 CLI subcommands.
- All retrievals route through `Session::dispatch_tool` (D4 invariant).
- No raw SQL.
- Snapshot files stay `0600` in `0700` directory; encryption-at-rest deferred to v1.x.
- Pre-push hook gates fmt/clippy/test locally; CI for binary releases only.

### Deferred to v0.3.0+
- cargo-dist release-preflight workflow + GLIBC 2.17 floor config (PR 2/3, blocked on GH Actions billing #306).
- release.yml smoke-test gating between `host.needs` and `build-global-artifacts.needs` (same).
- PG decimal precision (#262), column-rule isolation (#247), snapshot encryption-at-rest, crates.io publish.
