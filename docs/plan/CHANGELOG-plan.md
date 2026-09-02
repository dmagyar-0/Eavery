# Plan changelog

Record here every place where implementation reality differed from the plan
documents, and every locked decision that had to change, with the reason.
Newest first.

| Date | Task | What differed | What was done |
|---|---|---|---|
| 2026-09-01 | (review) | `REVIEW-2026-09.md` found spec bugs and inconsistencies across the plan | Applied: D15 (reads allowed outside Project), D16 (checkpoint before restore), `PlanJson` wire struct, `Engine` trait on `&self`, `plan_mode_hint` + plan-mode-exit refusal, `@agentclientprotocol/codex-acp` (Zed package deprecated), `agent-client-protocol-schema = "1"`, Codex CLI + codex-acp download and in-app `codex login`, shallow health check by default, exclude list additions, cloud-placeholder guard, C11–C13 added (prompt injection, Journal growth, concurrency), M5/M6 exit tests swapped, S0 spikes added, vocab lint restricted to strings, `opener` plugin, Ctrl+Z scoping, message-coalesced resume summary. |
