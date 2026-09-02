# Backlog (not v1)

Ideas and requests that are out of scope for v1. Do not implement anything
here without moving it into `10-task-breakdown.md` with a milestone.

- Playbook registry and sharing (agentskills.io-compatible)
- Scheduled and event-triggered runs
- `.pptx` writing
- Visual diff for `.docx`/`.xlsx` (tracked-changes style)
- Email, calendar, Slack, Drive connectors shipped in-box
- Team policy sync, SSO, audit export
- Contributing the Tauri shell upstream to goose
- Apache 2.0 relicensing decision
- Bundling a Node runtime so the Claude and Gemini adapters are terminal-free too
- Driving `claude -p --input-format stream-json` directly as a Node-free Claude path (same Anthropic billing bucket as ACP, so no policy gain; only an install gain)
- User-initiated Journal pruning ("forget history older than N days"); must reconcile with D5 and never be automatic
- Chart-aware `.xlsx` editing (preserve charts, pivots, conditional formatting on write)
- Localisation of the Everyday dictionary
- Crash reporting / opt-in telemetry decision
