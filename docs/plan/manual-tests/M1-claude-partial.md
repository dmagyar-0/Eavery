# M1-T05 (partial): the Claude adapter, not signed in

**Date:** 2026-09-16
**OS:** Linux 6.18 (x86_64), Rust 1.94.1
**Engine:** `claude` — `npx -y @agentclientprotocol/claude-agent-acp`,
adapter version 0.78.0, Node 22.
**Status:** **partial.** No Claude Code login was available on this machine, so
everything M1-T05 asks about the plan phase — the plan mode id, the permission
option kinds, how `ExitPlanMode` arrives — is still open. M1-T05 stays
unticked. What is recorded here is the handshake, which does not need a login,
and how the adapter behaves when there is none.

## How it was run

```
cargo run -p eavery-cli -- engines --engine claude
EAVERY_LOG=trace cargo run -p eavery-cli -- engines --engine claude
```

The engine was found by `eavery-engines::discovery` through `npx`, with no
`claude` CLI on the machine — which is itself worth noting: the adapter starts
perfectly well without the CLI it drives.

## `initialize` response (verbatim fields of interest)

```json
{"protocolVersion":1,
 "agentInfo":{"name":"@agentclientprotocol/claude-agent-acp",
              "title":"Claude Agent","version":"0.78.0"},
 "authMethods":[],
 "agentCapabilities":{
   "loadSession":true,
   "promptCapabilities":{"image":true,"embeddedContext":true},
   "mcpCapabilities":{"http":true,"sse":true},
   "sessionCapabilities":{"additionalDirectories":{},"close":{},"delete":{},
                          "fork":{},"list":{},"resume":{},"subagents":{}},
   "_meta":{"claudeCode":{"promptQueueing":true},"authStatus":{}}}}
```

So, against `04-acp-engines.md` §1:

- Protocol version 1, as the plan assumes. The handshake Eavery sends is
  accepted as written.
- `loadSession: true`, so the M7-T05 durability path can use `session/load`
  for this engine rather than the summary-prepend fallback.
- `promptCapabilities` includes `embeddedContext`, which M6 will want.
- `mcpCapabilities` advertises `http` and `sse`. Whether `mcpServers` passed in
  `session/new` are actually loaded is still M1-T05's question; it could not be
  answered without a login.

## The finding that matters: `authMethods` is empty when not signed in

`04-acp-engines.md` §9 step 3 says to report "needs sign-in" when
`authMethods` is non-empty and `session/new` returns an auth error. The Claude
adapter does neither half:

- `authMethods` is `[]` whether or not the user is signed in.
- `session/new` fails with a plain `-32603 Internal error`, carrying nothing
  that names authentication.
- The sign-in state arrives instead as a **notification on a vendor-extension
  method**, sent unprompted after `session/new`:

  ```json
  {"jsonrpc":"2.0","method":"_auth/status_update",
   "params":{"authStatus":{"kind":"none","label":"Not logged in"}}}
  ```

  `_`-prefixed methods are ACP's extension space, so this is legal and
  adapter-specific. `agentCapabilities._meta.claudeCode` and
  `agentCapabilities.auth.logout` in the initialize response are the same
  extension advertising itself.

The consequence today is that `eavery-cli engines` reports the adapter as
`unavailable` with the reason `could not open a session: claude refused
session/new: Internal error (code -32603)`, where the honest answer is "you
are not signed in". The developer-facing reason is at least printed.

Nothing was built on `_auth/status_update` here, deliberately: a rule derived
from the not-signed-in case alone cannot be checked against the signed-in one,
and `eavery-acp` is meant to hold no engine-specific knowledge. M1-T05 should
decide it with both states in front of it. The two options look like:

1. Have `AcpEngine` record the last `_auth/status_update` it saw and let the
   health check consult it — a small, contained exception to "this layer knows
   nothing about particular engines", justified by the plan's §9 step 3 being
   unimplementable for this engine otherwise.
2. Treat a `session/new` failure on an engine with a `sign_in_command` as
   "needs sign-in" when nothing better is known. Simpler, and wrong whenever
   the real cause is something else.

## Anything surprising

The adapter exits with SIGKILL when Eavery shuts the connection mid-request
rather than closing cleanly — visible as `the engine exited with signal: 9` in
the log. That is Eavery's own `shutdown` killing it after the failed
`session/new`, not a crash of the adapter, and it is what `kill_on_drop` plus
an explicit shutdown are for. Worth re-checking in M1-T05 that a *clean*
shutdown of a working session does not look the same.
