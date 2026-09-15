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

## CI on all three platforms

First run: <https://github.com/dmagyar-0/Eavery/actions/runs/34993581803>.
Linux and macOS green. **Windows failed**, and found a real bug rather than a
test artifact:

`std::fs::canonicalize` returns a verbatim path (`\\?\C:\...`) on Windows.
That was the session `cwd`, so it went over the wire to the engine, which joined
`{{cwd}}/notes.txt` onto it — and in a verbatim path a forward slash is not a
separator, so the write landed nowhere. Two CLI tests failed; Linux and macOS
could not have caught it.

This is the hazard `06-plan-gate-permissions.md` §3.1 warns about, one layer
earlier than the plan expects it. Fixed by `eavery-core::paths` over `dunce`,
with the ACP layer and the CLI routed through it, and guarded by two tests that
are meaningful only on Windows. Recorded in `CHANGELOG-plan.md`.
