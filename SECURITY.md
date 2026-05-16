# Security Policy

## Reporting a Vulnerability

Please do not open a public GitHub issue for suspected vulnerabilities.

Report security issues through GitHub's private vulnerability reporting for this repository when available. If that is unavailable to you, email the maintainers at `security@empiretwo.com` with:

- Affected version or commit.
- A concise description of the issue and expected impact.
- Reproduction steps or a proof of concept, if you can share one safely.
- Whether the report includes sensitive production data.

We aim to acknowledge new reports within 3 business days. Fix timelines depend on severity and release risk; we will coordinate disclosure before publishing details.

## Scope

Security-sensitive areas include:

- Raw production values reaching an agent, manifest row, trace, error, or other output before Gaze redaction.
- Snapshot files under `~/.gaze-lens/snapshots/` being created with weaker than `0600` file permissions or outside a `0700` directory.
- SQL or SSH command injection in configured data sources.
- Bypasses around the locked MCP tool surface or structured-query-only policy.
- Secrets written to repo files, logs, CI output, or generated agent configuration.

## Supported Versions

`gaze-lens` is pre-1.0. Security fixes target the current `main` branch and the most recent tagged release when a patch release is practical.

## Operator Assumptions

The v1 threat model assumes operators run local disk encryption such as FileVault or LUKS. `gaze-lens` does not currently implement per-snapshot encryption at rest.
