# gaze-lens

PII-safe read-access for live production investigation by AI agents.

`gaze-lens` lets a developer point their LLM agent at a production database or app log during an incident and get back **pseudonymized** results — `<EMAIL_001>` instead of `alice@example.com`. The engineer can later replay the agent's session with real values for review.

Built on the [Gaze](https://github.com/PIInuts) pseudonymization engine. Part of the [PIInuts](https://github.com/PIInuts) product family — every product in the family is named `gaze-X`.

> **Status:** pre-v1. See [SPEC.md](./SPEC.md) for the locked product spec, architecture, anti-features, and roadmap.

## Why

Today, when an engineer wants their AI agent to investigate prod, they have two bad options:

1. Give the agent raw access — and leak names / emails / addresses to the model provider.
2. Give the agent nothing — and waste an hour eyeballing psql while the incident burns.

`gaze-lens` is the third option: **pseudonymized agent access with auditable, reversible token mapping**.

## Surfaces (v1)

- `gaze-lens mcp` — MCP server (stdio). Primary surface. Agent connects, calls tools.
- `gaze-lens query "..."` — CLI for human dry-run.
- `gaze-lens replay <session>` — human-only restore of a recorded agent session.

## Sources (v1)

- **Database** — MySQL, Postgres, SQLite (via sqlx). Read-only.
- **App logs** — file tail / grep over SSH. No server-side install required.

## Reference

`reference/debug-proxy/` is the predecessor crate (extracted from the Gaze monorepo). Used as a mining source during v1 implementation, not part of the active build.
