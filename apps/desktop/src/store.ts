// What the window is currently showing.
//
// A plain observable object, read through `useSyncExternalStore`. There is no
// state library here on purpose: the only state the frontend has is a copy of
// what the core just said, and the core is the one that decides things
// (`docs/plan/03-architecture.md` §1). Anything that looks like a rule
// appearing in this file is a rule in the wrong crate.

import { useSyncExternalStore } from "react";
import * as ipc from "./ipc";
import { createFeed, type Feed } from "./events";
import type {
  Checkpoint,
  EngineListing,
  PermissionView,
  Project,
  Settings,
  StoredEvent,
} from "./types";

export type Trouble = {
  message: string;
  nextAction: string | null;
};

export type State = {
  /** True until the first load finishes, so the window can say nothing rather than "no projects". */
  loading: boolean;
  projects: Project[];
  projectId: string | null;
  /** The conversation on screen, if the Project has had one. */
  sessionId: string | null;
  transcript: StoredEvent[];
  checkpoints: Checkpoint[];
  engines: EngineListing[];
  settings: Settings;
  /** The turn running right now, if any. Stop is the only thing to do while it is. */
  turnId: string | null;
  /** Questions waiting for an answer, oldest first: one dialog at a time. */
  asking: PermissionView[];
  /** The last thing that went wrong, as something to do about it. */
  trouble: Trouble | null;
};

const initial: State = {
  loading: true,
  projects: [],
  projectId: null,
  sessionId: null,
  transcript: [],
  checkpoints: [],
  engines: [],
  settings: { mode: "everyday", default_engine: null },
  turnId: null,
  asking: [],
  trouble: null,
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
  set({ trouble: { message, nextAction } });
}

export function dismissTrouble() {
  set({ trouble: null });
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
      set({ turnId: core.turn_id });
      break;
    case "turn_finished":
      set({ turnId: null, asking: [] });
      void refreshCheckpoints();
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
    case "error":
      set({
        trouble: { message: core.message, nextAction: core.next_action },
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
  set({ projectId, sessionId: null, transcript: [], checkpoints: [] });

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
      await feed.watch(session.id);
    }
  } catch (error) {
    stumbled(error);
  }
}

export async function forgetProject(projectId: string) {
  try {
    await ipc.removeProject(projectId);
    const projects = await ipc.listProjects();
    set({
      projects,
      ...(state.projectId === projectId
        ? { projectId: null, sessionId: null, transcript: [], checkpoints: [] }
        : {}),
    });
  } catch (error) {
    stumbled(error);
  }
}

// ---- turns -----------------------------------------------------------------

export async function ask(request: string) {
  if (!state.projectId) return;
  try {
    const turnId = await ipc.startTurn(state.projectId, request);
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

// ---- history ---------------------------------------------------------------

export async function refreshCheckpoints() {
  if (!state.projectId) return;
  try {
    set({ checkpoints: await ipc.listCheckpoints(state.projectId) });
  } catch (error) {
    stumbled(error);
  }
}

export async function goBackTo(checkpointId: string) {
  if (!state.projectId) return;
  try {
    const outcome = await ipc.restoreCheckpoint(state.projectId, checkpointId);
    await refreshCheckpoints();
    // Files something else held open are never quietly skipped.
    if (outcome.skipped_locked.length > 0) {
      set({
        trouble: {
          message: `These files were open, so they were left as they are: ${outcome.skipped_locked.join(", ")}`,
          nextAction: "Close them and go back again.",
        },
      });
    }
  } catch (error) {
    stumbled(error);
  }
}

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

export async function saveSettings(settings: Settings) {
  try {
    await ipc.setSettings(settings);
    set({ settings });
  } catch (error) {
    stumbled(error);
  }
}

/** For tests and for the Diagnostics panel: the state as it stands. */
export function snapshot(): State {
  return state;
}
