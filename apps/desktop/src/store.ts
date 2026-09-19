// What the window is currently showing.
//
// A plain observable object, read through `useSyncExternalStore`. There is no
// state library here on purpose: the only state the frontend has is a copy of
// what the core just said, and the core is the one that decides things
// (`docs/plan/03-architecture.md` §1). Anything that looks like a rule
// appearing in this file is a rule in the wrong crate.

import { useSyncExternalStore } from "react";
import * as ipc from "./ipc";
import * as os from "./os";
import { createFeed, type Feed } from "./events";
import { t } from "./vocab/t";
import type {
  Checkpoint,
  Diagnostics,
  EngineListing,
  EngineStatus,
  JournalInfo,
  PermissionView,
  Plan,
  Project,
  Settings,
  StoredEvent,
  Turn,
  Unprotected,
} from "./types";

export type Trouble = {
  message: string;
  nextAction: string | null;
  /** The error code, when there was one. Developer mode shows it. */
  code?: string;
};

/** Which screen is up (`07-ui-vocabulary.md` §1). Onboarding is M7. */
export type Screen = "home" | "project" | "settings";

/** What Redo would go back to: where the files were just before the last restore. */
export type RedoPoint = { to: string; label: string };

/** A plan waiting for the person's answer, and who saw their documents to make it. */
export type PlanReview = { turnId: string; plan: Plan; vendor: string };

export type State = {
  /** True until the first load finishes, so the window can say nothing rather than "no projects". */
  loading: boolean;
  screen: Screen;
  projects: Project[];
  projectId: string | null;
  /** The conversation on screen, if the Project has had one. */
  sessionId: string | null;
  transcript: StoredEvent[];
  /** The conversation's turns, oldest first: what was asked, and what each one can be undone to. */
  turns: Turn[];
  checkpoints: Checkpoint[];
  /** What Undo does not cover in this Project, and why. */
  unprotected: Unprotected[];
  /** Where the Project's history is and how big it has got. Developer mode shows it. */
  journal: JournalInfo | null;
  /** Journal descriptions for the Home screen, by Project id. Developer mode only. */
  journals: Record<string, JournalInfo>;
  redo: RedoPoint | null;
  engines: EngineListing[];
  /** Engines whose health check is running right now. */
  checkingEngines: string[];
  settings: Settings;
  /** The turn running right now, if any. Stop is the only thing to do while it is. */
  turnId: string | null;
  /** The plan the running turn is waiting on, if it is (`AwaitingApproval`). */
  reviewing: PlanReview | null;
  /** Questions waiting for an answer, oldest first: one dialog at a time. */
  asking: PermissionView[];
  /** The last thing that went wrong, as something to do about it. */
  trouble: Trouble | null;
  diagnostics: Diagnostics | null;
};

const initial: State = {
  loading: true,
  screen: "home",
  projects: [],
  projectId: null,
  sessionId: null,
  transcript: [],
  turns: [],
  checkpoints: [],
  unprotected: [],
  journal: null,
  journals: {},
  redo: null,
  engines: [],
  checkingEngines: [],
  settings: { mode: "everyday", default_engine: null },
  turnId: null,
  reviewing: null,
  asking: [],
  trouble: null,
  diagnostics: null,
};

let state: State = initial;
const listeners = new Set<() => void>();

function set(changes: Partial<State>) {
  state = { ...state, ...changes };
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** The whole state. Components pick what they need out of it. */
export function useStore(): State {
  return useSyncExternalStore(subscribe, () => state);
}

/** Reports a failure as the next action, and keeps the window usable. */
function stumbled(error: unknown) {
  const { message, nextAction } = ipc.explain(error);
  const code = ipc.isAppError(error) ? error.code : undefined;
  set({ trouble: { message, nextAction, code } });
}

export function dismissTrouble() {
  set({ trouble: null });
}

export function go(screen: Screen) {
  set({ screen });
}

// ---- the event feed --------------------------------------------------------

let feed: Feed | null = null;

/**
 * The highest `seq` whose consequences have already been applied.
 *
 * Kept by `seq` rather than by counting: a refetch can add events *before*
 * ones already held, and a permission request reacted to twice would put two
 * dialogs on screen for one question.
 */
let reactedTo = 0;

/** Applies what an event means for the window, beyond appearing in the transcript. */
function react(event: StoredEvent) {
  const core = event.event;
  switch (core.type) {
    case "turn_started":
      set({ turnId: core.turn_id, redo: null, reviewing: null });
      void refreshTurns();
      break;
    case "plan_ready":
      set({ reviewing: { turnId: core.turn_id, plan: core.plan, vendor: core.vendor } });
      void refreshTurns();
      break;
    case "phase_changed":
      // The plan is answered once the turn moves on from waiting.
      if (core.phase !== "awaiting_approval") set({ reviewing: null });
      void refreshTurns();
      break;
    case "turn_finished":
      set({ turnId: null, asking: [], reviewing: null });
      void refreshCheckpoints();
      void refreshTurns();
      void refreshProtection();
      break;
    case "permission_requested":
      set({ asking: [...state.asking, core.request] });
      break;
    case "permission_resolved":
      set({
        asking: state.asking.filter(
          (request) => request.request_id !== core.request_id,
        ),
      });
      break;
    case "checkpoint_created":
    case "restored":
      void refreshCheckpoints();
      break;
    case "engine_crashed":
      set({ turnId: null, asking: [], reviewing: null });
      break;
    case "error":
      set({
        trouble: {
          message: core.message,
          nextAction: core.next_action,
          code: core.code,
        },
      });
      break;
    default:
      break;
  }
}

/** Starts the app: the settings, the projects, and the event stream. */
export async function start() {
  feed ??= createFeed({
    onChange: (sessionId) => {
      if (sessionId !== state.sessionId) return;
      const transcript = feed?.events(sessionId) ?? [];
      // Anything the new events mean for the rest of the window.
      for (const event of transcript) {
        if (event.seq <= reactedTo) continue;
        reactedTo = event.seq;
        react(event);
      }
      set({ transcript });
    },
    onGapUnrepaired: (_sessionId, error) => stumbled(error),
  });
  await feed.start();

  try {
    const [settings, projects] = await Promise.all([
      ipc.getSettings(),
      ipc.listProjects(),
    ]);
    set({ settings, projects, loading: false });
  } catch (error) {
    set({ loading: false });
    stumbled(error);
  }
}

// ---- projects --------------------------------------------------------------

/** The folder picker, then the Project. Nothing happens when it is cancelled. */
export async function chooseFolder() {
  let path: string | null;
  try {
    path = await os.pickFolder(t(state.settings.mode, "openFolder"));
  } catch (error) {
    stumbled(error);
    return;
  }
  if (path) await openProject(path);
}

export async function openProject(path: string) {
  try {
    const project = await ipc.openProject(path);
    set({ projects: await ipc.listProjects() });
    await selectProject(project.id);
  } catch (error) {
    stumbled(error);
  }
}

export async function selectProject(projectId: string) {
  if (state.sessionId && feed) feed.unwatch(state.sessionId);
  reactedTo = 0;
  set({
    projectId,
    screen: "project",
    sessionId: null,
    transcript: [],
    turns: [],
    checkpoints: [],
    unprotected: [],
    journal: null,
    redo: null,
    turnId: null,
    reviewing: null,
    asking: [],
  });

  try {
    const [sessions, checkpoints] = await Promise.all([
      ipc.listSessions(projectId),
      ipc.listCheckpoints(projectId),
    ]);
    set({ checkpoints });

    // The newest conversation, when the Project has had one. A Project that
    // has never been asked anything has no session until its first turn.
    const session = sessions.at(0);
    if (session && feed) {
      set({ sessionId: session.id });
      await Promise.all([feed.watch(session.id), refreshTurns()]);
    }
    await refreshProtection();
  } catch (error) {
    stumbled(error);
  }
}

export async function forgetProject(projectId: string) {
  try {
    await ipc.removeProject(projectId);
    const projects = await ipc.listProjects();
    if (state.projectId === projectId && state.sessionId && feed) {
      feed.unwatch(state.sessionId);
    }
    set({
      projects,
      ...(state.projectId === projectId
        ? {
            projectId: null,
            sessionId: null,
            transcript: [],
            turns: [],
            checkpoints: [],
            unprotected: [],
            journal: null,
            redo: null,
          }
        : {}),
    });
  } catch (error) {
    stumbled(error);
  }
}

export async function chooseProjectEngine(projectId: string, engineId: string) {
  try {
    await ipc.setProjectEngine(projectId, engineId);
    set({ projects: await ipc.listProjects() });
  } catch (error) {
    stumbled(error);
  }
}

/** Where each Project's history lives, for the Home screen in Developer mode. */
export async function describeJournals() {
  const journals: Record<string, JournalInfo> = { ...state.journals };
  for (const project of state.projects) {
    try {
      journals[project.id] = await ipc.journalInfo(project.id);
    } catch {
      // A Project whose folder has gone still has a row on Home; there is
      // nothing to describe, and nothing to report either.
    }
  }
  set({ journals });
}

/** What Undo does not cover, and how big the history is, for the Project on screen. */
async function refreshProtection() {
  const projectId = state.projectId;
  if (!projectId) return;
  try {
    const [unprotected, journal] = await Promise.all([
      ipc.unprotectedFiles(projectId),
      ipc.journalInfo(projectId),
    ]);
    if (state.projectId !== projectId) return;
    set({ unprotected, journal });
  } catch (error) {
    stumbled(error);
  }
}

// ---- turns -----------------------------------------------------------------

/** Plan first: the assistant says what it would do, and nothing runs until the plan is approved. */
export const plan = (request: string) => startTurn(request, "plan");

/** Straight to work, no plan: Developer mode's "Run", and Everyday's questions. */
export const ask = (request: string) => startTurn(request, "direct");

async function startTurn(request: string, mode: ipc.TurnMode) {
  if (!state.projectId) return;
  try {
    const turnId = await ipc.startTurn(state.projectId, request, mode);
    set({ turnId, trouble: null });

    // The first turn is what starts a conversation, so the session to watch
    // may only exist now.
    if (!state.sessionId) {
      const session = (await ipc.listSessions(state.projectId)).at(0);
      if (session && feed) {
        set({ sessionId: session.id });
        await feed.watch(session.id);
      }
    }
    await refreshTurns();
  } catch (error) {
    stumbled(error);
  }
}

export async function stop() {
  if (!state.turnId) return;
  try {
    await ipc.cancelTurn(state.turnId);
  } catch (error) {
    stumbled(error);
  }
}

/** Go ahead with the plan on screen, with the person's changes if they wrote any. */
export async function approvePlan(edits?: string) {
  const review = state.reviewing;
  if (!review) return;
  const trimmed = edits?.trim();
  try {
    await ipc.approvePlan(review.turnId, trimmed ? trimmed : undefined);
  } catch (error) {
    stumbled(error);
  }
}

/** Not now. The turn ends where it is and nothing is changed. */
export async function rejectPlan() {
  const review = state.reviewing;
  if (!review) return;
  try {
    await ipc.rejectPlan(review.turnId);
  } catch (error) {
    stumbled(error);
  }
}

export async function answer(
  requestId: string,
  decision: Parameters<typeof ipc.answerPermission>[1],
) {
  try {
    await ipc.answerPermission(requestId, decision);
  } catch (error) {
    stumbled(error);
  }
}

async function refreshTurns() {
  const sessionId = state.sessionId;
  if (!sessionId) return;
  try {
    const turns = await ipc.listTurns(sessionId);
    if (state.sessionId !== sessionId) return;
    turns.sort((a, b) => a.started_at.localeCompare(b.started_at));
    set({ turns });
  } catch (error) {
    stumbled(error);
  }
}

/** The last turn that finished, and can therefore be undone. */
export function lastFinishedTurn(): Turn | null {
  for (let i = state.turns.length - 1; i >= 0; i--) {
    const turn = state.turns[i];
    if (turn.id === state.turnId) continue;
    if (turn.pre_checkpoint) return turn;
  }
  return null;
}

// ---- history ---------------------------------------------------------------

export async function refreshCheckpoints() {
  if (!state.projectId) return;
  try {
    set({ checkpoints: await ipc.listCheckpoints(state.projectId) });
  } catch (error) {
    stumbled(error);
  }
}

/**
 * Goes back. Afterwards, Redo points at where the files were just before:
 * the checkpoint immediately below the restore in the list, which is the
 * "Before going back" one when there was anything to keep, and the previous
 * head when there was not (`05-git-journal.md` §6, D16).
 */
export async function goBackTo(checkpointId: string) {
  if (!state.projectId) return;
  try {
    const outcome = await ipc.restoreCheckpoint(state.projectId, checkpointId);
    const checkpoints = await ipc.listCheckpoints(state.projectId);
    const at = checkpoints.findIndex((cp) => cp.id === outcome.checkpoint.id);
    const before = at >= 0 ? checkpoints[at + 1] : undefined;
    set({
      checkpoints,
      redo: before ? { to: before.id, label: before.label } : null,
    });
    // Files something else held open are never quietly skipped.
    if (outcome.skipped_locked.length > 0) {
      const mode = state.settings.mode;
      set({
        trouble: {
          message: t(mode, "lockedFiles", {
            files: outcome.skipped_locked.join(", "),
          }),
          nextAction: t(mode, "lockedFilesNext"),
        },
      });
    }
    await refreshProtection();
  } catch (error) {
    stumbled(error);
  }
}

/** What changed, or would change, between a checkpoint and now. */
export const changesSince = (checkpointId: string) =>
  state.projectId ? ipc.diffSummary(state.projectId, checkpointId) : null;

export async function protectNow(label: string) {
  if (!state.projectId) return;
  try {
    await ipc.checkpointNow(state.projectId, label);
    await refreshCheckpoints();
  } catch (error) {
    stumbled(error);
  }
}

// ---- engines and settings --------------------------------------------------

export async function refreshEngines() {
  try {
    set({ engines: await ipc.listEngines(state.settings.mode === "developer") });
  } catch (error) {
    stumbled(error);
  }
}

/** Checks one engine now, and replaces its row when the answer comes. */
export async function checkEngine(engineId: string, deep = false) {
  if (state.checkingEngines.includes(engineId)) return;
  set({ checkingEngines: [...state.checkingEngines, engineId] });
  try {
    const status: EngineStatus = await ipc.runHealthCheck(engineId, deep);
    set({
      engines: state.engines.map((engine) =>
        engine.id === engineId ? { ...engine, status } : engine,
      ),
    });
  } catch (error) {
    stumbled(error);
  } finally {
    set({
      checkingEngines: state.checkingEngines.filter((id) => id !== engineId),
    });
  }
}

export async function saveSettings(settings: Settings) {
  try {
    await ipc.setSettings(settings);
    set({ settings });
  } catch (error) {
    stumbled(error);
  }
}

// ---- diagnostics -----------------------------------------------------------

export async function refreshDiagnostics() {
  try {
    set({ diagnostics: await ipc.diagnostics() });
  } catch (error) {
    stumbled(error);
  }
}

/** For tests and for the Diagnostics panel: the state as it stands. */
export function snapshot(): State {
  return state;
}
