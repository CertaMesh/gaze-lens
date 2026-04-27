# Contributing to gaze-lens

## Dev workflow

1. Anvil worktree per feature: `anvil create gaze-lens/<topic> --base main`.
2. PR-based; no direct-on-main.
3. `[agent]` prefix on every commit.
4. Test suite green before each commit; fix-and-commit-separately on failure.
5. Stage specific files by name; never `git add -A`.

## Gaze dependency pin

`Cargo.toml` pins `gaze` and `gaze-recognizers` to a specific `PIInuts/gaze` Git revision. Do not silently float to an arbitrary Gaze checkout. When adopting new Gaze features, update the `rev = "..."` SHA in `Cargo.toml` and bump the gaze-lens patch version in the same PR.

Local development can use a local Gaze checkout through a per-developer Cargo patch. Add this to `~/.cargo/config.toml` and do not commit it:

```toml
[patch."https://github.com/PIInuts/gaze.git"]
gaze = { path = "/abs/path/to/Gaze/crates/gaze" }
gaze-recognizers = { path = "/abs/path/to/Gaze/crates/gaze-recognizers" }
```

## Releases

Releases are tag-driven through [cargo-dist](https://opensource.axo.dev/cargo-dist/). To cut a release:

1. Bump the package version in `Cargo.toml`.
2. Commit the version bump.
3. Create a matching SemVer tag, for example `git tag v1.0.1`.
4. Push the tag with `git push origin v1.0.1`.

The GitHub Actions release workflow runs on `v*.*.*` tags, builds the configured macOS, Linux, and Windows archives, generates shell and PowerShell installers, and uploads everything to the GitHub release for that tag. v1.0.0 shipped without binaries; v1.0.1 and later tags will publish them automatically.

The release workflow requires a repository secret named `GAZE_REPO_TOKEN` on `PIInuts/gaze-lens` so Cargo can fetch the private `PIInuts/gaze` dependency. Use a fine-grained PAT scoped to `PIInuts/gaze` with read-only repository access, then add it in GitHub under Settings -> Secrets and variables -> Actions -> New repository secret.

The release workflow configures Git to use that token before cargo-dist resolves dependencies. When changing the required Gaze revision, update the `rev = "..."` SHA in `Cargo.toml`, bump the patch version, and call out the Gaze revision in the release PR description.

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

The 5 SPEC v1 MCP tools (`query`, `schema`, `list_tables`, `log_tail`, `log_grep`) are the locked public surface. Adding a 6th requires a SPEC amendment PR, not an impl PR. Internal helper methods are fine; do not wire them through `frontend::mcp::McpFrontend` without SPEC.

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
