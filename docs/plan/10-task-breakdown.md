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
- [x] **M0-T04 (M)** `eavery-core::engine` trait (all methods `&self`; see
  `03-architecture.md` §5) and `RawAgentEvent` enum from `04-acp-engines.md`
  §6. No implementation yet.
- [ ] **M0-T05 (L)** Fake agent: an ACP agent binary that reads a JSON script
  (`11-testing-ci.md` §2) and replays it: `initialize` reply, `session/new`
  reply with optional modes, and for each `session/prompt` a list of actions
  (`text`, `thought`, `tool_call`, `tool_call_update`, `plan`,
  `request_permission` expecting a decision, `fs_write`, `sleep_ms`, `stop`).
  Implement as hand-rolled JSON-RPC over stdio (it must not depend on the SDK
  so that SDK bugs are visible). Done when `echo` of a scripted text reply
  works via a manual `printf ... | fake-agent` test and unit tests cover
  request/response framing.
- [ ] **M0-T06 (L)** `eavery-acp::AcpEngine` implementing `Engine` with the
  2.x SDK (`04-acp-engines.md` §5): spawn from a `LaunchSpec`, initialize,
  session/new, prompt with streaming to `EventSink`, permission handler
  bridge, cancel, shutdown. Fallback: `04-acp-engines.md` §7 hand-rolled
  client. Done when an integration test runs a fake script with text, a tool
  call, and a permission request through `AcpEngine` and observes the events
  in order.
- [ ] **M0-T07 (M)** CLI: `eavery-cli prompt --engine fake --script <path> --cwd <dir> "<text>"`
  prints events as they arrive and answers permissions from the terminal
  (`a`/`r`). Done when the M0 exit test passes and is recorded here.

**M0 exit recorded:** ______

## M1 — Real engines from the CLI

- [ ] **M1-T01 (M)** `eavery-engines`: `EngineSpec` table from
  `04-acp-engines.md` §2–3, `LaunchSpec` resolution (explicit path, PATH,
  well-known locations per `08-onboarding-packaging.md` §2), Windows
  `npx.cmd` handling. Unit tests with a fake PATH. 
- [ ] **M1-T02 (S)** PATH fix on macOS/Linux via `fix-path-env` (or equivalent
  login-shell probe with 3 s timeout), called once at process start in CLI
  and desktop.
- [ ] **M1-T03 (M)** Health check (`04-acp-engines.md` §9) with timeouts and
  `EngineStatus` results; CLI command `eavery-cli engines` prints a table.
- [ ] **M1-T04 (M)** Manual verification against goose: configure goose with
  any provider, run the M1 exit prompt. Record the `modes` it advertises, and
  whether `mcpServers` in `session/new` are loaded, in `CHANGELOG-plan.md`.
- [ ] **M1-T05 (M)** Same for the Claude adapter (`claude-agent-acp`). Record
  the plan mode id, the permission option kinds it sends, **and exactly how
  `ExitPlanMode` arrives** (kind, title, rawInput) so `plan_exit_signatures`
  can be filled in. Record whether reads go through `fs/read_text_file`.
- [ ] **M1-T06 (M)** Same for `@agentclientprotocol/codex-acp`. Record mode
  ids (read-only / workspace-write / full-access or equivalents), approval
  behaviour in each, and whether `mcpServers` is honoured. Set
  `plan_mode_hint` to the read-only mode.
- [ ] **M1-T07 (S)** Same for `gemini --experimental-acp`. If it is unusable
  on the tested version, mark the engine `experimental: true` (hidden behind
  Developer mode) and record why.
- [ ] **M1-T08 (S)** stderr capture ring buffer and `EngineCrashed` event with
  the last 50 lines; test by scripting the fake agent to exit mid-prompt.
- [ ] **M1-T09 (M)** One-day evaluation of goose's `claude-acp` / `codex-acp`
  providers as a single front door (`04-acp-engines.md` §3). Record the
  verdict in `CHANGELOG-plan.md`; if adopted, the direct Claude/Codex rows
  become `experimental` rather than removed.

**M1 exit recorded:** ______

## M2 — Journal

- [ ] **M2-T01 (M)** `Journal::open_or_create` with detached git dir,
  `info/exclude` (full list from `05` §3, including `*.eavery-tmp` and the
  engine state folders), initial checkpoint with progress callback and
  cancel. Tests 1, 6, 9, 12 from `05-git-journal.md` §7.
- [ ] **M2-T02 (M)** `checkpoint` with size guard, cloud-placeholder guard, and trailers; `list`. Tests 2, 5, 7.
- [ ] **M2-T03 (M)** `diff` and `diff_worktree` producing `ChangeSet` with text diffs. Test on text and binary fixtures.
- [ ] **M2-T04 (L)** `restore` forward-only with the D16 pre-restore checkpoint, per-file, lock-tolerant. Tests 3, 4, 8, 10, 11.
- [ ] **M2-T05 (S)** `unprotected()`, `size_on_disk()`, the guard constants, background packing above 5,000 loose objects; `open_project` size scan with `MAX_FILES` and `WARN_TOTAL_BYTES`.
- [ ] **M2-T06 (M)** `eavery-core::store`: SQLite open, migrations, CRUD for
  projects/sessions/turns/events/checkpoints/audit/settings. Tests with a temp db.
- [ ] **M2-T07 (M)** `eavery-core::turn` state machine in **direct mode only**
  (no plan gate yet): pre-checkpoint → prompt → post-checkpoint → digest.
  Permission handler = allow reads/reversible, ask via callback for the rest.
  One turn per Project (C13): a second `start_turn` while one runs returns
  an error; `restore` is refused while a turn runs.
- [ ] **M2-T08 (M)** CLI: `project open <dir>`, `project list`, `run --project <id> --engine <id> "<text>"`,
  `history --project <id>`, `undo --project <id> [--to <cp>]`, `diff --project <id> <from> <to>`.
- [ ] **M2-T09 (S)** M2 exit test against a real engine, byte-compare with
  `diff -r` (or a Rust helper), recorded below with the engine used.

**M2 exit recorded:** ______

## M3 — Desktop shell (Developer mode)

- [ ] **M3-T01 (M)** `pnpm create tauri-app` (react-ts) into `apps/desktop`;
  add `src-tauri` to the workspace; app builds and shows a window on all three
  OSes in CI (build only, no run).
- [ ] **M3-T02 (M)** `ts-rs` bindings generation into `apps/desktop/src/types.ts`
  via a `cargo test` in `eavery-core`; CI fails if the generated file is stale.
- [ ] **M3-T03 (M)** Tauri state: an `AppCore` struct wrapping store, journal
  cache, engine registry, event broadcast; `core://event` emission with `seq`.
- [ ] **M3-T04 (L)** Commands from `03-architecture.md` §7: projects, engines,
  `start_turn` (direct), `answer_permission`, `cancel_turn`, checkpoints,
  `journal_size`, `list_events`, settings. Each command is a thin call into
  `eavery-core`.
- [ ] **M3-T05 (M)** Frontend `ipc.ts`, `events.ts`, `store.ts` with gap re-fetch.
- [ ] **M3-T06 (L)** Screens: Home, Project (three panes), Settings (mode +
  engines only). Raw strings acceptable but must go through `t()` from the start.
- [ ] **M3-T07 (M)** `Transcript`, `ToolCallRow`, `PermissionDialog` (queue), `Checkpoints` with Undo/Redo.
- [ ] **M3-T08 (S)** `Diagnostics` panel with log tail (tail the `tracing` file).
- [ ] **M3-T09 (S)** Kill children on exit (`kill_on_drop` plus explicit
  shutdown in Tauri's `RunEvent::Exit`).

**M3 exit recorded:** ______

## M4 — Plan gate and policy

- [ ] **M4-T01 (S)** Prompt templates and the tiny renderer (`06-plan-gate-permissions.md` §4). Unit tests for `{{#if}}`.
- [ ] **M4-T02 (M)** `policy::classify` and the decision table; `ConnectorRegistry` with the `outbound` flag; `is_inside` with Windows verbatim-path normalisation. Unit tests for every row plus a `\\?\C:\` root case.
- [ ] **M4-T03 (M)** `PlanGateHandler` with `plan_exit_signatures` refusal and `fs/write_text_file` refusal during planning; `fs/read_text_file` served from anywhere (D15).
- [ ] **M4-T04 (M)** Plan extraction parser with fallback; tests for valid JSON, invalid JSON, markdown list fallback.
- [ ] **M4-T05 (L)** Two-phase turn in `eavery-core::turn`: Planning → AwaitingApproval → Executing; mode switching via `set_mode` using `plan_mode_hint` / `asking_mode_hint`; cancel in each phase; digest with outbound and refused lists (outbound list always present, "Nothing" when empty).
- [ ] **M4-T06 (M)** Audit log rows for every decision; `list_audit` command (Developer mode view).
- [ ] **M4-T07 (M)** UI: `PlanCard` including the "Your documents are sent to {vendor}" line, `approve_plan`/`reject_plan` commands, "always" storage.
- [ ] **M4-T08 (M)** Fake-agent scripts and tests from `06-plan-gate-permissions.md` §7.
- [ ] **M4-T09 (S)** M4 exit test with a real engine recorded below.

**M4 exit recorded:** ______

## M5 — Everyday mode

- [ ] **M5-T01 (M)** `vocab/dictionary.ts` complete per `07-ui-vocabulary.md` §2; `t()` with variables; mode toggle persisted.
- [ ] **M5-T02 (S)** `scripts/check-vocab.mjs` (string literals and JSX text only, parsed, not grepped) and CI step.
- [ ] **M5-T03 (M)** Everyday renderings: `ToolCallRow` one-liners, thoughts hidden, `Digest` component, error-as-next-action.
- [ ] **M5-T04 (M)** `DocumentsPane` with changed-file markers and OS open.
- [ ] **M5-T05 (S)** "Ask a question" direct mode with read-only intent.
- [ ] **M5-T06 (M)** Copy pass: every string in the app reviewed against the UI rules; no protocol words in Everyday mode.
- [ ] **M5-T07 (S)** M5 exit test (text-file task, implementer-run, Everyday mode end to end) recorded below.

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
