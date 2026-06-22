# Search app logs

`gaze-lens` exposes two MCP tools for app logs from a configured log profile:
`log_tail` (recent lines) and `log_grep` (search). A log profile can be remote
over SSH (`kind = "ssh_log"`) or a local file path (`kind = "local_log"`). Both
pseudonymize output through Gaze and write a manifest row before anything reaches
the agent. This page covers tailing, the two `log_grep` modes, and when to prefer
each.

For exact tool semantics, see [reference/spec.md](../reference/spec.md)
(§v1 sources → App logs) and [reference/mcp-tools.md](../reference/mcp-tools.md).

## Prerequisites

- A log profile that passes `check` (see [configure-profiles.md](./configure-profiles.md)).
  For a local file:

  ```toml
  [profiles.source]
  kind = "local_log"
  path = "./storage/logs/app.log"
  ```

  For a remote file:

  ```toml
  [profiles.source]
  kind = "ssh_log"
  host = "deploy@app.example.invalid"
  path = "/var/log/app.log"
  ```

- For `ssh_log`, your SSH key loaded and the host pinned in `known_hosts` —
  `gaze-lens` runs SSH with `BatchMode=yes` and strict host-key checking.
- An MCP client wired to the server (see
  [wire-up-mcp-clients.md](./wire-up-mcp-clients.md)). These are agent-facing
  tools; every call carries a `profile` argument.

## Steps

1. **Tail recent lines.** `log_tail` streams the tail of the configured log file,
   redacted:

   ```json
   {"tool": "log_tail", "args": {"profile": "prod", "lines": 200}}
   ```

2. **Search with a regex (default mode).** `log_grep` defaults to `mode: "regex"`,
   preserving the v0.4 behavior:

   ```json
   {"tool": "log_grep", "args": {"profile": "prod", "pattern": "timeout|ECONNRESET", "limit": 100}}
   ```

   Matching lines come back with PII tokenized (`<EMAIL:Addr_1>` etc.).
   On profiles marked `production = true`, regex mode emits a runtime warning
   because it can act as a raw-text presence oracle. The call still runs; default
   mode remains `regex`.

3. **Prefer keyword mode for sensitive or production logs.** Pass `mode: "keyword"`
   to run the match predicate over the *same redacted text the agent sees*:

   ```json
   {"tool": "log_grep", "args": {"profile": "prod", "pattern": "payment failed", "mode": "keyword"}}
   ```

   In keyword mode, `pattern` is whitespace-separated terms (case-insensitive,
   ANDed across all terms, original line order, honors `limit`). Token searches
   must use the complete token minted for the current session, such as
   `<hash:Name_N>`; partial fragments such as `Email_1` return 0 hits by design.
   Any unknown `mode` value fails closed as invalid args.

4. **Refresh the cache when you need fresh lines.** Keyword mode is backed by a
   short-lived in-memory cache over redacted text. Bust it and re-tail the bounded
   log window with `refresh`:

   ```json
   {"tool": "log_grep", "args": {"profile": "prod", "pattern": "payment failed", "mode": "keyword", "refresh": true}}
   ```

## When to prefer keyword over regex

| Use | Mode |
|---|---|
| Sensitive logs, `production`-tier profiles | `mode: "keyword"` |
| Non-sensitive logs, you need regex expressiveness | default `mode: "regex"` |

**Residual risk — regex `log_grep` is a raw-text presence oracle.** In the default
regex mode the predicate runs over the *raw* log text while only displayed lines
are redacted, so a crafted regex can confirm the presence or absence of a raw PII
substring (an email local-part, an account id) that never appears in the tokenized
output — a one-bit-per-query oracle. No raw value is ever returned and the searched
window is still fully manifested, but the boolean match result leaks. Keyword mode
runs its predicate over the redacted text and cannot probe raw values. This is a
documented residual risk preserved for v0.4 compatibility; see
[explanation/threat-model.md](../explanation/threat-model.md) for the discussion
and [reference/spec.md](../reference/spec.md) (§v1 sources → App logs) for the
locked text.

## Done when

- `log_tail` returns redacted recent lines for your profile.
- `log_grep` returns matches, and you use `mode: "keyword"` for any
  sensitive / `production`-tier log search.
