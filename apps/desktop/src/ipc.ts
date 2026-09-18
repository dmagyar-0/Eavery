// One typed function per command (`docs/plan/03-architecture.md` §7).
//
// Nothing else in the frontend calls `invoke`. Keeping it to one file means
// the argument names — which Tauri converts from camelCase to the Rust
// snake_case — are written once, and a command that is renamed breaks here
// rather than in a screen.

import { invoke } from "@tauri-apps/api/core";
import type {
  AppError,
  AuditEntry,
  ChangeSet,
  Checkpoint,
  Decision,
  Diagnostics,
  EngineListing,
  EngineStatus,
  JournalInfo,
  Project,
  RestoreOutcome,
  Session,
  Settings,
  StoredEvent,
  Turn,
  Unprotected,
} from "./types";

/** Which loop a turn runs. `plan` is refused until the plan gate exists. */
export type TurnMode = "direct" | "plan";

/**
 * Every command fails as an `AppError`: a code, a message, and — usually —
 * the next action. This is what tells them apart from a thrown string.
 */
export function isAppError(error: unknown): error is AppError {
  return (
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    "message" in error
  );
}

/** The message to show a person, and what they can do about it. */
export function explain(error: unknown): {
  message: string;
  nextAction: string | null;
} {
  if (isAppError(error)) {
    return { message: error.message, nextAction: error.next_action };
  }
  return { message: String(error), nextAction: null };
}

// ---- projects --------------------------------------------------------------

export const listProjects = () => invoke<Project[]>("list_projects");

/** Opens a folder as a Project, taking its first checkpoint if it is new. */
export const openProject = (path: string) =>
  invoke<Project>("open_project", { path });

/** Forgets a Project. Deletes neither the folder nor its history. */
export const removeProject = (projectId: string) =>
  invoke<void>("remove_project", { projectId });

export const setProjectEngine = (projectId: string, engineId: string) =>
  invoke<void>("set_project_engine", { projectId, engineId });

// ---- engines ---------------------------------------------------------------

export const listEngines = (all = false) =>
  invoke<EngineListing[]>("list_engines", { all });

/** `deep` sends a real prompt, which costs the person a request and a wait. */
export const runHealthCheck = (engineId: string, deep = false) =>
  invoke<EngineStatus>("run_health_check", { engineId, deep });

// ---- turns -----------------------------------------------------------------

/**
 * Starts a turn and answers with its id. Everything it then does arrives as
 * events; a Project that is already working answers with an error instead.
 */
export const startTurn = (
  projectId: string,
  request: string,
  mode: TurnMode = "direct",
) => invoke<string>("start_turn", { projectId, request, mode });

export const answerPermission = (requestId: string, decision: Decision) =>
  invoke<void>("answer_permission", { requestId, decision });

export const cancelTurn = (turnId: string) =>
  invoke<void>("cancel_turn", { turnId });

export const listTurns = (sessionId: string) =>
  invoke<Turn[]>("list_turns", { sessionId });

// ---- history ---------------------------------------------------------------

export const listCheckpoints = (projectId: string, limit?: number) =>
  invoke<Checkpoint[]>("list_checkpoints", { projectId, limit });

export const checkpointNow = (projectId: string, label: string) =>
  invoke<Checkpoint>("checkpoint_now", { projectId, label });

/**
 * Goes back. The answer names any file something else held open, which the
 * UI has to show: a partial restore nobody is told about is worse than one
 * that failed.
 */
export const restoreCheckpoint = (projectId: string, checkpointId: string) =>
  invoke<RestoreOutcome>("restore_checkpoint", { projectId, checkpointId });

/** With no `to`, what has changed since `from` — including the user's own edits. */
export const diffSummary = (projectId: string, from: string, to?: string) =>
  invoke<ChangeSet>("diff_summary", { projectId, from, to });

export const listSessions = (projectId: string) =>
  invoke<Session[]>("list_sessions", { projectId });

/** `after` is the last `seq` already held, so this returns only what is missing. */
export const listEvents = (
  sessionId: string,
  after?: number,
  limit?: number,
) => invoke<StoredEvent[]>("list_events", { sessionId, after, limit });

export const listAudit = (projectId?: string, limit?: number) =>
  invoke<AuditEntry[]>("list_audit", { projectId, limit });

export const journalSize = (projectId: string) =>
  invoke<number>("journal_size", { projectId });

/** Where the history lives and how big it is. Developer mode shows it. */
export const journalInfo = (projectId: string) =>
  invoke<JournalInfo>("journal_info", { projectId });

export const unprotectedFiles = (projectId: string) =>
  invoke<Unprotected[]>("unprotected_files", { projectId });

// ---- settings --------------------------------------------------------------

export const getSettings = () => invoke<Settings>("get_settings");

export const setSettings = (settings: Settings) =>
  invoke<void>("set_settings", { settings });

// ---- diagnostics -----------------------------------------------------------

/** The version, the folders, and the last `lines` lines of the log. */
export const diagnostics = (lines?: number) =>
  invoke<Diagnostics>("diagnostics", { lines });
