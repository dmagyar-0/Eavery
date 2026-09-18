# Eavery desktop

The Tauri v2 shell: a React + TypeScript frontend over `eavery-core`.

The frontend is a renderer. It calls the commands in
[`docs/plan/03-architecture.md`](../../docs/plan/03-architecture.md) §7 and
listens to one event; it holds no business logic, because every decision that
matters — what a tool call is allowed to do, what is checkpointed, what the
history says — belongs to the core, where the CLI and the tests can reach it
too.

## Running it

```sh
pnpm install
pnpm tauri dev      # the window, with the frontend hot-reloading
pnpm build          # typecheck and build the frontend on its own
```

On Linux this needs Tauri's system libraries; the list CI installs is in
[`.github/workflows/ci.yml`](../../.github/workflows/ci.yml).

`cargo build -p eavery-desktop` embeds `dist/`, so the frontend has to be
built before the Rust side. That is also why CI builds it first.

## `src/types.ts`

Generated from the Rust types by `cargo test -p eavery-core`, and checked in.
The test fails when what is committed is not what the types produce, so a
renamed field is a failed test rather than a blank pane. Do not edit it.

## Layout

```
src/
├── App.tsx                 # the shell: top bar, the screen that is up, the dialogs, Cmd/Ctrl+Z
├── ipc.ts                  # one typed function per command; the only file that calls invoke
├── os.ts                   # the folder picker and "open with the OS"; the only file that calls the plugins
├── events.ts               # the core://event feed, with the gap re-fetch
├── store.ts                # the window's state: a copy of what the core last said
├── transcript.ts           # events → keyed rows, with row identity kept across re-renders
├── types.ts                # generated; see above
├── vocab/
│   ├── dictionary.ts       # every user-facing string, in both modes; null = hidden in that mode
│   ├── t.ts                # t(mode, key, vars): pure
│   └── useT.ts             # the hook that binds t() to the mode in the store
├── screens/                # Home, Project (three panes), Settings
├── components/             # Transcript, ToolCallRow, PermissionDialog, Digest, Checkpoints,
│                           # Composer, DocumentsPane, Activity, Diagnostics, Trouble
└── styles/app.css
```

Two rules hold everywhere under `screens/` and `components/`:

- **No string of its own.** Every word a person sees comes from
  `vocab/dictionary.ts` through `t()`, so the Everyday/Developer toggle is one
  lookup away from all of it, and the words a non-technical person must never
  see live in one file. M5-T02 adds the script that checks this in CI.
- **No decision of its own.** What a tool call may do, what Undo goes back to,
  what is protected: the core answers, the window shows. A rule that turns up
  in `store.ts` is a rule in the wrong crate.

## What is here so far

M3-T01 to M3-T09 bar the milestone's exit test: the window, the generated
types, the state, the commands, the screens, the transcript and its dialogs,
the history with Undo and Redo, and the Diagnostics panel. "Plan it" is
disabled until the plan gate (M4); the Documents tree is M5-T04; install and
sign-in buttons are M7. The task list is
[`docs/plan/10-task-breakdown.md`](../../docs/plan/10-task-breakdown.md).

## Trying it without an assistant

The fake engine is offered in a debug build (or with `EAVERY_DEV=1`). It
needs `eavery-fake-agent` on the PATH and a script to replay, which it takes
from `EAVERY_FAKE_SCRIPT` since the app starts every engine the same way;
`EAVERY_DATA_DIR` keeps the trial out of the real data directory:

```sh
cargo build -p eavery-fake-agent
PATH="$PWD/../../target/debug:$PATH" \
EAVERY_FAKE_SCRIPT="$PWD/../../crates/eavery-core/tests/scripts/hello.json" \
EAVERY_DATA_DIR=/tmp/eavery-trial pnpm tauri dev
```

Then Settings → make the fake engine the default, Home → open a copy of a
folder, and ask for "notes": the script asks permission to create a file,
creates it, and the transcript, the Digest, the history and Undo are all
real.
