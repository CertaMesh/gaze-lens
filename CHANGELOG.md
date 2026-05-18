# Changelog

## [0.4.0] — 2026-05-18

### Added
- Cross-platform binary roadmap documentation captures Windows, Linux,
  keyring, and recognizer-backend blockers, plus the CI proof gates required
  before adding new dist targets. (#61)
- v0.9 pseudonymization observability spec amendment in `SPEC.md` and
  `docs/v0.9-observability.md`, with the locked MCP/CLI surface unchanged.
  (#59)
- Public-readiness documentation scaffold with `LICENSE`, `SECURITY.md`,
  public CI workflow, README badges, and public release guidance. (#58)

### Changed
- Bumped Gaze runtime crates to `0.9.0-rc.1` (`gaze-pii`,
  `gaze-recognizers`, and `gaze-mcp-core`). The bump is API-compatible with
  `0.7.0`; `ort` remains in the default graph pending upstream optional-feature
  work tracked by todo #8 and `docs/cross-platform-roadmap.md`. (#62)
- Replaced real-looking example domains with non-resolving `.invalid` examples
  across docs, fixtures, and goldens for public release hygiene. (#60)
- Synchronized agent-facing docs with the v0.3.0 crates.io Gaze dependency
  flow.

### Fixed
- Configured database SSH tunnels are now opened at runtime for `query`,
  `serve`, and `check`, so tunneled MySQL/Postgres profiles can reach their
  production sources through the same guarded source path. (#56)
- `check` reuses the validated database password when opening MySQL/Postgres
  connections, avoiding a second secret lookup or prompt during the same
  profile check. (#57)
- Dropped a stale `schema_allowlist` warning assertion from `tests/cli_check.rs`
  that no longer matched the current warning text. (#63)

## [0.3.0] — 2026-05-11

### Changed
- Internal MCP dispatch now routes through `gaze_mcp_core::PiiEnvelope::dispatch`
  while keeping the locked 5-tool MCP surface unchanged: `query`, `schema`,
  `list_tables`, `log_tail`, and `log_grep`.
- Switched Gaze runtime dependencies from git-tagged packages to crates.io:
  `gaze = { package = "gaze-pii", version = "0.7.0" }`,
  `gaze-recognizers = "0.7.0"`, and `gaze-mcp-core = "0.7.0"`.
  The package alias keeps existing `use gaze::*` imports stable.
- Adopted the Gaze v0.7.0 redaction wave, including validator veto,
  anchored context, and ambiguity side-channel support in the shared
  redaction path.

### Added
- A chokepoint regression test pins the envelope ordering: manifest begin
  happens before source access, tool output is redacted, and manifest finish
  records the completed call row and snapshot reference.

### Removed
- Gaze git dependency setup is no longer required. `cargo install
  gaze-lens` can resolve all Gaze crates from crates.io without a repository
  token.

## [0.2.5] — 2026-05-08

### Fixed
- `init` discovered credential prompt help no longer duplicates menu labels
  above the select list, completing the prompt UX cleanup after the short
  selectable labels shipped in v0.2.4. (#49)
- `gaze-lens query` now emits stderr-only progress/loading status while
  preserving stdout/JSON output, and query/check `SourceError` failures now
  include generic private-DB/SSH-tunnel mitigation guidance. (#51)

## [0.2.4] — 2026-05-08

### Fixed
- `init` SSH discovery credential prompt choices now use short, stable
  selectable labels and keep longer explanations outside selectable rows,
  fixing terminal repaint/flood UX when operators navigate with arrow keys.
  A regression test pins the short-label contract.

## [0.2.3] — 2026-05-08

### Added
- README onboarding now gives a guided first-run path for installation,
  initialization, MCP wiring, agent verification, and replay. (#37, #551)
- `gaze-lens query` defaults to readable CLI output, with compact machine JSON
  available through `--format json`. (#42, #554)
- `init` prompts now explain profile scope and Laravel SSH discovery choices
  inline, so operators can choose intentionally during setup. (#43, #553)

### Changed
- `schema` and `list_tables` documentation and help text now spell out raw
  default presentation, tokenized mode, allowlist behavior, and profile reload
  expectations. Allowlist/profile reload tests pin that UX contract. (#44, #558)

### Fixed
- MySQL, Postgres, and SQLite adapters now force sqlx statement logging off
  with `LevelFilter::Off`, preventing SQL/log argument leakage through tracing
  subscribers. (#38, #242)
- `cli_serve` tests are hermetic against user profile configuration, avoiding
  local `~/.gaze-lens` state leaks into test results. (#45, #695)

## [0.2.2] — 2026-05-04

### Added
- `init` writes a guided setup flow that emits per-agent MCP snippets and an
  AGENTS.md primer alongside the generated profile, so first-run agents have
  copy-paste config for Claude Code, Cursor, Codex, and generic MCP clients.
  (#357, #360)
- `init` can optionally discover Laravel-style `DB_*` credentials by reading
  an explicit remote `.env` over SSH, then choose Path A (store discovered
  prod credential), Path B (recommended: host/database only plus separate
  readonly credential), or Path C (abort). Strict host-key checking is on by
  default; `--allow-new-ssh-host` opts into TOFU. Provenance metadata is
  persisted to `profile.toml`. (#358)
- Profile secrets now have an OS-native keyring backend
  (`profile.toml::secret_backend = "keyring"`). Falls back to plaintext when
  the native keyring is unavailable, with an explicit `--allow-plaintext`
  opt-in. macOS Keychain, Windows Credential Manager, and Secret Service on
  Linux are exercised through `keyring` crate. An ignored `integration-keyring`
  test gates compile-time API stability without requiring an unlocked
  runner. (#356)
- `gaze-lens check --explain-risk` emits a structured trust report covering
  redaction policy, source class, snapshot retention posture, and the
  process surface visible to the operator. Used by AI agents to verify the
  pseudonymization contract before issuing queries against an unfamiliar
  profile. (#359)

### Changed (BREAKING for MCP agents)
- `gaze-lens serve` now exposes a single MCP entry covering all configured
  profiles. Every MCP tool call (`query`, `schema`, `list_tables`, `log_tail`,
  `log_grep`) requires a new `profile: string` argument. Existing v0.2.x agents
  that did not send `profile` will receive `invalid_params` until updated.
  Run `gaze-lens init` to regenerate per-host MCP config and AGENTS.md guidance.
  CLI `query`/`demo` continue to work unchanged (single-profile mode defaults
  the `profile` arg to the configured profile name). (#355)

### Changed
- `schema` and `list_tables` now show raw table/column names by default for
  agent utility. Profiles can opt back into presentation tokenization with
  `schema_tokenize = true`; `schema_allowlist` only affects presentation in
  that mode. Query authorization remains governed by source schema policy.
- Bumped Gaze pinned dependency from `v0.4.6` to `v0.6.4`. `gaze::Value`
  conversion contract (D11) preserved through the existing exhaustiveness
  pin in `gaze_value_to_json`. Manifest serialization continues to use
  `serde_json` and is unaffected by the bump. (#24)
- Release configuration now limits prebuilt archives to Apple Silicon macOS
  (`aarch64-apple-darwin`) while the Gaze recognizer backend portability spike
  tracks Intel macOS, Linux, and Windows binary distribution.

### Fixed
- **Operator-facing UX (security messaging):** the legacy v0.2.x → v0.2.2
  MCP migration prompt in `gaze-lens init` had inverted compliance-isolation
  framing. It claimed that *removing* the legacy per-profile MCP entries
  would *break* compliance isolation — the opposite of the v0.2.2 contract,
  in which compliance isolation is enforced by the mandatory `profile`
  argument on every MCP tool call (SPEC §"MCP server"). Operators picking
  the (former) default `N` silently produced a non-conformant config that
  would fail the next agent invocation with `invalid_params`. The prompt
  is rewritten to surface the SPEC rationale, name the `invalid_params`
  consequence of declining, and the interactive default flips from `N`
  (preserve) to `Y` (migrate) to match `[Y/n]`. Test pin updated with a
  negative assertion against the inverted framing to prevent regression.
  (#518, #519, #520)
- **Security:** `gaze-lens check --explain-risk` now sanitizes the rendered
  profile name against terminal escape sequences. An attacker-controlled
  profile name containing `\x1b[2K` could previously overwrite earlier
  trust-report lines in agent log rendering. The render layer rejects names
  outside `^[a-zA-Z0-9_-]{1,64}$` with a typed validation error. Defense-in-
  depth applied to both `report.profile` and
  `report.process_surface.profile_under_review` so future direct constructors
  of `TrustReport` cannot bypass the guard. Profile-name validation regex
  was extracted to a shared helper to keep the input gate and render gate
  in lockstep. (#439, #512, #513, #514, #515)

### Performance
- `gaze-lens check` no longer builds the Gaze pipeline twice on the
  non-`--explain-risk` path. (#438)

### Documentation
- CONTRIBUTING.md clarifies that Gaze dependency bumps inside an in-flight
  `v0.x.y` release cycle do not require an immediate `gaze-lens` patch
  version bump; the release cut at the end of the cycle rolls them up. For
  patches to an already-shipped release line, the version bump rides with
  the dependency change. (#503)
- CONTRIBUTING.md documents how to regenerate `Cargo.lock` for committable
  state when `~/.cargo/config.toml` has a local `[patch.crates-io]` block for
  Gaze crates — disable the patch before running `cargo update`, then verify
  the lockfile records crates.io sources rather than a path-based resolution.
  (#504)
- `gaze_value_to_json` is annotated as the source-of-truth exhaustiveness
  pin for new `gaze::Value` variants. (#502)

### Internal
- Pre-push hook gained a docs-only fast-path that skips the full
  `fmt + clippy + test` gate when a push touches only documentation files.
  (#18)
- Pre-push hook now skips the cargo gate on delete-only pushes
  (`local_sha == zero`), which previously incurred a 25-30 minute cold-cache
  wait when removing a merged remote branch. (#516)
- Trust-report tests use a proper crypto-rand 32-byte sentinel rather than
  concatenated ULIDs, matching the original plan. (#437)

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
