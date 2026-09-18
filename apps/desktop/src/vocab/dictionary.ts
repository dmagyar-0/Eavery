// The Everyday/Developer dictionary (`docs/plan/07-ui-vocabulary.md` §2).
//
// This is the only file in the frontend where engine vocabulary appears.
// Every key has both renderings; `null` means the thing is not shown at all
// in that mode. Screens and components never hold a user-facing string of
// their own — they ask `t()` — so the mode toggle is one lookup away from
// every word on screen, and the words a non-technical person must never see
// (commit, repository, stderr…) live here and nowhere else.
//
// M5-T01 completes this per §2; the keys the M3 screens need are here now so
// that no string has to be moved later.

export type Rendering = { everyday: string | null; developer: string | null };

/** The same words in both modes. */
const same = (text: string): Rendering => ({ everyday: text, developer: text });

export const dictionary = {
  // ---- nouns (§2) ----------------------------------------------------------
  project: { everyday: "Project", developer: "Repository" },
  projects: { everyday: "Projects", developer: "Repositories" },
  documents: { everyday: "Documents", developer: "Files" },
  checkpoint: { everyday: "Checkpoint", developer: "Commit" },
  checkpoints: { everyday: "History", developer: "Commits" },
  undo: { everyday: "Undo", developer: "Revert to commit" },
  redo: { everyday: "Redo", developer: "Re-apply" },
  changes: { everyday: "What changed", developer: "Diff" },
  connector: { everyday: "Connector", developer: "MCP server" },
  connectors: { everyday: "Connectors", developer: "MCP servers" },
  playbook: { everyday: "Playbook", developer: "Skill" },
  playbooks: { everyday: "Playbooks", developer: "Skills" },
  engine: { everyday: "Assistant", developer: "Engine (ACP agent)" },
  engines: { everyday: "Assistants", developer: "Engines (ACP agents)" },
  plan: { everyday: "Here's my plan", developer: "Plan" },
  approve: { everyday: "Go ahead", developer: "Approve" },
  approveEdits: { everyday: "Go ahead, with changes", developer: "Approve with edits" },
  cancel: { everyday: "Not now", developer: "Cancel" },
  working: { everyday: "Working on it…", developer: "Running" },
  thought: { everyday: null, developer: "Thinking" },
  toolRead: { everyday: "Looked at {file}", developer: "read {file}" },
  toolEdit: { everyday: "Updated {file}", developer: "edit {file}" },
  toolCreate: { everyday: "Created {file}", developer: "create {file}" },
  toolDelete: { everyday: "Removed {file}", developer: "delete {file}" },
  toolMove: { everyday: "Moved {file}", developer: "move {file}" },
  toolExecute: { everyday: "Did a step in the background", developer: "exec: {title}" },
  toolFetch: { everyday: "Looked something up online", developer: "fetch {title}" },
  toolSearch: { everyday: "Searched the documents", developer: "search {title}" },
  toolThink: { everyday: null, developer: "think: {title}" },
  toolOther: { everyday: "Did a step: {title}", developer: "{kind}: {title}" },
  permOutbound: {
    everyday:
      "Eavery wants to send something outside this computer: {what}. This cannot be undone.",
    developer: "Outbound: {what}",
  },
  permDestructive: {
    everyday:
      "Eavery wants to change something outside this Project: {what}. Eavery cannot undo that.",
    developer: "Destructive: {what}",
  },
  permExecute: {
    everyday: "Eavery wants to run a step that it cannot fully explain: {what}",
    developer: "Execute: {what}",
  },
  permOther: {
    everyday: "Eavery wants to do something it has to ask about: {what}",
    developer: "{risk}: {what}",
  },
  permInPlan: { everyday: "This was in the plan.", developer: "This was in the plan." },
  permNotInPlan: {
    everyday: "This was NOT in the plan.",
    developer: "This was NOT in the plan.",
  },
  allowOnce: { everyday: "Allow this time", developer: "Allow once" },
  allowAlways: { everyday: "Always allow in this Project", developer: "Allow always" },
  reject: { everyday: "Don't", developer: "Reject" },
  digestTitle: { everyday: "Done. Here's what happened", developer: "Turn summary" },
  digestUndo: { everyday: "Undo all of this", developer: "Revert to pre-turn commit" },
  errorGeneric: { everyday: "That didn't work. {next}", developer: "{code}: {message}" },
  modeFast: { everyday: "Fast", developer: "model: {model}" },
  notProtected: { everyday: "Not protected by Undo", developer: "Excluded from journal" },

  // ---- the shell -----------------------------------------------------------
  appName: same("Eavery"),
  navHome: same("Home"),
  navSettings: same("Settings"),
  loading: same("Loading…"),
  dismiss: same("Dismiss"),
  close: same("Close"),
  refresh: same("Refresh"),
  troubleHeadline: { everyday: "That didn't work.", developer: "Error" },
  troubleDetail: { everyday: null, developer: "{message}" },

  // ---- Home ----------------------------------------------------------------
  homeTitle: { everyday: "Your Projects", developer: "Repositories" },
  homeLead: {
    everyday:
      "Open a folder and Eavery protects it from that moment: every change is checkpointed before it happens, and one button takes it back.",
    developer:
      "Opening a folder creates a detached journal for it under the data directory. The folder itself gets no .git.",
  },
  homeEmpty: { everyday: "No Projects yet.", developer: "No repositories yet." },
  openFolder: same("Open a folder…"),
  forgetProject: same("Forget"),
  forgetProjectHint: {
    everyday: "Removes it from this list. Your files and their history stay where they are.",
    developer: "Removes the row. The folder and its journal are kept.",
  },
  projectAdded: same("Added {when}"),
  projectEngine: { everyday: "Assistant: {engine}", developer: "engine: {engine}" },
  projectEngineDefault: same("Default"),
  journalPath: { everyday: null, developer: "journal: {path}" },
  journalSize: { everyday: "History takes up {size}", developer: "journal: {size}, {loose} loose objects" },

  // ---- Project -------------------------------------------------------------
  paneDocuments: { everyday: "Documents", developer: "Files" },
  paneConversation: { everyday: "Conversation", developer: "Transcript" },
  paneActivity: same("Activity"),
  tabActivity: same("Activity"),
  tabHistory: { everyday: "History", developer: "Commits" },
  tabDiagnostics: { everyday: null, developer: "Diagnostics" },
  showFolder: same("Show folder"),
  openDocument: same("Open"),
  changedLastRun: { everyday: "Changed in the last run", developer: "Changed by the last turn" },
  noChangesYet: { everyday: "Nothing has been changed yet.", developer: "No turn has changed anything yet." },
  noActivityYet: { everyday: "Nothing has happened yet.", developer: "No tool calls yet." },
  notProtectedWhy_too_large: { everyday: "too big", developer: "over the size limit" },
  notProtectedWhy_not_downloaded: { everyday: "not downloaded from the cloud yet", developer: "cloud placeholder" },
  ownEditsNote: {
    everyday:
      "Your own edits between runs are protected too: the next run checkpoints them before it starts.",
    developer: "The pre-turn commit captures the work tree, including edits made outside Eavery.",
  },

  // ---- Composer ------------------------------------------------------------
  composerPlaceholder: {
    everyday: "What would you like done in this Project?",
    developer: "Prompt",
  },
  composerHint: same("Enter to send, Shift+Enter for a new line"),
  planIt: same("Plan it"),
  planItSoon: {
    everyday: "Planning first arrives soon. For now, ask directly.",
    developer: "The plan gate is M4. Direct mode only.",
  },
  askDirect: { everyday: "Ask", developer: "Run" },
  stop: same("Stop"),
  noSessionYet: {
    everyday: "Nothing has been asked in this Project yet.",
    developer: "No session for this repository yet.",
  },

  // ---- Transcript ----------------------------------------------------------
  you: same("You"),
  assistant: { everyday: "Eavery", developer: "Agent" },
  turnPhase: { everyday: null, developer: "phase: {phase}" },
  turnFinished: { everyday: "Finished", developer: "turn finished: {reason}" },
  turnStopped: { everyday: "Stopped", developer: "turn cancelled" },
  turnFailed: { everyday: "Stopped early", developer: "turn failed" },
  planEntries: { everyday: "Steps", developer: "Plan entries" },
  planReady: { everyday: "Here's my plan", developer: "Plan ready" },
  checkpointTaken: { everyday: "Checkpoint: {label}", developer: "commit: {label}" },
  restoredTo: { everyday: "Went back to: {label}", developer: "restored to {label} ({to})" },
  engineStatusLine: { everyday: null, developer: "engine {engine}: {state}" },
  engineCrashed: {
    everyday: "The assistant stopped unexpectedly.",
    developer: "engine {engine} crashed",
  },
  engineOutput: { everyday: "What it reported last", developer: "stderr tail" },
  permissionAsked: { everyday: "Asked: {what}", developer: "permission requested: {what}" },
  permissionDecided: {
    everyday: "{decision}",
    developer: "{decision} (by {by})",
  },
  decision_allow_once: { everyday: "Allowed this time", developer: "allow_once" },
  decision_allow_always: { everyday: "Always allowed", developer: "allow_always" },
  decision_reject_once: { everyday: "Not allowed", developer: "reject_once" },
  decision_reject_always: { everyday: "Never allowed", developer: "reject_always" },
  decision_cancelled: { everyday: "Not answered", developer: "cancelled" },
  by_policy: { everyday: "automatically", developer: "policy" },
  by_user: { everyday: "by you", developer: "user" },
  by_plan_gate: { everyday: "by the plan", developer: "plan_gate" },
  toolStatus: { everyday: null, developer: "{status}" },
  toolLocations: { everyday: null, developer: "{locations}" },
  toolRisk: { everyday: null, developer: "risk: {risk}" },

  // ---- Digest --------------------------------------------------------------
  digestNothing: { everyday: "Nothing changed in your files.", developer: "No file changes." },
  digestAdded: same("Added ({count})"),
  digestChanged: same("Changed ({count})"),
  digestRemoved: same("Removed ({count})"),
  digestOutbound: { everyday: "Sent outside this computer", developer: "Outbound" },
  digestRefused: same("Refused"),
  nothing: same("Nothing"),

  // ---- Permission dialog ---------------------------------------------------
  permissionTitle: { everyday: "Eavery is asking", developer: "Permission request" },
  permissionWhere: { everyday: "It concerns", developer: "locations" },
  permissionQueued: same("{count} more waiting"),
  permissionNoOptions: {
    everyday: "The assistant offered no way to answer this.",
    developer: "The engine sent no options.",
  },

  // ---- Checkpoints ---------------------------------------------------------
  goBackHere: same("Go back to this point"),
  goingBackWouldChange: same("Going back here would change:"),
  goingBackNothing: same("Going back here would change nothing."),
  goingBackChecking: same("Checking…"),
  undoLastRun: { everyday: "Undo the last run", developer: "Revert to the last pre-turn commit" },
  redoLastUndo: { everyday: "Redo", developer: "Re-apply what was undone" },
  filesChanged: same("{count} files"),
  fileChangedOne: same("1 file"),
  noCheckpoints: same("No checkpoints yet."),
  checkpointKind_pre_turn: { everyday: "before", developer: "pre_turn" },
  checkpointKind_post_turn: { everyday: "after", developer: "post_turn" },
  checkpointKind_manual: { everyday: "saved", developer: "manual" },
  checkpointKind_restore: { everyday: "went back", developer: "restore" },
  protectNow: { everyday: "Save a point to come back to", developer: "Commit now" },
  protectNowLabel: { everyday: "Saved by you", developer: "Manual commit" },
  lockedFiles: {
    everyday: "These files were open, so they were left as they are: {files}",
    developer: "Skipped, held open elsewhere: {files}",
  },
  lockedFilesNext: same("Close them and go back again."),
  undoConfirmTitle: { everyday: "Undo the last run?", developer: "Revert to the pre-turn commit?" },
  undoConfirmBody: {
    everyday: "Your files go back to how they were before “{request}”. You can redo this afterwards.",
    developer: "Restores {checkpoint}. A restore commit is added; nothing is rewritten.",
  },

  // ---- Settings ------------------------------------------------------------
  settingsTitle: same("Settings"),
  settingsMode: same("Mode"),
  modeEveryday: same("Everyday"),
  modeDeveloper: same("Developer"),
  modeHint: {
    everyday: "Everyday keeps the words plain. Developer shows what is happening underneath.",
    developer: "Developer mode shows raw tool calls, decisions with their actor, and the log.",
  },
  settingsEngines: { everyday: "Assistants", developer: "Engines" },
  settingsEnginesLead: {
    everyday: "Eavery drives the assistant you already pay for. Pick which one new Projects start with.",
    developer: "Every engine in the table, with what a shallow health check said. Answers are cached for ten minutes.",
  },
  defaultEngine: { everyday: "New Projects use", developer: "Default engine" },
  noDefaultEngine: same("Whichever is first"),
  checkAgain: same("Check again"),
  checking: same("Checking…"),
  experimentalTag: same("experimental"),
  programPath: { everyday: null, developer: "program: {path}" },
  engineState_not_installed: same("Not installed"),
  engineState_needs_node: same("Needs Node.js"),
  engineState_needs_sign_in: same("Needs sign-in"),
  engineState_installing: same("Downloading"),
  engineState_signing_in: same("Signing in"),
  engineState_ready: same("Ready"),
  engineState_unavailable: same("Not available"),
  engineCopy_not_installed: same("{engine} isn't installed on this computer. {instructions}"),
  engineCopy_needs_node: same(
    "{engine} needs Node.js, which isn't installed. Install it from https://nodejs.org, or use ChatGPT (Codex) instead, which Eavery can set up for you.",
  ),
  engineCopy_needs_sign_in: same(
    "{engine} needs you to sign in. Open Terminal and run the command below, then check again.",
  ),
  engineCopy_unavailable: {
    everyday: "{engine} isn't available right now. You can switch to another assistant.",
    developer: "{engine} isn't available right now: {reason}",
  },
  engineCopy_ready: same("Ready"),
  engineCopy_installing: same("Downloading {engine}… {percent}%"),
  engineCopy_signing_in: same("Finish signing in to {engine} in your browser, then come back here."),
  engineVersion: { everyday: null, developer: "{name} {version}, protocol {protocol}" },
  engineModes: { everyday: null, developer: "modes: {modes}" },

  // ---- Diagnostics ---------------------------------------------------------
  diagnosticsTitle: { everyday: null, developer: "Diagnostics" },
  diagnosticsVersion: same("Version {version}"),
  diagnosticsDataDir: same("Data folder: {path}"),
  diagnosticsLogPath: same("Log: {path}"),
  diagnosticsCopy: same("Copy diagnostics"),
  diagnosticsCopied: same("Copied"),
  diagnosticsCopyFailed: same("Select the text below and copy it"),
  diagnosticsEmpty: same("Nothing has been logged yet."),
  diagnosticsLogTail: { everyday: null, developer: "Last {count} lines of the log" },
} as const satisfies Record<string, Rendering>;

export type Key = keyof typeof dictionary;
