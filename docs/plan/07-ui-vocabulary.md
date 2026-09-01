# 07 — Desktop UI and the Vocabulary Layer

## 1. Stack and structure

- React 18 + TypeScript + Vite, created with `pnpm create tauri-app` (template
  `react-ts`). No UI framework; plain CSS modules. State: `zustand` (one store).
- Tauri API: `@tauri-apps/api` (`invoke`, `listen`), plugins `dialog` (folder
  picker), `shell` (open links only), `store` is not used (settings live in
  Rust), `updater`, `process`, `log`.
- Folder layout:

```
apps/desktop/src/
├── main.tsx
├── app.tsx                 # routes: Onboarding | Home | Project | Settings
├── ipc.ts                  # typed wrappers for every command in 03-architecture §7
├── events.ts               # subscribes to core://event, feeds the store, handles seq gaps
├── store.ts                # zustand: projects, engines, sessions, turns, events, settings
├── vocab/
│   ├── dictionary.ts       # the Everyday/Developer dictionary (see §2)
│   └── t.ts                # t(key, vars?) reads mode from the store
├── screens/
│   ├── Onboarding.tsx
│   ├── Home.tsx            # project list, open folder
│   ├── Project.tsx         # three panes: Documents | Conversation | Activity
│   └── Settings.tsx        # engines, connectors, playbooks, mode toggle
├── components/
│   ├── Composer.tsx        # request box + "Plan it" / "Ask" buttons
│   ├── Transcript.tsx      # renders CoreEvents
│   ├── PlanCard.tsx        # plan review + approve/edit/cancel
│   ├── PermissionDialog.tsx
│   ├── Digest.tsx
│   ├── Checkpoints.tsx     # list + Undo/Redo/Go back here
│   ├── ToolCallRow.tsx     # Developer: raw; Everyday: one line, no tool names
│   ├── DocumentsPane.tsx   # file list with "changed in this run" markers
│   └── Diagnostics.tsx     # Developer only
└── styles/
```

Types shared with Rust: generate `src/types.ts` from the Rust types with
`ts-rs` (`#[derive(TS)]` on every serialised type in `eavery-core`) in a
`cargo test` that writes the bindings. Never hand-write the event types twice.

## 2. The dictionary

`vocab/dictionary.ts` is the only place engine vocabulary appears in the
frontend. Every key has both renderings.

```ts
export const dictionary = {
  project:        { everyday: "Project",            developer: "Repository" },
  documents:      { everyday: "Documents",          developer: "Files" },
  checkpoint:     { everyday: "Checkpoint",         developer: "Commit" },
  checkpoints:    { everyday: "History",            developer: "Commits" },
  undo:           { everyday: "Undo",               developer: "Revert to commit" },
  redo:           { everyday: "Redo",               developer: "Re-apply" },
  changes:        { everyday: "What changed",       developer: "Diff" },
  connector:      { everyday: "Connector",          developer: "MCP server" },
  connectors:     { everyday: "Connectors",         developer: "MCP servers" },
  playbook:       { everyday: "Playbook",           developer: "Skill" },
  playbooks:      { everyday: "Playbooks",          developer: "Skills" },
  engine:         { everyday: "Assistant",          developer: "Engine (ACP agent)" },
  plan:           { everyday: "Here's my plan",     developer: "Plan" },
  approve:        { everyday: "Go ahead",           developer: "Approve" },
  approveEdits:   { everyday: "Go ahead, with changes", developer: "Approve with edits" },
  cancel:         { everyday: "Not now",            developer: "Cancel" },
  working:        { everyday: "Working on it…",     developer: "Running" },
  thought:        { everyday: null,                 developer: "Thinking" },   // null = hidden
  toolRead:       { everyday: "Looked at {file}",   developer: "read {file}" },
  toolEdit:       { everyday: "Updated {file}",     developer: "edit {file}" },
  toolCreate:     { everyday: "Created {file}",     developer: "create {file}" },
  toolDelete:     { everyday: "Removed {file}",     developer: "delete {file}" },
  toolExecute:    { everyday: "Did a step in the background", developer: "exec: {title}" },
  toolFetch:      { everyday: "Looked something up online",   developer: "fetch {title}" },
  toolSearch:     { everyday: "Searched the documents",       developer: "search {title}" },
  permOutbound:   { everyday: "Eavery wants to send something outside this computer: {what}. This cannot be undone.", developer: "Outbound: {what}" },
  permDestructive:{ everyday: "Eavery wants to change something outside this Project: {what}. Eavery cannot undo that.", developer: "Destructive: {what}" },
  permExecute:    { everyday: "Eavery wants to run a step that it cannot fully explain: {what}", developer: "Execute: {what}" },
  allowOnce:      { everyday: "Allow this time",    developer: "Allow once" },
  allowAlways:    { everyday: "Always allow in this Project", developer: "Allow always" },
  reject:         { everyday: "Don't",              developer: "Reject" },
  digestTitle:    { everyday: "Done. Here's what happened", developer: "Turn summary" },
  digestUndo:     { everyday: "Undo all of this",   developer: "Revert to pre-turn commit" },
  errorGeneric:   { everyday: "That didn't work. {next}", developer: "{code}: {message}" },
  modeFast:       { everyday: "Fast",               developer: "model: {model}" },
  notProtected:   { everyday: "Not protected by Undo", developer: "Excluded from journal" },
} as const;
```

Rule enforced by an ESLint custom rule (or a simple script in CI,
`scripts/check-vocab.mjs`): files under `src/screens` and `src/components`
must not contain the literal words `commit`, `repo`, `repository`, `diff`,
`MCP`, `skill`, `stdout`, `stderr`, `stack trace`, `JSON`, `token` outside
`vocab/`. The check greps case-insensitively and fails CI.

## 3. Screens

### Home
List of Projects (name, folder path shortened, last used, engine). "Open a
folder" button (Tauri dialog, directory picker). Warning banner if the folder
is over the size guard. In Developer mode, show the Journal path.

### Project (three panes)
- **Left, Documents.** Flat-ish tree of the folder (collapsed by default
  beyond depth 2). Files changed in the current or last run get a dot.
  Clicking opens the file with the OS default app (`opener` plugin). Never
  render file contents in v1 except text diffs in the digest.
- **Centre, Conversation.** Transcript of `CoreEvent`s. Composer at the bottom
  with two buttons: primary "Plan it" (`mode: plan`) and secondary "Ask a
  question" (`mode: direct`, read-only intent). While a turn runs, the
  composer shows "Working on it…" with a Stop button.
- **Right, Activity.** In Everyday mode: a short trail of one-line tool
  events and the checkpoint list with Undo. In Developer mode: raw tool calls
  with kind/status/locations, permission decisions with `by`, plan entries,
  mode changes, and the Diagnostics tab.

### Plan card
Appears in the conversation when `PlanReady` arrives. Sections in Everyday
mode: summary; numbered steps; "Files it will change" (relative paths);
"Will leave this computer" (highlighted, or "Nothing" in green);
"Cannot be undone" (highlighted, or "Nothing"); "Will not do". Buttons per
dictionary. Developer mode also shows the raw plan markdown and the JSON.

### Permission dialog
Modal, one at a time, queue behind. Title from `permOutbound`/`permDestructive`/
`permExecute`. Body: the facts from `PermissionView.explanation` (paths,
hosts, connector name). Buttons per §2; "always" only where the decision
table allows. Destructive defaults focus to Reject.

### Digest
On `TurnFinished` with a digest: files added/changed/removed as lists with
counts; "Sent outside this computer" list; "Refused" list; one Undo button
that restores `undo_to`. If nothing changed: "Nothing changed in your files."

### Checkpoints
List with label, time, files changed. Selecting shows "Going back here would
change: ..." and a "Go back to this point" button. After a restore, a Redo
affordance appears for the previous HEAD. Locked files from a restore are
listed with the copy from `05-git-journal.md` §5.

### Settings
- **Mode**: Everyday / Developer toggle (persisted).
- **Assistant / Engines**: list from `list_engines` with status chips
  (Ready, Needs sign-in, Not installed, Not available), "Check again", and
  per-engine sign-in instructions. BYO-key entry for goose (stored in OS
  keychain through the `keyring` crate; never in SQLite). Ollama model picker.
- **Connectors**: add/edit/remove MCP servers (name, command, args, env,
  `outbound` flag with the sentence "Can this connector send information
  outside this computer?"). Built-in `eavery-docs` cannot be removed.
- **Playbooks**: list of discovered Playbooks (project and library) with
  name/description; "Open folder" button; no editor in v1.
- **Diagnostics** (Developer): log tail, copy button, data dir path.

## 4. UI rules

1. **Hidden, not removed.** Every Developer element exists in the DOM tree
   behind `mode === "developer"`; do not create parallel components.
2. **Errors are next actions.** `Error` events render `next_action` as the
   headline in Everyday mode; the message and code are in an expandable line
   in Developer mode only.
3. **Never block on streaming.** Text chunks append to the current agent
   message; do not re-render the whole transcript per chunk (keyed list,
   append-only reducer).
4. **Sequence gaps.** If a `core://event` arrives with `seq > last + 1`, call
   `list_events({after: last})` and merge.
5. **Keyboard.** Enter sends, Shift+Enter newline, Esc closes dialogs,
   Cmd/Ctrl+Z in the Project screen opens the Undo confirmation for the last turn.
6. **No spinners longer than 2 s without words.** "Working on it…" plus the
   latest activity line.
7. **Accessibility baseline.** Buttons are buttons, dialogs trap focus,
   contrast 4.5:1, everything reachable by keyboard.

## 5. Everyday-mode copy for engine states

| EngineStatus | Copy |
|---|---|
| NotInstalled | "{engine} isn't installed on this computer. {instructions}" |
| NeedsSignIn | "{engine} needs you to sign in. Open Terminal and run `{command}`, then check again." (the command is the one thing shown verbatim) |
| Unavailable(reason) | "{engine} isn't available right now. You can switch to another assistant." + reason in Developer mode |
| Ready | "Ready" |
