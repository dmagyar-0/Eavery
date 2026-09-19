# Fake-agent scripts

Shared by the `eavery-core`, `eavery-cli` and (from M3) desktop tests, per
`docs/plan/11-testing-ci.md` §2, which also documents the format.

`hello.json` is the demo the README runs: one turn with a thought, a read, a
plan update, a permission request expecting `allow_once`, a write through the
client, and a closing message.

`plan.json` is the plan gate end to end (`docs/plan/06-plan-gate-permissions.md`
§7, tests 1 and 2): a planning turn that reads, is refused an edit and a
`fs/write_text_file`, is refused leaving plan mode, and ends with an
`eavery-plan` block; then an execute turn that asks, is allowed, and writes.
The script asserts the gate from its own side — every `expect` and
`expect_refused` fails the run if the client answers otherwise — so it needs
`report.txt` in the Project folder, a request the plan prompt carries, and
`--plan` (or `mode: "plan"`).
