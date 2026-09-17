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

## What is here so far

M3-T01: the window. M3-T02: the generated types. The application state, the
commands and the screens are M3-T03 to M3-T09 in
[`docs/plan/10-task-breakdown.md`](../../docs/plan/10-task-breakdown.md).
