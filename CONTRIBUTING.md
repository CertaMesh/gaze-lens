# Contributing to gaze-lens

## Dev workflow

1. Anvil worktree per feature: `anvil create gaze-lens/<topic> --base main`.
2. PR-based; no direct-on-main.
3. `[agent]` prefix on every commit.
4. Test suite green before each commit; fix-and-commit-separately on failure.
5. Stage specific files by name; never `git add -A`.

## Local pre-push hook

The repo ships a pre-push hook at `.githooks/pre-push` that runs `cargo fmt --check`, `cargo clippy --all-targets --no-deps -- -D warnings`, and `cargo test --all-targets` before any `git push`. It catches the same regressions the release workflow would catch, locally, without needing GitHub Actions minutes.

Activate it once per clone:

```sh
git config core.hooksPath .githooks
```

The setting is local to the clone (stored in `.git/config`, not committed) so each contributor opts in.

Emergency bypass for WIP-branch backups:

```sh
SKIP_HOOK=1 git push origin my-feature
```

Do not bypass for shared branches or tags; the hook exists to keep main and release tags green.

## Gaze dependency pin

`Cargo.toml` pins `gaze` and `gaze-recognizers` to a `PIInuts/gaze` Git tag with a matching crate version, e.g. `tag = "v0.6.4"` + `version = "0.6.4"`. Do not silently float to an arbitrary Gaze checkout. When adopting new Gaze features, bump both `tag` and `version` together in `Cargo.toml`, run `cargo update -p gaze -p gaze-recognizers` so `Cargo.lock` records the resolved sha, and bump the gaze-lens patch version in the same PR.

During an in-flight v0.x.y release cycle, Gaze dependency bumps inside that cycle do not require an immediate gaze-lens patch version bump; the release cut rolls up the Gaze bump with the rest of the cycle's changes. For patches to an already-shipped release line, bump the gaze-lens patch version in the same PR as the Gaze dependency change.

Local development can use a local Gaze checkout through a per-developer Cargo patch. Add this to `~/.cargo/config.toml` and do not commit it:

```toml
[patch."https://github.com/PIInuts/gaze.git"]
gaze = { path = "/abs/path/to/Gaze/crates/gaze" }
gaze-recognizers = { path = "/abs/path/to/Gaze/crates/gaze-recognizers" }
```

### Regenerating Cargo.lock for committable state

Before running `cargo update -p gaze -p gaze-recognizers` for a commit, temporarily disable any `[patch."https://github.com/PIInuts/gaze.git"]` block in `~/.cargo/config.toml` by commenting it out. After the update, verify the `Cargo.lock` entries for `gaze` and `gaze-recognizers` include `source = "git+https://github.com/PIInuts/gaze.git?tag=vX.Y.Z#<sha>"` instead of a path-based resolution. Re-enable the `[patch]` block after committing for local development.

An optional future hardening step is a CI guard that rejects committable lockfiles with path-based Gaze resolutions.

## Releases

Releases are tag-driven through [cargo-dist](https://opensource.axo.dev/cargo-dist/). To cut a release:

1. Bump the package version in `Cargo.toml`.
2. Commit the version bump.
3. Create a matching SemVer tag, for example `git tag v0.1.1`.
4. Push the tag with `git push origin v0.1.1`.

The GitHub Actions release workflow runs on `v*.*.*` tags, builds the configured macOS, Linux, and Windows archives, generates shell and PowerShell installers, and uploads everything to the GitHub release for that tag. v0.1.0 ships without binaries; v0.1.1 and later tags will publish them automatically.

The release workflow requires a repository secret named `GAZE_REPO_TOKEN` on `PIInuts/gaze-lens` so Cargo can fetch the private `PIInuts/gaze` dependency. Use a fine-grained PAT scoped to `PIInuts/gaze` with read-only repository access, then add it in GitHub under Settings -> Secrets and variables -> Actions -> New repository secret.

The release workflow configures Git to use that token before cargo-dist resolves dependencies. When changing the required Gaze revision, bump the `tag = "..."` + `version = "..."` pair in `Cargo.toml`, refresh `Cargo.lock`, bump the patch version, and call out the Gaze tag in the release PR description.

## sqlx macro policy (banned for production-source queries)

**Do not use `sqlx::query!`, `sqlx::query_as!`, or any compile-time query macro for production-source adapters** (MySQL, Postgres, SQLite reading from operator profiles).

Reasons:
- Each backend's offline metadata cache is per-DB-version per-CI-pipeline; cost scales with the matrix.
- Reference adapter at `reference/debug-proxy/src/adapter/mysql.rs:51,114,125,143` already uses dynamic `sqlx::query`/`query_as`; that pattern is the production path.

Exceptions:
- Internal manifest schema queries against gaze-lens's own SQLite may use compile-time macros, since the DB schema is owned and stable.

## PR review routing

(Per orchestrator-mode skill.)

| PR class | Reviewer |
|---|---|
| Code (small-medium scope) | Codex |
| Spec/prose-heavy code | Claude |
| Docs-only (`*.md`) | None — orchestrator squash-merges after sanity read |
| High-stakes (release / security / multi-module) | Dual: Claude + Codex |

## Public-surface expansion rule

The 5 SPEC v1 MCP tools (`query`, `schema`, `list_tables`, `log_tail`, `log_grep`) are the locked public surface. Each accepts a required `profile` argument in v0.2.2; argument-schema growth is allowed under the locked tool list, but adding a 6th tool requires a SPEC amendment PR, not an impl PR. Internal helper methods are fine; do not wire them through `frontend::mcp::McpFrontend` without SPEC.

## MySQL integration tests

The standard `cargo test --all-targets` run must not require Docker or a local database. MySQL end-to-end tests belong behind the opt-in `integration-mysql` feature and should be run explicitly in environments that provide a MySQL testcontainer or compatible local service.

## Postgres integration tests

The standard `cargo test --all-targets` run must not require Docker or a local database. Postgres end-to-end tests belong behind the opt-in `integration-postgres` feature and should be run explicitly in environments that provide a Postgres testcontainer or compatible local service. SQLite target-source tests use a temp-file database and run in the standard suite.

Run the opt-in Postgres smoke with:

```sh
cargo test --all-targets --features integration-postgres
```

Default CI does not run this feature in v1; enabling it in CI is a deferred decision.

## Runtime policy fallback

`serve` loads `profile.policy` when a profile names a policy TOML. Profiles without an explicit policy use the built-in fallback equivalent to an empty `[policy.database]` section, which preserves non-sensitive fields and still enables the default email detector.

## Snapshot retention policy

v0.2 introduces two opt-in profile fields governing snapshot lifecycle. Both default to v0.1 behavior (unlimited retention, manifest as the audit log of record per D3) when unset.

Profile fields (TOML, profile-layer — NOT under `[session]`, which would collide with Gaze `ttl_secs`):

```toml
snapshot_retention_days = 30   # Option<u32>; None / unset = unlimited
auto_purge = false             # bool; false = warn-only, true = silent purge + tombstone
```

**Merge rule (destructive-default default-deny).** When a project profile and a user profile are merged, the resolved `auto_purge` is

```text
merged_auto_purge = project.auto_purge && user.auto_purge
```

Plain conjunction. If the project file does not enable `auto_purge`, the user file CANNOT override to `true`. If the project file enables `auto_purge`, the user file MAY downgrade to warn-only by setting `auto_purge = false`. There is no third "user consent" field; rev 2 r2-patch-1 corrected an earlier draft that referenced one. Rationale: `auto_purge` is a destructive operational policy and consent must be expressed at the team-shared (project) layer.

`snapshot_retention_days` itself uses standard user-overrides-project merge; the conjunction rule applies only to `auto_purge`.

**Per-day friction-warning suppression.** When `auto_purge = false` and the sweep finds expired snapshots, gaze-lens emits a stderr listing of what would be purged. To avoid warning fatigue on one-shot `query` invocations, gaze-lens touches `~/.gaze-lens/.warned-YYYY-MM-DD-<profile>` on the first emission per day per profile and suppresses subsequent stderr text for the same day. A `tracing` debug event is still emitted on every sweep so operators can observe activity from logs. `auto_purge = true` info-level emissions are NOT suppressed (cheap and informational).

**v0.1 posture preserved.** Profiles with neither field set produce zero sweep activity. CLI builders skip `ManifestMaintenance::open` entirely; no manifest IO, no FS scan, no warning text. v0.1.x manifests open under v0.2 binaries via the `user_version = 2 → 3` migration described in ARCHITECTURE.md §Manifest schema versioning.

See ARCHITECTURE.md §ManifestMaintenance for the implementation contract and SPEC.md §Snapshot retention (v0.2 opt-in) for the surface contract.

## rmcp version pin

`Cargo.toml` declares `rmcp = { version = "0.2", features = [...] }`. This Cargo SemVer requirement already permits any `0.2.y` patch release without lockfile churn. Do NOT tighten this to `=0.2.x`, `~0.2`, or `^0.2.x`:

- For `0.x.y` versions Cargo treats `"0.2"`, `"^0.2"`, and `"~0.2"` as equivalent — they all resolve to `>=0.2.0, <0.3.0` and pick the highest matching publish.
- `=0.2.x` would force a literal pin, lose patch-level fixes, and create review churn every time rmcp ships a security or compatibility patch.

If a future rmcp 0.3 is required, bump the requirement explicitly in a dedicated PR. The local pre-push gate will catch any compile breakage at that point.
