# 08 — Onboarding, Engine Detection, Packaging

First run is a P0 engineering problem (`docs/02-building-blocks.md` §3). This
document specifies it precisely.

## 1. First-run flow

```
Welcome ─▶ Detecting assistants… ─▶ [found ≥1 zero-key engine] ─▶ Pick one ─▶ Health check ─▶ Open a folder ─▶ Done
                                  └▶ [found none] ─▶ Choose: "I have Claude/ChatGPT/Gemini" | "I have an API key" | "Keep everything on this computer"
                                                       │                                    │                          │
                                                       ▼                                    ▼                          ▼
                                             Install instructions +               Download goose ─▶ paste key    Download goose ─▶ detect Ollama ─▶ pick model
                                             "Check again"                         ─▶ health check                 ─▶ health check
```

Rules:
- Detection runs in the background while the welcome screen shows; the user
  never waits on a blank screen.
- Never ask for an API key on the first screen. The key path is the third
  option and is labelled honestly: "Pay per use with your own key".
- Every path ends in the same health check and the same "Open a folder" step.
- The whole flow is re-enterable from Settings → Assistants.

## 2. Engine detection

For each `EngineSpec`, resolve the executable:

1. `settings.engine_paths[id]` if set.
2. `which`-style search over the fixed PATH (see `02-challenges.md` C5;
   use the `which` crate after applying `fix-path-env`).
3. Well-known locations:

| Platform | Locations checked |
|---|---|
| macOS | `/opt/homebrew/bin`, `/usr/local/bin`, `~/.local/bin`, `~/.claude/local`, `~/.volta/bin`, `~/.nvm/versions/node/*/bin`, `~/.cargo/bin`, `~/.bun/bin` |
| Linux | `/usr/local/bin`, `~/.local/bin`, `~/.claude/local`, `~/.nvm/versions/node/*/bin`, `~/.volta/bin`, `~/.cargo/bin`, `/snap/bin` |
| Windows | `%LOCALAPPDATA%\Programs\*`, `%APPDATA%\npm`, `%USERPROFILE%\.claude\local`, `%USERPROFILE%\.cargo\bin`, `%ProgramFiles%\nodejs`, `%LOCALAPPDATA%\Microsoft\WinGet\Links` |

For npm-distributed adapters (`claude-agent-acp`, `codex-acp` via npm), check
for the global binary first; if absent but `node`/`npx` is present, launch via
`npx -y <package>` and warn that first start may take a minute (npm download).
Also check whether the underlying CLI exists (`claude`, `codex`, `gemini`)
because the adapter is useless without a login.

Detection results are cached for the session and refreshed by "Check again".

## 3. Sign-in instructions (shown verbatim)

| Engine | Instruction |
|---|---|
| claude | "Install Claude Code from https://claude.com/claude-code, open Terminal, run `claude` once and sign in. Then install the bridge: `npm install -g @agentclientprotocol/claude-agent-acp`." |
| codex | "Install Codex CLI (`npm install -g @openai/codex`), run `codex login` and sign in with ChatGPT. Then install the bridge: `npm install -g @zed-industries/codex-acp`." |
| gemini | "Install Gemini CLI (`npm install -g @google/gemini-cli`), run `gemini` once and sign in with Google." |
| goose (key) | Handled inside Eavery: download, paste key. |
| goose-local | "Install Ollama from https://ollama.com and pull a model, for example `ollama pull qwen3:8b`." (Do not hardcode a model that may not exist; read the list from Ollama and offer the installed ones.) |

Keep these strings in one file, `crates/eavery-engines/src/instructions.rs`,
so they can be updated without touching UI code.

## 4. goose download

- Source: GitHub releases of the goose project (Apache 2.0). Record the exact
  release URL pattern and SHA-256 checksums for the pinned version in
  `crates/eavery-engines/src/goose_release.rs`. Pin one version; bump
  deliberately.
- Download to `<data_dir>/engines/goose/<version>/`, verify checksum, set
  executable bit, run `goose --version` to confirm.
- On macOS, the downloaded binary will be quarantined; run
  `xattr -d com.apple.quarantine <path>` or the user gets a Gatekeeper
  dialog. Test this on a clean Mac.
- Keys entered by the user go to the OS keychain via the `keyring` crate
  under service `dev.eavery.Eavery`, account `<provider>`. They are passed to
  the goose child as env vars and never written to disk by Eavery.
- Offline installers (regulated environments) can bundle goose as a Tauri
  sidecar instead (`bundle.externalBin`); keep both code paths behind one
  `GooseSource` enum.

## 5. Health check UX

Run `run_health_check` (`04-acp-engines.md` §9) with a progress line per
step: "Starting…", "Connected", "Signed in", "Answered". Failure copy per
`07-ui-vocabulary.md` §5. A health check that passes writes
`EngineStatus::Ready` with timestamp; the Project screen shows "Ready" and
does not re-check for 10 minutes.

## 6. Packaging

- `tauri.conf.json`: `identifier` `dev.eavery.Eavery`; `bundle.targets`
  `["dmg","app","msi","nsis","appimage","deb"]`; `plugins.updater` with a
  public key generated by `tauri signer generate` (private key in CI secrets
  only); `bundle.externalBin` includes `eavery-docs-mcp` (the MCP server binary
  is always bundled; goose is downloaded).
- macOS: sign and notarise in CI (Apple Developer ID). Until credentials
  exist, ship unsigned builds labelled "dev" and document the
  right-click → Open workaround in the README. Do not silently ship unsigned.
- Windows: MSI via WiX (Tauri default). Code signing later; document the
  SmartScreen prompt.
- Linux: AppImage and deb. AppImage must not be sandboxed (needs to spawn
  user binaries).
- Release workflow: tag `v*` → GitHub Actions matrix build → attach installers
  and `latest.json` for the updater.
- App size target: under 30 MB per platform without goose.

## 7. Durability across restart

- On startup, load Projects and the last open Project; render transcript from
  `list_events`.
- Re-spawn the engine lazily on the first new request, not at startup.
- If the engine reported `loadSession: true` and `session.engine_session_id`
  exists, call `session/load`; the replayed `user_message_chunk` /
  `agent_message_chunk` updates are ignored for the transcript (already in
  SQLite) but the session id is reused. Otherwise open a new session and
  prepend a summary built from the last 20 `AgentText` events, capped at 2,000
  characters, to the first prompt as "Context from the previous conversation".
- If the app was closed mid-turn, the turn is marked `Failed` with next action
  "Eavery was closed while working. Your files are protected; check History."
  and the post-checkpoint is taken at next open.
