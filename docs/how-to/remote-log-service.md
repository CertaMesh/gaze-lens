# Run a remote log service

The first-party remote service exposes one bounded `log_tail` operation over a
private TLS MCP transport. A separate local `gaze-lens serve` process reads that
result through the existing Source, Session, Gaze redaction, snapshot and manifest
path before an agent receives anything. The local public MCP interface still has
five tools; the CLI still has six commands.

The service is an explicit mode of `serve`, not a generic MCP gateway. It never
starts a Gaze model, a local manifest, or the agent-facing MCP frontend. It cannot
run commands, accept paths or SQL from callers, discover upstream tools, or proxy
arbitrary MCP servers. Run it under a dedicated OS account with read permission
only for configured log files. Network access to the listener remains an operator
firewall decision.

## Service configuration

Generate an independent random 256-bit credential for each grant, for example
`openssl rand -hex 32`. The credential is exactly 64 hexadecimal characters.
Store the literal value in the client's environment or keyring. Do not put it in
an agent prompt, tool argument, command-line argument, or the service config.
Calculate SHA-256 over the exact ASCII credential, without a trailing newline,
using a trusted local tool; the service stores only that 64-character hex digest.
These instructions do not create credentials automatically.

Obtain a server certificate and matching PKCS#8 PEM private key, with a SAN for
the client's configured server name. Protect the key and grant files with OS
permissions. Certificate rotation requires restarting the service. Keep the
client trust-root bundle current; there is no insecure verification option.

Example `service.toml` (paths and addresses are synthetic):

```toml
listen = "127.0.0.1:9443"
certificate = "/srv/gaze/tls/server.pem"
private_key = "/srv/gaze/tls/key.pem"
grants = "/srv/gaze/grants.json"

[resources]
application = "/srv/app/logs/application.log"
```

The grants file is JSON, with at most 128 grants and 64 KiB total:

```json
{
  "grants": [
    {
      "sha256": "REPLACE_WITH_SHA256_OF_THE_EXACT_CREDENTIAL",
      "enabled": true,
      "expires_unix": 1893456000,
      "operation": "log_tail",
      "resource": "application"
    }
  ]
}
```

The placeholder above is deliberately invalid. Each digest must be unique and
all grant entries must be valid. Missing, malformed, oversized, disabled, expired,
or insufficient grants fail closed. Replace the grants file atomically to revoke
or rotate credentials. It is reopened before each read and again before releasing
the result; restart is not required. The final authorization check is the
revocation linearization point. Revocation cannot recall bytes already released.

Start the service explicitly:

```sh
gaze-lens serve --remote-service-config /srv/gaze/service.toml
```

Certificate, key, configuration, grants and log files must be regular files, not
symlinks, devices, sockets or FIFOs. Files are opened with nonblocking and
no-follow flags, then checked through the opened descriptor. Parent directory
ownership remains an operator responsibility.

## Local privacy proxy

Use a separate configuration and process for remote profiles. Mixing direct and
remote profiles in one `serve` invocation is rejected before model initialization.
A watchdog timeout terminates this whole dedicated proxy; restart creates a fresh
session ID. It does not terminate the remote service.

```toml
[[profiles]]
name = "remote-application"
policy = "/home/operator/gaze/production-policy.toml"
production = true

[profiles.source]
kind = "remote_mcp_log"
endpoint = "127.0.0.1:9443"
server_name = "logs.example.test"
trust_root = "/home/operator/gaze/root-ca.pem"
resource = "application"

[profiles.source.secret]
type = "env"
var = "GAZE_REMOTE_TOKEN"
```

The endpoint is an IP address and port (IPv6 uses `[address]:port`); `server_name`
is the certificate identity and may be a DNS name. No DNS-based endpoint discovery,
redirect, insecure TLS fallback, or upstream policy negotiation occurs. Keyring
references use the existing `type = "keyring"`, `service`, and `account` fields.
Project privacy policy remains authoritative. A user transport override may
change endpoint, identity, trust root and secret reference, but keeps the
project's configured resource.

Configure the MCP client to run:

```sh
gaze-lens --project-config /home/operator/gaze/remote.toml serve --profile remote-application
```

The agent calls the existing `log_tail` with `profile` and optional `lines`.
`log_grep`, database tools and discovery are unsupported for this source; local
`query` rejects it. `check` validates the remote configuration shape; it does not
read remote logs or prove credential validity. Remote profiles require an explicit policy with `default_action = "tokenize"`
or `"redact"`. Every column override must also tokenize or redact: column rules
produce class rules that apply to log text. Preserve, format-preserve and
generalization actions are rejected for this source. This guard checks the same
loaded policy used to build the pipeline. Production NER requirements and
local model availability still apply. Detection remains coverage-dependent: an email-only policy does not recognize
names or arbitrary PII. A minimal synthetic-only policy is:

```toml
[policy]
default_action = "tokenize"
[policy.database]
```

For production also configure `[ner].model_dir`; do not use the synthetic-only
policy as a production configuration.

## Private wire contract

The wire is newline-delimited JSON-RPC 2.0 over TLS, pinned to MCP `2025-11-25`:

1. `initialize`, ID 1, fixed client/server identity, protocol and capabilities.
2. `notifications/initialized` without params.
3. `tools/list`, ID 2, fixed one-tool schema.
4. `tools/call`, ID 3, `log_tail`, exact `resource` and `lines` arguments.
5. Client TLS half-close; service verifies clean closure and rejects extra bytes.
6. Standard MCP result with `content: []`, `isError: false`, and exact
   `structuredContent: {lines: [...], truncated: [...]}`.
7. Service clean TLS closure; client rejects any extra response before Gaze.

Authentication is private transport metadata at
`params._meta["gaze-lens/token"]`, authored by the trusted local client. It is
never a public agent tool argument or a manifest argument. This is not HTTP MCP
OAuth. TLS verifies server identity; the credential grant authorizes the client.

Unknown and duplicate fields at any depth, arbitrary content blocks, instructions,
annotations, errors, server requests, notifications, malformed UTF-8, unexpected
IDs/versions and unclean EOF are rejected. Raw messages never enter rmcp's runtime
or its generic transport parser. The raw modes disable tracing and use a fixed
panic hook that exits silently; `RUST_LOG=trace`, `--log trace` and verbose errors
do not reveal remote content. TLS key logging remains disabled, including when
`SSLKEYLOGFILE` is set. Do not add a custom TLS key logger.

## Bounds and failure behavior

The fixed ceilings are 8 active service connections, no waiting admission queue,
5 seconds for connection/handshake, 10 seconds per frame read, 30 seconds per
complete connection/source operation, 1 MiB per JSON frame, 1 MiB scanned log
bytes, 1,000 lines, 8 KiB per line and 128 KiB total result text. A connection has
three requests and three responses; there is no streaming. Filesystem workers
retain an admission permit until actual completion even when the connection
expires, preventing timed-out reads from creating unbounded background work.
An OS/filesystem stall can occupy a service slot until the syscall completes.
The local client likewise has one shared non-queuing slot for blocking keyring and
trust-root jobs, held by the actual job after caller timeout. Retries reject while
that slot remains occupied. A stuck OS job can delay graceful runtime shutdown;
operator process termination may be needed. No timed-out result is released.

Reads stop at the initial file length. Incomplete first scan-boundary and trailing
lines are dropped, oversized lines are dropped whole, and the total byte cap stops
between lines. Nothing clips UTF-8 or an email into a detector-unrecognizable
fragment. Invalid complete UTF-8 is rejected. Fixed truncation values are `bytes`,
`lines`, and `line_bytes`; the local proxy maps them to existing truncation types.
Concurrent log rewriting is not a filesystem snapshot.

The local proxy admits one request at a time, rejecting contention immediately.
A native thread arms a 90-second monotonic deadline before model startup, and
separately before each request's source, redaction, audit and response admission.
Cancellation does not disarm it. Only explicitly completed, timely work disarms
it. Expiry uses `_exit(124)`, with no stderr write, unwinding, atexit handlers or
abort-generated core dump. Panics take the same silent fail-closed path. OS
scheduling is not a mathematical hard real-time guarantee.

The deadline covers response admission, not delivery acknowledgement. A kill can
truncate an already sanitized frame. An audit `ok` row means redacted and durably
audited, not acknowledged by the agent; pending/error rows and ok-without-delivery
are possible. Timeout messages recovered from the core audit boundary accept only
fixed phase/tool fields and a context-free duration; arbitrary upstream error text
is never promoted into audit metadata. Direct typed timeout diagnostics remain
available to the local caller. There is no second delivery ledger. Kernel IO already submitted can
complete after process death; no surviving in-process privacy worker can continue.

The existing `replay` command restores recorded call arguments; it does not
archive or reconstruct returned log bodies. Retained sanitized output can be
restored locally using its committed snapshot through Gaze. The manifest stores
result summaries, not a second raw log archive.

Raw token mappings remain local trusted data, requiring disk encryption and the
existing 0600 snapshot / 0700 directory permissions. Durable atomic snapshot
replacement must preserve previous successful calls across a kill. Private sibling
temporary snapshot files are additional trusted raw copies and may remain after
an abrupt exit. Do not place snapshots on unencrypted storage.

Tests use generated self-signed loopback certificates, synthetic logs and isolated
manifests. They prove an encrypted connection and server-name/root rejection, not
a production CA chain, operator certificate rotation, or detector perfection.

## Operational changes and follow-ups

Removed failure paths include raw upstream messages entering rmcp logging,
unbounded backward log scans, duplicate JSON-key ambiguity, and snapshot
truncation destroying previously committed replay. The remote policy gate also
rejects configurations that preserve detected PII. Audit timeout strings with
formerly free-form context now become a generic internal audit error rather than
retaining that context; direct typed frontend timeout diagnostics are unchanged.

Retained limits include detector coverage, model availability, trusted local
snapshot storage, operator file permissions, and non-atomic log rewriting. TLS
does not make a policy recognize every kind of PII, and audit success is still not
a delivery acknowledgement.

New operational failures include certificate/name/trust rejection, expired or
revoked grants, immediate busy rejection, and whole-proxy exit on watchdog expiry
or panic. Monitor process exit code 124 and restart with a fresh session. A stuck
filesystem/keyring syscall can retain a bounded work slot or delay graceful
shutdown; replacing the process is the recovery boundary.

Concrete follow-ups are certificate-expiry/rotation monitoring, deployed-filesystem
stall exercises on supported Unix platforms, and explicit protocol compatibility
review before expanding this private one-operation transport. They are not
implemented by this change.
