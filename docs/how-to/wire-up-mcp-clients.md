# Wire up MCP clients

`gaze-lens serve` is a stdio MCP server. One server process exposes all configured
profiles and surfaces exactly five tools: `query`, `schema`, `list_tables`,
`log_tail`, `log_grep`. This recipe wires it into Claude Code, Codex, or Cursor.

The fastest path is `gaze-lens init`, which writes the right config for you. The
manual configs below are small if you prefer to hand-edit. For the tool arguments
themselves, see [reference/mcp-tools.md](../reference/mcp-tools.md); for CLI flags,
see [reference/cli.md](../reference/cli.md).

## Prerequisites

- A `gaze-lens` binary on `PATH`.
- At least one profile that passes `check` (see
  [configure-profiles.md](./configure-profiles.md)).

## Steps

1. **Claude Code — `.mcp.json`.** `init` offers to write this; or add one server
   entry by hand:

   ```jsonc
   {
     "mcpServers": {
       "gaze-lens": {
         "command": "gaze-lens",
         "args": ["serve"]
       }
     }
   }
   ```

2. **Codex — `~/.codex/config.toml`.** Codex config is written only when you ask
   for it explicitly:

   ```sh
   gaze-lens init --profile prod --client codex
   ```

3. **Cursor — `.cursor/mcp.json`.** Likewise opt-in:

   ```sh
   gaze-lens init --profile prod --client cursor
   ```

   `--client` is repeatable, so `init --client codex --client cursor` writes both.

4. **Pass `profile` on every tool call.** There is no default profile. Each call
   selects a configured profile by name; an empty or unknown profile is rejected as
   MCP `InvalidParams` with the loaded profile list in the error:

   ```json
   {"tool": "query", "args": {"profile": "prod", "table": "users", "limit": 5}}
   ```

   Calling a DB tool with a log profile (or a log tool with a DB profile) returns a
   structured profile-class mismatch error.

5. **(Optional) Restrict which profiles a server exposes.** By default `serve`
   loads every configured profile. Pass `--profile` (repeatable) to expose only a
   subset:

   ```sh
   gaze-lens serve --profile prod --profile staging
   ```

   The tool schemas still require the `profile` argument on every call. A legacy
   single-profile `serve --profile prod` entry remains valid as a one-element
   restrict-list.

## Notes

- The server eagerly parses TOML, validates profile and policy files, and builds
  Gaze pipelines at startup; source connections (DB pools, SSH validation) are lazy
  on first call and cached for the process lifetime. **Restart the server after
  editing a profile** — reload requires a restart.
- For how every result is forced through redaction before it reaches the agent, see
  [explanation/pseudonymization-and-replay.md](../explanation/pseudonymization-and-replay.md).

## Done when

- Your agent lists exactly the five tools and nothing else.
- A `query` call with a valid `profile` returns tokenized rows (`<EMAIL:Addr_1>`
  rather than a raw address).
- A call with a missing or unknown `profile` is rejected with the loaded-profile
  list.
