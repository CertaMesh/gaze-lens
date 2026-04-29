## gaze-lens

This project uses [gaze-lens](https://github.com/PIInuts/gaze-lens) for PII-safe
read-only investigation against live production data. AI agents reach the
6 CLI subcommands (`serve`, `init`, `query`, `replay`, `check`, `demo`) and the
5 MCP tools (`query`, `schema`, `list_tables`, `log_tail`, `log_grep`) over
stdio.

### Profile

The active profile is `{{PROFILE}}`. Find its config in `.gaze-lens.toml`
(project) or `~/.gaze-lens/profiles.toml` (user).

### Quickstart

```sh
gaze-lens check --profile {{PROFILE}}     # verify config + connectivity
gaze-lens query --profile {{PROFILE}} --table users --limit 5
gaze-lens serve --profile {{PROFILE}}     # MCP stdio server for agents
gaze-lens replay <session_ulid>           # reverse the redaction locally
```

All retrievals route through `Session::dispatch_tool` and pass through the
Gaze redaction pipeline before any value is returned or persisted to the
manifest. Replay is local-only — raw values never leave your machine.
