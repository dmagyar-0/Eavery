# 10 — Task Breakdown

Ordered by dependency. Each task: what to build, where, how you know it is
done, and a fallback if the primary approach fails. Sizes: S (< half a day),
M (about a day), L (2–3 days). Mark tasks done by replacing `[ ]` with `[x]`
and appending the commit hash.

Conventions: crate paths are relative to the repo root. "CLI" means
`crates/eavery-cli`. "Fake" means `crates/eavery-fake-agent`.

---

## S0 — Spikes (throwaway, before M0)

Pass/fail lines are in `01-implementation-plan.md` §4. Code lives in
`spikes/` and is deleted after S0; only the write-ups in
`docs/plan/manual-tests/S0-*.md` remain.

- [ ] **S0-T01 (L)** Terminal-free zero-key with ChatGPT: throwaway Tauri
  window that downloads Codex CLI and `@agentclientprotocol/codex-acp`
  binaries with checksum, spawns `codex login`, drives one ACP prompt via
  the 2.x SDK, and confirms the read-only mode blocks a write. Record the
  exact mode ids, the permission option kinds, and whether `mcpServers` from
  `session/new` is honoured.
- [ ] **S0-T02 (L)** Journal on a synced folder: `git2` detached git dir on a
  OneDrive (Windows) and iCloud (macOS) folder with ~500 MB of Office files,
  Excel holding one open. Checkpoint, edit, restore, hand-edit then restore.
  Record checkpoint times, placeholder behaviour, and lock errors.
- [ ] **S0-T03 (M)** `.docx` find-and-replace across runs with `zip` +
  `quick-xml` on ten real documents from a finance/ops person; open each in
  Word; record which constructs break (fields, tracked changes, split runs).
- [ ] **S0-T04 (M)** User sessions: watch three to five finance/ops people
  attempt a month-end task with Claude Code or Cowork on their own files.
  Record what the engines could and could not do, and which v1 Playbooks
  match real work.
- [ ] **S0-T05 (S)** Founder decision written into `CHANGELOG-plan.md`:
  proceed as planned, narrow the wedge, or option B from
  `REVIEW-2026-09.md` §7.

**S0 exit recorded:** ______

## M0 — Skeleton and fake engine

- [x] **M0-T01 (S)** `662f2c5` — Create the Cargo workspace from `03-architecture.md` §1–2
  with empty lib crates and `fn main() {}` binaries. `rust-toolchain.toml`,
  `.gitignore` (target, node_modules, dist, `*.sqlite`), `rustfmt.toml`,
  `clippy.toml`. Done when `cargo build --workspace` passes.
- [x] **M0-T02 (S)** `b2e700a` — CI: `.github/workflows/ci.yml` per `11-testing-ci.md` §5,
  Rust only for now (no Tauri yet). Done when the workflow is green on all three OSes.
- [x] **M0-T03 (M)** `1b98dd3` — `eavery-core::model` and `eavery-core::event` types from
  `03-architecture.md` §3–4, with `serde` and `ts-rs` derives, including
  `PlanJson` and `Plan::from(PlanJson)`. Unit test that every `CoreEvent`
  variant round-trips through JSON and that the sample `eavery-plan` block
  from `06` §2.3 parses into a `Plan` with populated steps. Done when the
  tests pass.
- [x] **M0-T04 (M)** `22c7f79` — `eavery-core::engine` trait (all methods `&self`; see
  `03-architecture.md` §5) and `RawAgentEvent` enum from `04-acp-engines.md`
  §6. No implementation yet.
- [x] **M0-T05 (L)** `446383f` — Fake agent: an ACP agent binary that reads a JSON script
  (`11-testing-ci.md` §2) and replays it: `initialize` reply, `session/new`
  reply with optional modes, and for each `session/prompt` a list of actions
  (`text`, `thought`, `tool_call`, `tool_call_update`, `plan`,
  `request_permission` expecting a decision, `fs_write`, `sleep_ms`, `stop`).
  Implement as hand-rolled JSON-RPC over stdio (it must not depend on the SDK
  so that SDK bugs are visible). Done when `echo` of a scripted text reply
  works via a manual `printf ... | fake-agent` test and unit tests cover
  request/response framing.
- [x] **M0-T06 (L)** `e386837` — `eavery-acp::AcpEngine` implementing `Engine` with the
  2.x SDK (`04-acp-engines.md` §5): spawn from a `LaunchSpec`, initialize,
  session/new, prompt with streaming to `EventSink`, permission handler
  bridge, cancel, shutdown. Fallback: `04-acp-engines.md` §7 hand-rolled
  client. Done when an integration test runs a fake script with text, a tool
  call, and a permission request through `AcpEngine` and observes the events
  in order.
- [x] **M0-T07 (M)** `01495f6` — CLI: `eavery-cli prompt --engine fake --script <path> --cwd <dir> "<text>"`
  prints events as they arrive and answers permissions from the terminal
  (`a`/`r`). Done when the M0 exit test passes and is recorded here.

**M0 exit recorded:** 2026-09-02, fake engine, Linux — `docs/plan/manual-tests/M0-exit.md`. CI on macOS and Windows still to confirm on the first push.

## M1 — Real engines from the CLI

- [x] **M1-T01 (M)** `f1664d5` — `eavery-engines`: `EngineSpec` table from
  `04-acp-engines.md` §2–3, `LaunchSpec` resolution (explicit path, PATH,
  well-known locations per `08-onboarding-packaging.md` §2), Windows
  `npx.cmd` handling. Unit tests with a fake PATH. `Platform` is a parameter
  rather than a `cfg!`, so the Windows rules are tested on every OS.
- [x] **M1-T02 (S)** `f1664d5` — PATH fix on macOS/Linux via the equivalent
  login-shell probe with a 3 s timeout (`eavery-engines::path_env`), resolved
  once per process. The probed PATH is returned as data rather than written
  back to the process environment, and passed to each engine child; see
  `CHANGELOG-plan.md`.
- [x] **M1-T03 (M)** `8af10da` — Health check (`04-acp-engines.md` §9) with
  timeouts and `EngineStatus` results, plus the 10-minute `HealthCache`; CLI
  command `eavery-cli engines` prints a table (`--deep`, `--all`, `--json`,
  `--engine <id>`). `eavery-cli prompt` now drives any engine in the table.
- [ ] **M1-T04 (M)** Manual verification against goose: configure goose with
  any provider, run the M1 exit prompt. Record the `modes` it advertises, and
  whether `mcpServers` in `session/new` are loaded, in `CHANGELOG-plan.md`.
- [ ] **M1-T05 (M)** Same for the Claude adapter (`claude-agent-acp`). Record
  the plan mode id, the permission option kinds it sends, **and exactly how
  `ExitPlanMode` arrives** (kind, title, rawInput) so `plan_exit_signatures`
  can be filled in. Record whether reads go through `fs/read_text_file`.
  Also decide how "not signed in" is detected: the adapter's `authMethods` is
  empty either way and `session/new` fails with a bare internal error, so §9
  step 3 as written never fires. Partial record, handshake only, in
  `manual-tests/M1-claude-partial.md`.
- [ ] **M1-T06 (M)** Same for `@agentclientprotocol/codex-acp`. Record mode
  ids (read-only / workspace-write / full-access or equivalents), approval
  behaviour in each, and whether `mcpServers` is honoured. Set
  `plan_mode_hint` to the read-only mode.
- [ ] **M1-T07 (S)** Same for `gemini --experimental-acp`. If it is unusable
  on the tested version, mark the engine `experimental: true` (hidden behind
  Developer mode) and record why.
- [x] **M1-T08 (S)** `b67f02a` — stderr capture ring buffer (M0-T06) and
  `EngineCrashed` event with the last 50 lines, via
  `CoreEvent::from_engine_error`; every other engine failure becomes an
  `Error` with a next action. Tested by scripting the fake agent to exit
  mid-prompt.
- [ ] **M1-T09 (M)** One-day evaluation of goose's `claude-acp` / `codex-acp`
  providers as a single front door (`04-acp-engines.md` §3). Record the
  verdict in `CHANGELOG-plan.md`; if adopted, the direct Claude/Codex rows
  become `experimental` rather than removed.

**M1 exit recorded:** ______

## M2 — Journal

- [x] **M2-T01 (M)** `f3ee1fd` — `Journal::open_or_create` with detached git
  dir, `info/exclude` (full list from `05` §3, including `*.eavery-tmp` and
  the engine state folders), initial checkpoint with progress callback and
  cancel. Tests 1, 6, 9, 12 from `05-git-journal.md` §7. The work tree is
  attached through `core.worktree` rather than `init_opts().workdir_path()`,
  which writes a gitlink into the Project and refuses on a Project that is
  already a git repository; see `CHANGELOG-plan.md`.
- [x] **M2-T02 (M)** `f3ee1fd`, `91f5ab6` — `checkpoint` with size guard,
  cloud-placeholder guard, and trailers; `list`. Tests 2, 5, 7.
- [x] **M2-T03 (M)** `91f5ab6` — `diff` and `diff_worktree` producing
  `ChangeSet` with text diffs. Tested on text and binary fixtures; a delta
  whose patch has no hunks is the binary test, because `Patch::from_diff`
  answers with a "Binary files differ" stub rather than nothing.
- [x] **M2-T04 (L)** `91f5ab6` — `restore` forward-only with the D16
  pre-restore checkpoint, per-file, lock-tolerant. Tests 3, 4, 8, 10, 11.
  Test 8 skips itself when run as a user that file permissions do not apply
  to, since root would pass it without testing anything.
- [ ] **M2-T05 (S)** *Mostly done* — `unprotected()`, `size_on_disk()`, the
  guard constants and `scan_project` with `MAX_FILES` / `WARN_TOTAL_BYTES`
  are in `42eb80f`. **Left:** background packing above 5,000 loose objects.
  `loose_object_count()` reports the number; the packing itself is not
  written, because reclaiming the space means deleting the loose copies once
  a pack holds them and libgit2 has no `gc` — see `CHANGELOG-plan.md`.
- [x] **M2-T06 (M)** `5f2b659` — `eavery-core::store`: SQLite open, migrations,
  CRUD for projects/sessions/turns/events/checkpoints/audit/settings. Tests
  with a temp db. The schema carries the rules rather than leaving them to the
  callers: STRICT tables, foreign keys on (with the per-connection pragma the
  cascades need), and two triggers that make the audit log append-only. See
  `CHANGELOG-plan.md`.
- [x] **M2-T07 (M)** `4ee3fec` — `eavery-core::turn` state machine in **direct
  mode only** (no plan gate yet): pre-checkpoint → prompt → post-checkpoint →
  digest. Permission handler = allow reads/reversible, ask via callback for
  the rest; it reclassifies first, because the ACP layer's risk class is a
  guess made without the Project root. One turn per Project (C13): a second
  `run_turn` while one runs returns an error, and so does `restore`. The tests
  drive a scripted in-process `Engine` rather than the fake agent binary,
  since core must not depend on `eavery-acp`; the fake agent covers the same
  ground through the CLI in M2-T08. See `CHANGELOG-plan.md`.
- [x] **M2-T08 (M)** `8107ee1` — CLI: `project open <dir>`, `project list`,
  `run --project <id> --engine <id> "<text>"`, `history --project <id>`,
  `undo --project <id> [--to <cp>]`, `diff --project <id> <from> [<to>]`.
  `--project` takes the folder as well as the id, `--to` takes the short
  checkpoint form the tables print, and a global `--data-dir` (or
  `EAVERY_DATA_DIR`) keeps the tests out of the real data directory.
- [ ] **M2-T09 (S)** M2 exit test against a real engine, byte-compare with
  `diff -r` (or a Rust helper), recorded below with the engine used.
  **Blocked on the same thing as M1-T04 to M1-T07**: an engine with a working
  login. The equivalent test against the fake engine passes as part of the CLI
  suite (`crates/eavery-cli/tests/project.rs`,
  `a_turn_changes_the_folder_and_undo_puts_it_back`), including the
  byte-for-byte check that Undo puts the folder back; what is missing is a run
  where the model is real.

**M2 exit recorded:** ______

## M3 — Desktop shell (Developer mode)

- [x] **M3-T01 (M)** `12e405c` — `pnpm create tauri-app` (react-ts) into
  `apps/desktop`; `src-tauri` added to the workspace as `eavery-desktop`;
  `cargo build --workspace` builds it on Linux here and CI builds it on all
  three OSes. CI gained the pnpm and Node steps it needs, because the crate
  embeds `apps/desktop/dist` at compile time. Tauri's release profile moved to
  the root manifest (a profile in a member is ignored) without its
  `panic = "abort"`; see `CHANGELOG-plan.md`.
- [x] **M3-T02 (M)** `12e405c` — `ts-rs` bindings generated into
  `apps/desktop/src/types.ts` by `cargo test -p eavery-core`, which rewrites
  the file and fails when that changed anything, so CI fails on a stale one.
  Built by walking `TS::visit_dependencies` from the IPC surface's types
  rather than with `#[ts(export)]`, which writes one file per type; see
  `CHANGELOG-plan.md`.
- [x] **M3-T03 (M)** `09c7022` — Tauri state: `AppCore` wrapping the store,
  a Journal per open Project, an engine per Project that has run a turn, the
  health-check cache, the `core://event` emission (the payload is a
  `StoredEvent`, so it carries `seq`), an in-process broadcast of the same
  events for anything without a webview, and the permission desk that
  `answer_permission` resolves.
- [x] **M3-T04 (L)** `09c7022` — Commands from `03-architecture.md` §7:
  projects, engines, `start_turn` (direct only; `mode: "plan"` is refused
  until M4), `answer_permission`, `cancel_turn`, checkpoints,
  `restore_checkpoint`, `diff_summary`, `list_events`, `list_audit`,
  `journal_size`, `unprotected_files`, settings. Errors cross as `AppError`
  (code, message, next action). Tested over the real IPC path with Tauri's
  mock runtime, which needs no window and no display; that test is what found
  Undo and "protect this now" needing an engine started, both since fixed
  (`CHANGELOG-plan.md`).
- [x] **M3-T09 (S)** `09c7022` — Kill children on exit: `RunEvent::Exit`
  shuts every engine down. Done here because M3-T03 is where the runners
  became reachable from the exit handler.
- [x] **M3-T05 (M)** `ae0c526` — Frontend `ipc.ts` (one typed function per
  command, the only place that calls `invoke`), `events.ts` (the
  `core://event` feed, with the gap re-fetch), `store.ts` (the window's state,
  read through `useSyncExternalStore`; no state library, because the only
  state the frontend has is a copy of what the core just said). The gap is
  detected globally and repaired per session — `seq` is one counter shared by
  every Project, so a skip may be in a conversation that is not on screen; see
  `CHANGELOG-plan.md`.
- [x] **M3-T06 (L)** `c94b7b6` — Screens: Home (the Projects, the folder picker
  through the dialog plugin, "Forget"), Project (three panes: Documents,
  Conversation with the composer, Activity with its tabs), Settings (the
  mode toggle, the default engine, every engine with its state chip and the
  §5 copy, "Check again"). Every string goes through `t()` from
  `vocab/dictionary.ts`, with both renderings. The Documents tree is M5-T04;
  "Plan it" is present and disabled until M4. Two commands added
  (`journal_info`, `diagnostics`); see `CHANGELOG-plan.md`.
- [x] **M3-T07 (M)** `c94b7b6` — `Transcript` (events grouped into keyed rows,
  streamed text appended to its row, row identity kept across re-renders),
  `ToolCallRow` (one line in Everyday, raw kind/status/locations/risk in
  Developer), `PermissionDialog` (modal, queued, focus-trapped, Esc = Don't,
  destructive focuses Reject), `Digest` with its Undo, `Checkpoints` with the
  "would change" preview, "Go back to this point", Undo of the last run, Redo
  after a restore, and Cmd/Ctrl+Z outside a text field. Checked end to end
  with the fake engine on a virtual display.
- [x] **M3-T08 (S)** `c94b7b6` — `Diagnostics`: `tracing` now also writes
  `<data_dir>/logs/eavery.log` (rotated at startup above 10 MB), and the
  panel — in Settings and as an Activity tab, Developer mode only — shows the
  version, the data folder, the log path and the last 200 lines, with "Copy
  diagnostics".

**M3 exit recorded:** ______

## M4 — Plan gate and policy

- [x] **M4-T01 (S)** `b2113db` — Prompt templates and the tiny renderer
  (`06-plan-gate-permissions.md` §4): `eavery-core::prompts`, with the two
  templates as Markdown beside it, `render` for `{{key}}` and `{{#if}}`, and
  `plan_prompt` / `execute_prompt` / `execute_prompt_for` (direct mode, §5).
  Unit tests for `{{#if}}`, broken tags, and both prompts.
- [x] **M4-T02 (M)** `b2113db` — `eavery-core::policy`: `classify` over a
  borrowed `CallFacts`, the §3.2 decision table as `decide` (the "always"
  column travels on `PermissionView` as `always`, and the core narrows an
  answer the table forbids), `ConnectorRegistry` with the `outbound` flag
  and tool names, the §3.3 signature with its per-Project memory in the
  settings table, and `is_inside` with the verbatim handling in
  `paths::is_inside`. Unit tests for every row plus the `\\?\C:\` root
  case, run on all three platforms. `ProjectRunner::open` takes the
  registry; see `CHANGELOG-plan.md`.
- [x] **M4-T03 (M)** `b2113db` — The gate is `policy::plan_gate` (with
  `is_plan_exit` and `bypassed_plan_gate`), a pure function tested for every
  row of §2.2 including the exit signatures matched on title or `rawInput`;
  `fs/write_text_file` is refused with the §2.2 message while
  `Engine::set_writes_allowed(false)` holds, and `fs/read_text_file` is
  served from anywhere (D15), both tested through the fake agent, whose
  `fs_write` now takes `expect_refused`. **Left for M4-T05:** applying the
  gate from the turn engine, which has no plan phase yet.
- [x] **M4-T04 (M)** `b2113db` — `eavery-core::plan::extract`: the last
  `eavery-plan` block as JSON, or the reply with its list items as steps.
  Tests for valid JSON, invalid JSON, the markdown-list fallback, the
  last-block rule, other fenced blocks, and an empty reply.
- [x] **M4-T05 (L)** `bf71530` — Two-phase turn in `eavery-core::turn`:
  `run_turn_in(TurnMode::Plan, ..)` goes Planning → AwaitingApproval →
  Executing, with the plan prompt under the gate (`Gatekeeper`: writes
  closed through `set_writes_allowed`, every mutation and every plan-mode
  exit refused, the §2.2 bypass reported as `PlanGateBypassed`), the plan
  parsed and emitted as `PlanReady` (with the vendor), an explicit yes
  awaited through the new `TurnCallbacks::approval` with no timeout, and the
  execute prompt under the policy with the plan's `outbound` list wording
  the Outbound questions. Modes: `pick_mode` moved to `eavery-core::engine`,
  `EngineSpec::facts()` hands the hints in as `EngineFacts`, the plan mode
  is set before the plan prompt and the asking mode before the execute
  prompt, a hint that matches nothing is logged and the gate holds. Cancel
  in Planning reaches the engine; in AwaitingApproval it ends the wait
  without the engine; in Executing as before. A rejected plan ends
  `Cancelled` with `stop_reason: "plan_rejected"`. Direct mode now sends the
  execute prompt with the request where the plan goes (§5). CLI: `run
  --plan [--approve yes|no] [--edits ..]`, the plan printed in full with
  "sends" and "forever" lines that always appear. Ten core tests, three CLI
  tests over real ACP; see `CHANGELOG-plan.md`.
- [x] **M4-T06 (M)** `bf71530` — Every decision writes an audit row with
  its actor: `plan_gate` for each planning answer (with `plan_exit`),
  `user` for `plan_approved` (with the edits) and `plan_rejected`, and
  `policy` / `user` for the execute phase as before, now with `phase` and
  `in_plan` in the detail. `list_audit` was in since M3-T04.
- [x] **M4-T07 (M)** `bf71530` — `PlanCard` in the transcript: summary,
  steps, documents, "Would leave this computer" and "Could not be undone"
  (always shown, "Nothing" when empty), "Your documents are sent to
  {vendor}", the person's edits once approved, the raw reply in Developer
  mode; while the turn waits, a box for changes and Go ahead / Not now.
  `approve_plan` / `reject_plan` commands over a `PlanDesk` keyed by turn;
  "Plan it" is the composer's primary button and Enter. "Always" storage
  was M4-T02's.
- [x] **M4-T08 (M)** `bf71530` — `scripts/plan.json` (§7 tests 1 and 2 in
  one script: an edit refused, a `fs/write_text_file` refused, an
  `ExitPlanMode` refused, then the approved execute turn) and the CLI
  tests for it, plus §7 test 3 as a script in `crates/eavery-cli/tests`;
  test 4 (malformed block) and the cancel and outbound cases are in
  `crates/eavery-core/tests/turn.rs`; test 6 (permission timeout) has been
  in `eavery-acp` since M0-T06. The outbound-Connector half of test 3 waits
  for a registry to fill (M6-T08).
- [ ] **M4-T09 (S)** M4 exit test with a real engine recorded below.
  **Blocked on the same thing as M1-T04 to M1-T07**: an engine with a
  working login.

**M4 exit recorded:** ______

## M5 — Everyday mode

- [x] **M5-T01 (M)** `527ffd8` — `vocab/dictionary.ts` complete per
  `07-ui-vocabulary.md` §2 (all 36 keys, and 160 more the screens needed);
  `t(mode, key, vars)` with `{name}` variables; the mode toggle persisted
  through `set_settings` into SQLite. Most of this landed with M3-T05 to
  M3-T08, which is why the dictionary was already close.
- [x] **M5-T02 (S)** `527ffd8` — `scripts/check-vocab.mjs`, parsed with the
  TypeScript compiler rather than grepped: string literals, template literals
  and JSX text under `src/screens`, `src/components` and `App.tsx`, skipping
  module specifiers and `className`. Run in CI on Linux (`pnpm check-vocab`).
  It also checks the dictionary's own `everyday:` renderings, including the
  `same(...)` ones — see `CHANGELOG-plan.md`.
- [x] **M5-T03 (M)** `527ffd8` — Everyday renderings: `ToolCallRow`
  one-liners with no tool names, thoughts hidden by a `null` dictionary
  entry, the `Digest` component, and errors rendered as their next action.
- [x] **M5-T04 (M)** `527ffd8` — `eavery-core::documents` plus the
  `list_documents` command, and `DocumentsPane` as a tree: folders open to
  depth 2 and wherever the last run touched something, `+`/`•`/`−` markers
  from the last digest, click to open with the OS. Names and paths only; no
  file contents are ever read.
- [x] **M5-T05 (S)** `527ffd8` — `TurnMode::Ask`: one prompt, writes shut at
  the engine, its read-only mode selected, and the plan gate answering every
  permission request. Everyday's second button is this; Developer's "Run"
  stays `Direct`. `eavery-cli prompt --ask` drives it too.
- [x] **M5-T06 (M)** `527ffd8` — Copy pass over every string, enforced from
  now on by the Everyday half of the vocabulary check. One real defect fixed:
  an error with no `next_action` rendered as "That didn't work. " with a
  dangling space and nothing to do about it.
- [ ] **M5-T07 (S)** M5 exit test (text-file task, implementer-run, Everyday
  mode end to end) recorded below. Needs a real engine and a person at the
  window, so it is open like the other exit tests.

**M5 exit recorded:** ______

## M6 — Document Connector and Playbooks

- [ ] **M6-T01 (M)** `eavery-docs-mcp` skeleton with `rmcp`, `--root`, `doc_info`, path guard, stdio test.
- [ ] **M6-T02 (M)** `xlsx_list_sheets`, `xlsx_read_range` (calamine).
- [ ] **M6-T03 (M)** `xlsx_write_cells` (umya) with validation round-trip; `xlsx_create` (rust_xlsxwriter).
- [ ] **M6-T04 (M)** `docx_read_text` (docx-rs).
- [ ] **M6-T05 (L)** `docx_replace_text` across runs with zip + quick-xml, preserving all parts; `docx_append_paragraphs`. Golden-file tests.
- [ ] **M6-T06 (M)** `pdf_read_text`, `pptx_read_text`.
- [ ] **M6-T07 (S)** Bundle the binary as `externalBin`; Eavery passes it in `session/new` for every engine; verify each engine lists its tools (record per engine).
- [ ] **M6-T08 (M)** Connectors settings UI and `connectors.json`; outbound flag.
- [ ] **M6-T09 (M)** Playbook discovery, validation, injection into the plan prompt; Settings → Playbooks list; composer menu.
- [ ] **M6-T10 (L)** Five bundled Playbooks, each with an explicit limits section, each run once with two engines; fix wording until both follow them.
- [ ] **M6-T11 (M)** Usability test with one non-technical person on the Word-document task (the M6 exit test, roadmap Phase 1 exit). Record observations and fix the top three problems before marking done. Word opens the modified `.docx` without repair; Excel opens the `.xlsx`.

**M6 exit recorded:** ______

## M7 — Onboarding, packaging, durability

- [ ] **M7-T01 (L)** Onboarding screens and flow from `08-onboarding-packaging.md` §1 with background detection, the four-way choice, and `NeedsNode` handling.
- [ ] **M7-T02 (M)** `EngineSource` download mechanism with checksum, quarantine removal on macOS, version pins, progress events; goose first.
- [ ] **M7-T02b (M)** Codex CLI and `@agentclientprotocol/codex-acp` downloads through the same mechanism; `install_engine` command.
- [ ] **M7-T02c (M)** `sign_in_engine`: spawn `codex login`, wait, re-run health check; `SigningIn` status copy. Clean-VM test with only a ChatGPT account and no Terminal.
- [ ] **M7-T03 (M)** Keychain storage for keys (`keyring`), env injection for goose child only.
- [ ] **M7-T04 (S)** Ollama detection (`/api/tags`) and model picker.
- [ ] **M7-T05 (M)** Session durability (`08-onboarding-packaging.md` §7): `session/load` or summary prepend; mid-turn close recovery.
- [ ] **M7-T06 (M)** Tauri updater config and signing keys; release workflow on tags.
- [ ] **M7-T07 (M)** Installers built in CI for all three OSes; manual install test on a clean VM each.
- [ ] **M7-T08 (S)** README updated with install instructions and the unsigned-build caveat.
- [ ] **M7-T09 (S)** M7 exit test recorded for: nothing installed + ChatGPT account (terminal-free); only Claude Code installed (Node permitted); nothing installed + API key; nothing installed + Ollama.

**M7 exit recorded:** ______

---

## Cross-cutting, do continuously

- Keep `CHANGELOG-plan.md` current whenever reality differs from these documents.
- Keep `BACKLOG.md` for ideas that are not v1.
- Every crate has `#![deny(unsafe_code)]` except where `git2` FFI needs otherwise (it does not).
- `cargo clippy --workspace --all-targets -- -D warnings` stays green.
