# 11 — Testing and CI

## 1. Principles

- The fake agent is the primary test double. Every behaviour of the turn
  engine, plan gate, and policy is tested against scripted agents, not against
  real LLMs. Real engines are tested manually at milestone exits and recorded.
- The Journal is tested with real files in temp directories; no mocking of git.
- The UI is tested lightly: type-checked, linted, vocab-checked, and built.
  Component tests are optional in v1.
- Tests must run on Linux, macOS, and Windows in CI. Path handling bugs show
  up only on Windows; do not skip it.

## 2. Fake agent script format

`crates/eavery-fake-agent` reads a JSON file:

```json
{
  "initialize": { "agentInfo": {"name": "fake", "version": "0.0.1"}, "loadSession": false },
  "session": { "modes": { "currentModeId": "work", "availableModes": [
      {"id": "work", "name": "Work"}, {"id": "plan", "name": "Plan"} ] } },
  "turns": [
    { "match": "plan", "actions": [
        {"thought": "Looking around"},
        {"tool_call": {"id": "t1", "title": "Read report.docx", "kind": "read", "status": "completed", "locations": ["{{cwd}}/report.docx"]}},
        {"request_permission": {"toolCallId": "t2", "title": "Edit report.docx", "kind": "edit", "locations": ["{{cwd}}/report.docx"], "expect": "reject_once"}},
        {"text": "I will update the report.\n\n```eavery-plan\n{\"summary\":\"Update report\",\"steps\":[\"Open report\",\"Change FY25 to FY26\"],\"files_touched\":[\"report.docx\"],\"outbound\":[],\"irreversible\":[],\"will_not_do\":[\"send email\"]}\n```"},
        {"stop": "end_turn"} ] },
    { "match": "approved this plan", "actions": [
        {"request_permission": {"toolCallId": "t3", "title": "Edit report.docx", "kind": "edit", "locations": ["{{cwd}}/report.docx"], "expect": "allow_once"}},
        {"fs_write": {"path": "{{cwd}}/report.docx", "text": "FY26"}},
        {"tool_call": {"id": "t3", "title": "Edit report.docx", "kind": "edit", "status": "completed"}},
        {"text": "Done. Changed one number."},
        {"stop": "end_turn"} ] }
  ]
}
```

Rules:
- `match` is a case-insensitive substring of the prompt text; the first
  matching turn is used, and turns are consumed in order (a turn used once is
  skipped next time unless `"repeat": true`).
- `{{cwd}}` is replaced with the `cwd` from `session/new`.
- `request_permission.expect` makes the fake agent exit with code 3 if the
  client answers with a different `kind`, which fails the test loudly.
- `fs_write` uses the client's `fs/write_text_file`; `write_direct` writes the
  file itself (to simulate engines that bypass the client).
- `sleep_ms` and `exit` (crash simulation) exist for lifecycle tests.
- Any `session/cancel` makes the current turn stop with `cancelled`.

Scripts live in `crates/eavery-core/tests/scripts/*.json` and are shared by
core, CLI, and desktop tests.

## 3. Test inventory by crate

| Crate | Tests |
|---|---|
| `eavery-core` | model round-trips; journal (`05-git-journal.md` §7); store migrations and CRUD; policy decision table; plan parser; prompt renderer; turn state machine with fake scripts (direct, plan gate, cancel, crash, permission timeout); durability (summary prepend) |
| `eavery-acp` | framing (SDK or hand-rolled); unknown `sessionUpdate` values ignored; permission bridge; cancel returns `cancelled`; stderr ring buffer |
| `eavery-engines` | spec resolution with fake PATH per OS; Windows `npx.cmd`; well-known locations; health check state machine with fake agent |
| `eavery-fake-agent` | JSON-RPC framing; script matching; `expect` failure exit |
| `eavery-docs-mcp` | golden files; write validation; path guard; stdio listing |
| `desktop` | `tsc --noEmit`; `eslint`; `scripts/check-vocab.mjs`; `pnpm build`; `cargo test` in `src-tauri` for command serialisation; bindings freshness |

## 4. Manual test protocol (milestone exits)

Record in `10-task-breakdown.md` under each milestone: date, OS, engine and
version, exact prompt, result, and anything surprising. Keep a
`docs/plan/manual-tests/` folder with one markdown file per run.

Standard prompts:
- M1: "List the files in this folder and summarise what each one is for."
- M2: "Create a file called notes.txt containing today's date, then add a second line saying hello."
- M4: "Look up the current weather for London and write it into weather.txt" (must trigger an outbound prompt).
- M5/M6: "Rename every 'FY25' to 'FY26' in the three Word documents in this folder and tell me what you changed."

## 5. CI workflow

`.github/workflows/ci.yml`:

```yaml
name: ci
on: { push: { branches: [main] }, pull_request: {} }
jobs:
  rust:
    strategy: { matrix: { os: [ubuntu-latest, macos-latest, windows-latest] } }
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { components: "clippy, rustfmt" }
      - uses: Swatinem/rust-cache@v2
      - if: runner.os == 'Linux'
        run: sudo apt-get update && sudo apt-get install -y libwebkit2gtk-4.1-dev libssl-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev
      - run: cargo fmt --all -- --check
      - run: cargo clippy --workspace --all-targets -- -D warnings
      - run: cargo test --workspace
  web:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: pnpm/action-setup@v4
        with: { version: 9 }
      - uses: actions/setup-node@v4
        with: { node-version: 20, cache: pnpm, cache-dependency-path: apps/desktop/pnpm-lock.yaml }
      - run: pnpm install --frozen-lockfile
        working-directory: apps/desktop
      - run: pnpm tsc --noEmit && pnpm eslint . && node ../../scripts/check-vocab.mjs && pnpm build
        working-directory: apps/desktop
```

Add the `web` job in M3. Add a `release.yml` in M7 using `tauri-apps/tauri-action`.

`git2` with `vendored-libgit2` compiles C code; the first CI build takes
several minutes, and `rust-cache` makes later ones fast. `rusqlite` `bundled`
likewise. Do not remove the cache step.

## 6. Definition of done for any task

1. Code compiles with no warnings under clippy `-D warnings`.
2. Tests for the task exist and pass on all three OSes in CI.
3. The task's acceptance line in `10-task-breakdown.md` is ticked with the commit hash.
4. Anything learned that contradicts the plan is in `CHANGELOG-plan.md`.
