# 04 — ACP Client and Engines

Everything the implementer needs to talk to an engine. Protocol facts below
were verified against https://agentclientprotocol.com in September 2026.

## 1. ACP in one page

- Transport: the client spawns the agent as a child process and exchanges
  **newline-delimited JSON-RPC 2.0** messages over the agent's stdin/stdout.
  stderr is for logs. Never write anything else to the agent's stdin.
- Property keys are `camelCase`; discriminator values are `snake_case`.
- All file paths in the protocol are absolute. Line numbers are 1-based.
- Protocol version to request: `1`.

### Methods the client calls on the agent

| Method | Kind | Purpose |
|---|---|---|
| `initialize` | request | negotiate `protocolVersion`, exchange capabilities |
| `authenticate` | request | only if `initialize` returned `authMethods` and the agent demands it |
| `session/new` | request | create a session for a `cwd`, passing `mcpServers` |
| `session/load` | request | resume a session by id (only if `agentCapabilities.loadSession`) |
| `session/prompt` | request | send a user turn; returns when the turn ends |
| `session/set_mode` | request | switch agent mode (e.g. plan mode) |
| `session/cancel` | notification | interrupt the current turn |

### Methods the agent calls on the client

| Method | Kind | Eavery behaviour |
|---|---|---|
| `session/request_permission` | request | answered by the plan gate or policy handler |
| `session/update` | notification | mapped to `RawAgentEvent` |
| `fs/read_text_file` | request | serve from disk from any path the engine could read itself (D15). Playbooks live outside the Project and the engine is told to read them. Log every read outside the Project at `debug`. |
| `fs/write_text_file` | request | refuse during Planning; during Executing allow only inside Project |
| `terminal/*` | request | v1 advertises `terminal: false`; engines then use their own shell tooling |

### Wire shapes (exact)

`initialize`:
```json
{"jsonrpc":"2.0","id":0,"method":"initialize","params":{
  "protocolVersion":1,
  "clientCapabilities":{"fs":{"readTextFile":true,"writeTextFile":true},"terminal":false}}}
```
The `clientCapabilities` shape (`fs.readTextFile`, `fs.writeTextFile`,
`terminal`) must be confirmed against the pinned `agent-client-protocol-schema`
types (`ClientCapabilities`, `FileSystemCapability`) on M0-T06; use the SDK's
builder types rather than hand-writing this JSON when using the SDK.

Response fields of interest: `protocolVersion`, `agentCapabilities.loadSession`,
`agentCapabilities.promptCapabilities`, `agentCapabilities.mcpCapabilities`,
`authMethods` (array; empty means no auth step needed), `agentInfo`.

`session/new`:
```json
{"jsonrpc":"2.0","id":1,"method":"session/new","params":{
  "cwd":"/abs/path/to/project",
  "mcpServers":[
    {"name":"eavery-docs","command":"/abs/path/eavery-docs-mcp","args":[],"env":[]},
    {"type":"http","name":"remote","url":"https://example/mcp","headers":[]}
  ]}}
```
Response: `{"sessionId":"...", "modes": {"currentModeId": "...", "availableModes": [{"id":"...","name":"...","description":"..."}]}}` — `modes` is optional.

`session/prompt`:
```json
{"jsonrpc":"2.0","id":2,"method":"session/prompt","params":{
  "sessionId":"sess_x","prompt":[{"type":"text","text":"..."}]}}
```
Response: `{"stopReason": "end_turn" | "max_tokens" | "max_turn_requests" | "refusal" | "cancelled"}`.

`session/update` notification, `params.update.sessionUpdate` is one of:
`agent_message_chunk`, `user_message_chunk`, `agent_thought_chunk`
(each with `content: {type:"text",text}` or other content block),
`tool_call`, `tool_call_update`, `plan` (`entries[]: {content, priority, status}`),
`available_commands_update`, `current_mode_update`, `usage_update`.

`tool_call`:
```json
{"sessionUpdate":"tool_call","toolCallId":"c1","title":"Edit report.md",
 "kind":"read|edit|delete|move|search|execute|think|fetch|other",
 "status":"pending|in_progress|completed|failed",
 "content":[{"type":"content","content":{"type":"text","text":"..."}},
            {"type":"diff","path":"/abs/file","oldText":"...","newText":"..."},
            {"type":"terminal","terminalId":"t1"}],
 "locations":[{"path":"/abs/file","line":12}],
 "rawInput":{}, "rawOutput":{}}
```
`tool_call_update` carries the same fields, all optional except `toolCallId`.

`session/request_permission` (agent → client):
```json
{"jsonrpc":"2.0","id":7,"method":"session/request_permission","params":{
  "sessionId":"sess_x",
  "toolCall":{"toolCallId":"c1","title":"...","kind":"edit","locations":[...]},
  "options":[
    {"optionId":"allow","name":"Allow","kind":"allow_once"},
    {"optionId":"allow_always","name":"Always allow","kind":"allow_always"},
    {"optionId":"reject","name":"Reject","kind":"reject_once"}]}}
```
Client response: `{"outcome":{"outcome":"selected","optionId":"allow"}}` or
`{"outcome":{"outcome":"cancelled"}}`. Eavery picks the option whose `kind`
matches its decision; if the wanted kind is missing, pick by this preference:
AllowAlways→allow_once, RejectAlways→reject_once, anything else→cancelled.

`session/cancel`: `{"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"sess_x"}}`.
After cancel the outstanding `session/prompt` must still return (with
`stopReason: cancelled`). Wait for it; do not send another prompt before it returns.

## 2. Engine table

`eavery-engines` holds one `EngineSpec` per engine. Data, not code.

```rust
pub struct EngineSpec {
    pub id: &'static str,             // "claude" | "codex" | "gemini" | "goose" | "goose-local" | "fake"
    pub display_name: &'static str,
    pub auth_kind: AuthKind,          // OwnLogin | ApiKey | Local | None
    pub launch: LaunchSpec,           // how to find and run it
    pub asking_mode_hint: Option<&'static str>, // mode id substring for the execute phase (asks before acting)
    pub plan_mode_hint: Option<&'static str>,   // mode id substring for the plan phase (most restrictive: Claude plan mode, Codex read-only)
    pub plan_exit_signatures: &'static [&'static str], // tool titles / rawInput markers that mean "leave plan mode" (e.g. "ExitPlanMode"); the plan gate rejects them
    pub source: EngineSource,                   // Bundled | Download { .. } | UserInstalled (08-onboarding-packaging.md §4)
    pub needs_node: bool,                       // true for claude and gemini adapters
    pub vendor: &'static str,                   // "OpenAI", "Anthropic", "Google", "local"; shown on the plan card
    pub sign_in_instructions: &'static str,     // shown in onboarding
}
```

## 3. Engine launch matrix (verified September 2026)

| id | Command | Auth | Notes |
|---|---|---|---|
| `claude` | `npx -y @agentclientprotocol/claude-agent-acp` (or the globally installed `claude-agent-acp`) | User's Claude Code login. The adapter uses the Claude Agent SDK, which uses the Claude Code CLI's own login. | Requires Node 18+ (Eavery cannot download this one; if Node is absent, say so and offer Codex). The package was previously `@zed-industries/claude-code-acp` (deprecated; do not use). `plan_mode_hint` = its plan mode id (record in M1-T05). **Leaving plan mode is a tool call (`ExitPlanMode`) that arrives as a permission request, probably with kind `other`; the plan gate must reject it** (`06` §2.2). Subscription billing for ACP use was scheduled to change 15 June 2026 and paused 16 June 2026; assume it returns. |
| `codex` | `codex-acp` binary from npm `@agentclientprotocol/codex-acp` (1.8.0, ships platform binaries as optional dependencies; the older `@zed-industries/codex-acp` is **deprecated**, do not use). Codex CLI itself from `openai/codex` GitHub releases (native binaries per platform). **Eavery downloads both** on the zero-key path (`08-onboarding-packaging.md` §4) and spawns `codex login` from the app. | `codex login` (ChatGPT account) or `OPENAI_API_KEY` / `CODEX_API_KEY`. Credentials in `~/.codex/auth.json` or OS keychain; Eavery never reads them. | No Node needed. Primary zero-key engine. Modes are read-only / workspace-write / full-access style, not "plan": `plan_mode_hint` = the read-only mode (OS-sandboxed, the strongest plan-phase guarantee available); `asking_mode_hint` = workspace-write with approval on request. Verify exact ids in M1-T06. |
| `gemini` | `gemini --experimental-acp` | Google sign-in done in Gemini CLI. | Without the flag it starts interactive mode and hangs. Known to be flaky across Gemini CLI versions; pin the tested version in `sign_in_instructions`. |
| `goose` | `goose acp` | Provider and model from `~/.config/goose/config.yaml` or env `GOOSE_PROVIDER`, `GOOSE_MODEL`, plus the provider key env (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GOOGLE_API_KEY`, `OPENROUTER_API_KEY`). | Eavery downloads the goose release binary for the platform on first use (Apache 2.0). Eavery passes provider/model/key via env for the child only, never written to goose's config. Also `--enable-scheduler` exists; do not pass it. |
| `goose-local` | `goose acp` with `GOOSE_PROVIDER=ollama`, `GOOSE_MODEL=<model>`, `OLLAMA_HOST=http://localhost:11434` | None. | Health check first probes `GET http://localhost:11434/api/tags` and lists models. |
| `fake` | `eavery-fake-agent --script <path>` | None | Tests only. Hidden in release builds unless `EAVERY_DEV=1`. |

Discovery order for each command: explicit path in Settings → PATH after the
login-shell fix (`02-challenges.md` C5) → well-known locations
(`08-onboarding-packaging.md` §2). On Windows, `npx` resolves to `npx.cmd` and
must be launched via `cmd /C npx ...`; `codex-acp` and `goose` are plain `.exe`.

goose loads MCP servers passed in `session/new` (`mcpServers`) alongside its own
extensions. The Claude adapter passes client MCP servers through. Codex and
Gemini: verify in M1 and record in `CHANGELOG-plan.md`; if an engine ignores
`mcpServers`, Eavery must instead write the engine's own MCP config, preferring
the per-project file where one exists (Codex: `<project>/.codex/config.toml`
`[mcp_servers.*]`; Gemini: `<project>/.gemini/settings.json` `mcpServers`)
over the user-global one, after asking the user, and the consent copy must say
that a global file also affects their terminal use of that engine. Those
folders are in the Journal exclude list.

**goose as a single front door.** goose ships its own ACP providers
(`claude-acp`, `codex-acp`) that wrap the same adapters above and use the same
subscriptions. Driving only goose, with its provider set to one of those, would
collapse this matrix to one row. M1-T09 evaluates it for one day and records
the verdict; the trade is an extra layer with its own mode and permission
quirks.

## 4. Launching a child process (Rust)

```rust
use tokio::process::Command;
use std::process::Stdio;

let mut cmd = Command::new(&resolved_exe);
cmd.args(&spec.args)
   .envs(extra_env)                // provider/model/key for goose only
   .current_dir(project_root)
   .stdin(Stdio::piped())
   .stdout(Stdio::piped())
   .stderr(Stdio::piped())
   .kill_on_drop(true);
#[cfg(windows)]
{ use std::os::windows::process::CommandExt; cmd.creation_flags(0x08000000); } // CREATE_NO_WINDOW
let mut child = cmd.spawn()?;
let stdin  = child.stdin.take().unwrap();
let stdout = child.stdout.take().unwrap();
let stderr = child.stderr.take().unwrap();
// spawn a task that reads stderr line by line into a ring buffer of 200 lines (for crash reports)
```

## 5. Using the `agent-client-protocol` 2.x SDK (verified example)

The SDK's own client example, copied from the repository
(`src/agent-client-protocol/examples/yolo_one_shot_client.rs`). Start from
this. It compiles against 2.0.0.

```rust
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, NewSessionRequest, PromptRequest, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome,
    SessionNotification, TextContent,
};
use agent_client_protocol::{AcpAgent, Agent, ConnectionTo};
use std::path::PathBuf;
use std::str::FromStr;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // AcpAgent parses "cmd arg arg" and spawns it; it implements ConnectTo (a transport).
    let agent = AcpAgent::from_str("goose acp")?;

    agent_client_protocol::Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                println!("{:?}", notification.update);   // -> map to RawAgentEvent
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _connection| {
                let option_id = request.options.first().map(|opt| opt.option_id.clone());
                if let Some(id) = option_id {
                    responder.respond(RequestPermissionResponse::new(
                        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id)),
                    ))
                } else {
                    responder.respond(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled))
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, |connection: ConnectionTo<Agent>| async move {
            let init = connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            eprintln!("agent: {:?}", init.agent_info);

            let new_session = connection
                .send_request(NewSessionRequest::new(PathBuf::from("/abs/project")))
                .block_task()
                .await?;
            let session_id = new_session.session_id;

            let resp = connection
                .send_request(PromptRequest::new(
                    session_id.clone(),
                    vec![ContentBlock::Text(TextContent::new("hello".to_string()))],
                ))
                .block_task()
                .await?;
            eprintln!("stop: {:?}", resp.stop_reason);
            Ok(())
        })
        .await?;
    Ok(())
}
```

How Eavery adapts it:

- `AcpAgent::from_str` is fine for M0. For real engines you need env vars, a
  cwd, and access to stderr; build the child yourself (§4) and construct the
  SDK transport from the child's stdin/stdout. Look in the crate docs for the
  transport types (`Stdio`, `ByteStreams`, `Lines`) and for how `AcpAgent`
  builds its own; copy that. If that takes more than a day, use §7.
- The permission handler must not decide inline. It sends a `PermissionView`
  to core over a channel and awaits a oneshot `Decision`, then responds.
  Keep a timeout (10 minutes) after which it responds `cancelled`.
- `connect_with`'s closure is the lifetime of the connection. Eavery keeps it
  alive for the whole session and drives it with an mpsc command channel
  (`Prompt`, `SetMode`, `Cancel`, `Shutdown`) received inside the closure.
- Add `fs/read_text_file` and `fs/write_text_file` handlers with
  `on_receive_request` in the same way as the permission handler. Types are in
  `schema::v1` (`ReadTextFileRequest`, `WriteTextFileRequest`, and their responses).
- Everything is `Send`-friendly async in 2.x; use the multi-thread tokio runtime.

## 6. Mapping ACP to `RawAgentEvent`

| ACP | RawAgentEvent |
|---|---|
| `agent_message_chunk` text | `Text(String)` |
| `agent_thought_chunk` text | `Thought(String)` |
| `user_message_chunk` | ignore (it echoes our prompt) unless replaying `session/load` |
| `tool_call` | `ToolCall { id, title, kind, status, locations, diff_paths, raw_input }` |
| `tool_call_update` | `ToolCallUpdate { id, status?, title?, locations?, content? }` |
| `plan` | `PlanEntries(Vec<{content, priority, status}>)` |
| `current_mode_update` | `ModeChanged(String)` |
| `available_commands_update`, `usage_update` | `Other(serde_json::Value)` (logged, not shown) |
| non-text content blocks (image, resource) | `Other` in v1 |

Unknown `sessionUpdate` values must not crash the client: deserialise into a
`serde_json::Value` first, match on `sessionUpdate`, and fall back to `Other`.

## 7. Fallback: hand-rolled JSON-RPC client

If the SDK blocks progress, implement this instead. It is about 400 lines.

1. Spawn as in §4. Wrap stdout in `BufReader` and read lines; each non-empty
   line is one JSON-RPC message. Wrap stdin in a `Mutex<ChildStdin>` and write
   one message per line followed by `\n`, then flush.
2. Outgoing requests: `id` from an `AtomicU64`; store a `oneshot::Sender<Value>`
   in a `HashMap<u64, _>`. Responses (`{"id":..,"result":..}` or `{"id":..,"error":..}`)
   resolve the sender.
3. Incoming messages with `method` and `id` are requests from the agent
   (`session/request_permission`, `fs/*`). Dispatch to handlers; write the
   response with the same `id`. Do this on a separate task so a slow permission
   answer never blocks reading further notifications.
4. Incoming messages with `method` and no `id` are notifications
   (`session/update`). Map to `RawAgentEvent`.
5. Use `agent-client-protocol-schema` for types if convenient; otherwise
   define minimal serde structs matching §1. Unknown fields must be ignored
   (`#[serde(default)]`, no `deny_unknown_fields`).
6. Errors: a JSON-RPC error object is `{code, message, data?}`. Standard codes:
   -32700 parse, -32600 invalid request, -32601 method not found, -32602 invalid
   params, -32603 internal. Answer unknown agent→client methods with -32601.

## 8. Prompt text sent to engines

Keep prompts in `crates/eavery-core/src/prompts/*.md` and load with
`include_str!`. Two files: `plan.md` and `execute.md`. Both are rendered with a
tiny template (`{{request}}`, `{{plan}}`, `{{playbooks}}`, `{{project_root}}`).
See `06-plan-gate-permissions.md` §4 for the content.

## 9. Health check

`run_health_check(engine, deep)`:
1. Resolve executable (report "not installed" with the paths searched).
2. Spawn, `initialize` with 15 s timeout (report "did not respond").
3. If `authMethods` is non-empty and the engine returns an auth error on
   `session/new`, report "needs sign-in" with `sign_in_instructions`.
4. `session/new` in a temp directory. **Stop here by default** (`deep =
   false`): this establishes installed, responding, and signed-in without
   spending the user's subscription or waiting on a model. Only when `deep =
   true` (first run for an engine, "Check again" in Settings, or a new engine
   version): `session/prompt` "Reply with the single word OK." with a 60 s
   timeout, then `session/cancel` if still running.
5. Report `Ready { agent_info, load_session: bool, modes }`; cache for 10 minutes.
   Never run the deep check for every engine at every launch.

Health checks never run automatically for the `fake` engine in release builds.
