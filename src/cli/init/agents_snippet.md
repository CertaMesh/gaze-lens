## gaze-lens

This project uses [gaze-lens](https://github.com/PIInuts/gaze-lens) for
PII-safe read-only investigation against live production data. AI agents
reach the 6 CLI subcommands (`serve`, `init`, `query`, `replay`, `check`,
`demo`) and the 5 MCP tools (`query`, `schema`, `list_tables`, `log_tail`,
`log_grep`) over stdio.

### Profiles

The configured profiles are: {{PROFILES}}. Configs live in `.gaze-lens.toml`
(project) or `~/.gaze-lens/profiles.toml` (user). Run `gaze-lens check` to
verify connectivity for every profile.

### Calling MCP Tools

Every MCP tool call requires a `profile` argument selecting which configured
source to dispatch. Example:

```json
{"tool": "query", "args": {"profile": "{{FIRST_PROFILE}}", "table": "users", "limit": 5}}
```

Pass `profile` matching one of the configured names listed above. Empty or
unknown profile returns `invalid_params` with the loaded profile list.

### Quickstart

```sh
gaze-lens check
gaze-lens query --profile {{FIRST_PROFILE}} --table users --limit 5
gaze-lens serve
gaze-lens replay <session_ulid>
```

All retrievals route through `Session::dispatch_tool` and pass through the
Gaze redaction pipeline before any value is returned or persisted to the
manifest. Replay is local-only — raw values never leave your machine.
