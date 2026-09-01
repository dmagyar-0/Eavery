# 02 — The Biggest Challenges and How to Solve Them

Ordered by how badly each can hurt the project. Each entry states the problem,
the evidence, the chosen solution, and what to do if the solution fails.

---

## C1. The zero-key path depends on a policy Anthropic has already tried to change

**Problem.** Bet #1 is "no API key: drive the user's own Claude Code over ACP".
Claude Code does not speak ACP natively. The bridge is the
`@agentclientprotocol/claude-agent-acp` adapter, which is built on the Claude
Agent SDK. On 13 May 2026 Anthropic announced that from 15 June 2026 ACP usage,
`claude -p`, the Agent SDK, and apps built on it would stop drawing from Pro/Max
subscription limits and move to a separate credit pool. On 16 June 2026 the
change was paused, not cancelled. Today it works; tomorrow it may not.

**Solution.**
1. **Engine abstraction from day one** (D2, D10). Every engine is a row in a
   table (`04-acp-engines.md` §3). Adding or disabling one is data, not code.
2. **Ship three paths, all tested in CI with the fake agent and manually with
   real engines:** zero-key (Claude Code, Codex, Gemini), BYO-key (goose +
   Anthropic/OpenAI/Google/OpenRouter key), local (goose + Ollama).
3. **Health check at launch, not at first prompt.** Before offering an engine in
   the UI, Eavery spawns it, sends `initialize`, opens a session, and sends a
   one-token prompt with a timeout. If it fails with an auth error, the UI says
   "Claude Code needs you to sign in" or "This engine is not available with your
   subscription right now — switch to Codex, or add a key" and offers the switch.
4. **Never touch the vendor's tokens.** Eavery only launches the CLI. It does
   not read, copy, or forward credentials. This keeps Eavery on the right side
   of every version of the policy so far.
5. **Codex is the equal first-class zero-key engine, not a fallback.** OpenAI's
   Codex CLI supports ChatGPT sign-in, and `codex-acp` reuses it. Test both
   equally.

**If it fails.** If Anthropic enforces the split, the Claude row in the engine
table gets `status: needs_credits` with a link to Anthropic's instructions; the
onboarding flow steers to Codex, Gemini, BYO-key, or Ollama. No code change.

---

## C2. Invisible git inside real office folders

**Problem.** Office folders live in Documents, OneDrive, Dropbox, SharePoint
sync, or a network share. A `.git` directory inside such a folder gets synced,
conflicts, is shown to the user, and breaks the "invisible" promise. Office
files are zipped binaries (`.docx`, `.xlsx`, `.pptx`) and can be large. Excel
and Word hold exclusive locks on open files on Windows and leave `~$name.docx`
lock files. Folders can contain gigabytes of PDFs and images the agent never
touches.

**Solution.** See `05-git-journal.md` for detail.
1. **Detached git directory.** The Journal's git dir lives at
   `<eavery-data>/journals/<project-id>/` and points at the Project folder as
   its work tree. Nothing is written inside the Project folder except the
   files the agent changes. `git2` supports this via
   `RepositoryInitOptions::workdir_path`.
2. **Built-in ignore list**, applied via the Journal's own `info/exclude`
   (never a `.gitignore` in the Project): `~$*`, `.~lock.*#`, `.DS_Store`,
   `Thumbs.db`, `desktop.ini`, `*.tmp`, `node_modules/`, `.git/`.
3. **Size guard.** Files above 50 MB are excluded from checkpoints and listed in
   the Project's "Not protected" panel. The folder's total tracked size is
   shown at Project creation; above 2 GB Eavery asks the user to pick a
   subfolder.
4. **Locked files.** Checkpoint reads never need write access. Restore writes
   files; on a sharing-violation error (Windows) Eavery reports "Close
   `Budget.xlsx` in Excel and press Undo again" and leaves the rest restored.
   Restore is per-file and idempotent, so retrying is safe.
5. **Forward-only restore** (D5). Restore = write a new commit whose tree is the
   target checkpoint's tree, then check out that tree. History only grows.
6. **Checkpoint cadence:** one before every turn (captures the user's own edits
   since last time), one after every turn, one on demand. Not per tool call:
   per-tool-call commits of a 30 MB spreadsheet are too slow and add nothing
   the digest cannot show.

**If it fails.** If `git2` cannot handle a folder (permissions, exotic
filesystem), the Project cannot be opened and the UI says why. Eavery must
refuse to run an agent on an unprotected folder. There is no "run anyway".

---

## C3. Making an engine plan before it acts

**Problem.** Claude Code, Codex, goose, and Gemini decide for themselves when to
call tools. Eavery wants: plan in plain English, user approves, then execute.
Asking nicely in the prompt is not enforcement; a model will sometimes start
editing during "planning".

**Solution.** See `06-plan-gate-permissions.md`.
1. **Client-side enforcement.** Eavery serves `session/request_permission`.
   During the plan phase it answers every request whose tool call `kind` is
   `edit`, `delete`, `move`, `execute`, or `fetch` with `reject_once`, and
   records that the engine tried. The engine gets told "not now" and continues
   to reason. Eavery also serves `fs/write_text_file` and refuses it during
   planning.
2. **Use the engine's own plan mode when it exists.** The `session/new` response
   lists `modes`. The Claude adapter exposes a plan mode; if a mode id
   contains `plan`, Eavery sets it with `session/set_mode` for the plan phase
   and switches back for execution. If no such mode exists, Eavery relies on
   step 1 plus the prompt.
3. **Structured plan extraction.** The planning prompt asks the engine to end
   with a fenced block ```` ```eavery-plan ```` containing JSON
   (`steps[]`, `files_touched[]`, `outbound[]`, `irreversible[]`,
   `will_not_do[]`). Eavery parses it; if parsing fails, it renders the
   engine's last message as the plan and still gates on approval. Never block
   the user because a model wrote slightly wrong JSON.
4. **Execute phase carries the approved plan verbatim** in the prompt, plus the
   user's edits, plus "do not do anything not in this plan; if you must,
   stop and explain". Permission policy switches to the irreversibility axis.
5. **Engines that never ask.** If an engine is launched in a bypass or
   "yolo" mode it will never call `request_permission`. Eavery must launch
   each engine in its asking mode (`04-acp-engines.md` §3 lists the flags)
   and treat any engine that completes a turn with file changes and zero
   permission requests during planning as "did not respect the plan gate":
   the changes are still checkpointed and undoable, and the UI shows a warning.

**If it fails.** The Journal is the backstop. Even a fully misbehaving engine
cannot lose work; the worst case is one Undo.

---

## C4. The ACP SDK is new and just had a breaking 2.0

**Problem.** `agent-client-protocol` 2.0.0 shipped 23 July 2026 with a different
API from 0.x (builder, `connect_with`, `on_receive_*`). Tutorials on the web
show the old API. A smaller model will copy the wrong one.

**Solution.**
1. `04-acp-engines.md` §5 contains the verified 2.0 client example, copied from
   the SDK repository. Start from it. Do not use `ClientSideConnection`,
   `Client` trait impls, or `LocalSet` patterns from 0.x articles.
2. All SDK usage lives in one crate, `eavery-acp`. The rest of the code only
   sees `CoreEvent` and `EngineHandle` (`03-architecture.md` §4). An SDK change
   touches one crate.
3. **Fallback:** the protocol is newline-delimited JSON-RPC 2.0 over stdio with
   about ten methods. `04-acp-engines.md` §7 specifies a hand-rolled client
   using `serde_json` and the `agent-client-protocol-schema` types. If the SDK
   costs more than two days of fighting, switch to the fallback and record it.

---

## C5. GUI apps do not see the user's PATH

**Problem.** On macOS, an app launched from Finder or the Dock gets
`PATH=/usr/bin:/bin:/usr/sbin:/sbin`. `claude`, `codex`, `gemini`, `goose`,
`node`, and `npx` are usually in `/opt/homebrew/bin`, `~/.local/bin`,
`~/.nvm/...`, or `~/.volta/bin`. Spawning `claude` fails with "not found" even
though it works in Terminal. On Windows, `npx` is `npx.cmd` and must be run
through `cmd /C` or resolved to the `.cmd` file. On Linux, Flatpak and Snap
sandboxes hide binaries.

**Solution.**
1. At startup, on macOS and Linux, run the user's login shell to obtain the
   real PATH: `$SHELL -ilc 'printf %s "$PATH"'` (with a 3 s timeout), and
   set it on the Eavery process. The `fix-path-env` crate does exactly this;
   use it or copy its approach.
2. Engine discovery searches, in order: an explicit path from Settings; the
   fixed PATH; a list of well-known locations per platform (`08-onboarding-packaging.md` §2).
3. On Windows, resolve `npx` to `npx.cmd` and spawn with `cmd /C`, or prefer the
   platform binary release of `codex-acp` and Node-free engines.
4. Every spawn failure is logged with the exact command, the PATH used, and the
   OS error, and surfaced in Developer mode.

---

## C6. Streaming, cancellation, and process lifecycle

**Problem.** Engines stream `session/update` notifications while a
`session/prompt` request is outstanding. The user may cancel mid-turn. The app
may quit with a child running. A crashed engine must not hang the UI.

**Solution.**
1. One tokio task per engine process owns the connection. It forwards every
   notification as a `CoreEvent` over a broadcast channel. The UI subscribes.
2. Cancel = send `session/cancel` (a notification), then wait up to 5 s for the
   `session/prompt` response with `stopReason: cancelled`; then kill the
   process if it did not return.
3. On app exit, kill all child processes. Use `tokio::process::Command` with
   `kill_on_drop(true)` and keep the `Child` in the engine task.
4. If the engine's stdout closes unexpectedly, emit `CoreEvent::EngineCrashed`
   with the last 50 lines of stderr; the UI offers restart. The Journal's
   pre-turn checkpoint makes a crash mid-edit recoverable.

---

## C7. Permission fatigue versus real safety

**Problem.** Every competitor prompts uniformly and trains users to click
"allow all". Eavery promises near-silence on reversible actions and loud
prompts on irreversible ones. But the client sees only a tool call's `kind`,
`title`, `locations`, and `rawInput`, not its true consequences.

**Solution.** A deterministic classifier in `eavery-core::policy`:
- `edit`, `delete`, `move` with every `locations[].path` inside the Project
  folder → **reversible** → auto-allow (`allow_once`), log it.
- Same kinds with any path outside the Project → **destructive** → ask.
- `execute` → **ask** in v1 (shell commands can do anything). Developer mode
  can enable "auto-allow commands inside the Project".
- `fetch` → **outbound** → ask, show the URL/host from `rawInput` if present.
- Tool calls from an MCP Connector marked `outbound: true` in Settings (email,
  Slack, HTTP) → **outbound** → ask, always, no "always allow" option.
- `read`, `search`, `think`, `other` → auto-allow.
- Unknown or missing `kind` → treat as `execute`.

"Always allow" is offered only for reversible and read classes and is stored
per Project. Outbound never gets "always".

---

## C8. One event model, two vocabularies, no divergence

**Problem.** If Everyday and Developer mode are two UIs they will drift, and
the escape hatch stops being a toggle.

**Solution.** The UI renders a single `CoreEvent` stream. A `vocab.ts`
dictionary maps every user-visible string through `t(key, mode)`. Components
never contain literal words for engine concepts. The Developer toggle changes
the mode value and which panels are visible, nothing else. See
`07-ui-vocabulary.md` §2 for the dictionary and the lint rule that enforces it.

---

## C9. Durable sessions across restart

**Problem.** Engines hold conversation state in memory. Quit the app and the
context is gone. ACP has `session/load` but only some engines support it, and
even then the process must be respawned.

**Solution.**
1. Every `CoreEvent` is written to SQLite as it happens. The UI can always
   re-render history from the database.
2. On restart, if the engine advertised `loadSession: true` in `initialize`,
   Eavery calls `session/load` with the stored session id and cwd. If not,
   Eavery starts a new session and prepends a compact summary
   ("Previous conversation summary: ...", generated from the last N events)
   to the first prompt.
3. The Journal is engine-independent, so checkpoints and Undo always work
   regardless of whether the conversation resumed.

---

## C10. Document fidelity

**Problem.** Rust has good `.xlsx` crates and adequate `.docx` crates, and
nothing production-grade for writing `.pptx`. A model asked to emit OOXML by
hand will produce files Office refuses to open. Users judge the product on
whether Word opens the file.

**Solution.** See `09-documents-playbooks.md`.
1. v1 ships `eavery-docs-mcp` with narrow, deterministic tools:
   `docx_read_text`, `docx_replace_text`, `docx_append_paragraphs`,
   `xlsx_read_range`, `xlsx_write_cells`, `xlsx_list_sheets`,
   `pdf_read_text`, `pptx_read_text`. Each tool round-trips the original file
   with `zip` + `quick-xml` for edits that must preserve formatting, and uses
   `umya-spreadsheet` / `docx-rs` where the operation is well supported.
2. Every write tool validates by re-opening its own output before returning.
3. The MCP tool descriptions tell the engine to prefer these tools over
   writing scripts, and the Playbooks do the same.
4. `.pptx` write is deferred; the tool list says so, so the engine does not
   attempt it silently.

**If it fails.** If a crate cannot preserve a document's formatting, the tool
returns an error and the engine reports "I can read this file but changing it
would risk its formatting"; the user is not handed a corrupt file.

---

## C11. Scope discipline

**Problem.** The vision documents describe a large product. A session
implementing it will be tempted to build the Playbook registry, scheduling,
or a custom agent loop.

**Solution.** `01-implementation-plan.md` §2 lists what v1 is not. The task list
in `10-task-breakdown.md` is closed: if a task is not there, it is not v1. New
ideas go into `docs/plan/BACKLOG.md`, not into code.
