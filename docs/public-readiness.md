# Public Release Readiness

This checklist tracks the repository state needed before treating `gaze-lens` as a public open-source project. It is a release gate, not a claim that the repository is currently public.

## Repository Basics

- [ ] Repository visibility is intentionally set for the release milestone.
- [ ] `README.md` describes the current product status without implying broader platform support than exists.
- [ ] `LICENSE` is present and matches the `Apache-2.0` package metadata in `Cargo.toml`.
- [ ] `SECURITY.md` gives a private vulnerability reporting path.
- [ ] `CONTRIBUTING.md` documents local and pull-request validation.
- [ ] `SPEC.md` remains the source of truth for the locked public surface.

## CI And Release

- [ ] Public pull requests run `cargo fmt --check`.
- [ ] Public pull requests run `cargo clippy --all-targets --no-deps -- -D warnings`.
- [ ] Public pull requests run `cargo test --all-targets`.
- [ ] Default CI does not require Docker, a live database, or production credentials.
- [ ] Release automation is documented as Apple Silicon macOS-only until additional targets are proven in CI.
- [ ] Tag-driven release flow has been tested on a non-production dry run or reviewed against the generated `cargo-dist` workflow.

## Security And Privacy

- [ ] No checked-in profiles, logs, snapshots, manifests, tokens, or operator secrets.
- [ ] The README points users to the threat model before production use.
- [ ] Snapshot storage assumptions are explicit: `0700` directory, `0600` files, local disk encryption required.
- [ ] Raw SQL remains out of the v1 public surface.
- [ ] Schema tokenization behavior is documented as raw-by-default unless enabled by profile.

## Publication

- [ ] Maintainers have reviewed open issues and PRs for private customer, incident, or credential details.
- [ ] GitHub repository description, topics, and homepage are accurate.
- [ ] Branch protection requires the public CI workflow before merging to `main`.
- [ ] Release notes call out supported binary targets and source-build expectations.
