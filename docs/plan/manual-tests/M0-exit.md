# M0 exit test

**Date:** 2026-09-02
**OS:** Linux 6.18 (x86_64), Rust 1.94.1
**Engine:** `fake` (`eavery-fake-agent`), the only engine that exists before M1.

## What the exit test asks for

> CI green on Linux, macOS, Windows; the CLI round-trips a prompt through the
> fake agent including one permission request.
> — `01-implementation-plan.md` §4

## Script

`crates/eavery-cli/tests/prompt.rs::exit_test_script`: a thought, a read tool
call, an engine-side plan update, a permission request expecting `allow_once`,
a write through the client's `fs/write_text_file`, a tool call update, a
closing message, `end_turn`.

## Result

Run three ways, all passing:

1. **`--answer allow`** — the transcript below, and `notes.txt` written into the
   project folder by the client rather than by the agent.
2. **A real terminal, answered `a`** — driven through a pty. The prompt renders
   the tool call, its risk class and the path, and reads the answer.
3. **Unattended, no `--answer`** — the request is *rejected*, and the CLI says
   why. Assuming consent when nobody is there to give it is the one thing this
   path must never do.

```
engine   fake 0.0.1 (protocol v1, loadSession=false)
session  sess_fake_1
modes    [work] plan
thought  Looking around
tool     [completed] List the folder (read)  /tmp/.../proj
plan     1 step(s)
           - [in_progress] Write notes.txt
answer   AllowOnce for Create notes.txt
tool     [completed] t2
text     Created notes.txt with one line.
done     end_turn
```

All three are pinned as tests in `crates/eavery-cli/tests/prompt.rs`, so the
exit test runs in CI rather than living in a shell history.

## Anything surprising

Three bugs the exit test found:

- `Connection::shutdown` deadlocked against the task waiting on the child,
  which held the child's mutex across `wait()`. The child now belongs to the
  waiter, and shutdown asks it rather than reaching for the same lock.
- `session/update` notifications were dispatched on a task each, so two updates
  could be delivered in either order. An ordered stream is the entire product of
  that layer. They are now dispatched inline on the reader task.
- The CLI printed the transcript from two independent writers — the event
  printer and the permission handler — so the answer to a permission request
  could appear above events the engine had sent before asking. One task now owns
  the transcript, and both producers feed it. Pinned by
  `the_transcript_is_in_order`.

The first two were fixed in M0-T06, the third in M0-T07.

## Not yet verified

CI on macOS and Windows. The workflow exists (`.github/workflows/ci.yml`) and
runs `fmt`, `clippy -D warnings` and `test` on all three, but this session only
has Linux. Nothing in the code is Linux-specific; the Windows-only paths are
`CREATE_NO_WINDOW` on spawn and `EXE_SUFFIX` when locating the fake agent.
Confirm on the first push and record here.
