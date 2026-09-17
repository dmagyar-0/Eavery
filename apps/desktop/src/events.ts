// The one event stream, and what to do when some of it is missed.
//
// `core://event` carries every `StoredEvent` as it happens. Each one has a
// `seq` from a single counter shared by every Project, which is the part worth
// thinking about:
//
// - A gap in that counter means events were missed **somewhere**, not
//   necessarily in the conversation on screen. Two Projects working at once
//   interleave their events, so consecutive events for one session almost
//   never have consecutive `seq`s.
// - So the gap is detected globally and repaired per session: whenever the
//   global counter skips, every session being watched asks for everything
//   after the last `seq` it actually holds (`03-architecture.md` §7).
//
// That makes a missed event a short refetch rather than a transcript with a
// hole in it — which matters because the transcript is the record of what
// happened to someone's files.

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { StoredEvent } from "./types";
import { listEvents } from "./ipc";

/** The event name the Rust side emits on. */
export const CORE_EVENT = "core://event";

export type Feed = {
  /** Starts listening. Call the returned function to stop. */
  start: () => Promise<UnlistenFn>;
  /** Watches one session's transcript, and fetches what it does not have. */
  watch: (sessionId: string) => Promise<void>;
  unwatch: (sessionId: string) => void;
  /** Everything held for a session, oldest first. */
  events: (sessionId: string) => StoredEvent[];
};

export type FeedOptions = {
  /** Called whenever a session's transcript changed. */
  onChange: (sessionId: string) => void;
  /** Called when a refetch failed, so the UI can say the transcript may be short. */
  onGapUnrepaired?: (sessionId: string, error: unknown) => void;
};

export function createFeed(options: FeedOptions): Feed {
  // Keyed by `seq`, so an event that arrives twice — live and again in a
  // refetch — is stored once.
  const bySession = new Map<string, Map<number, StoredEvent>>();
  let lastGlobal = 0;
  let refetching = false;

  const transcript = (sessionId: string) => {
    let events = bySession.get(sessionId);
    if (!events) {
      events = new Map();
      bySession.set(sessionId, events);
    }
    return events;
  };

  const add = (event: StoredEvent) => {
    const events = bySession.get(event.session_id);
    // Not a session anyone is looking at. The store has it; there is no point
    // holding it in the window.
    if (!events) return false;
    if (events.has(event.seq)) return false;
    events.set(event.seq, event);
    return true;
  };

  const lastSeq = (sessionId: string) => {
    const seqs = transcript(sessionId).keys();
    let last = 0;
    for (const seq of seqs) if (seq > last) last = seq;
    return last;
  };

  /** Asks for everything each watched session is missing. */
  const repair = async () => {
    // One repair at a time: a burst of events must not become a burst of
    // identical requests.
    if (refetching) return;
    refetching = true;
    try {
      for (const sessionId of [...bySession.keys()]) {
        try {
          const missed = await listEvents(sessionId, lastSeq(sessionId));
          let changed = false;
          for (const event of missed) changed = add(event) || changed;
          if (changed) options.onChange(sessionId);
        } catch (error) {
          options.onGapUnrepaired?.(sessionId, error);
        }
      }
    } finally {
      refetching = false;
    }
  };

  return {
    start: () =>
      listen<StoredEvent>(CORE_EVENT, ({ payload }) => {
        const skipped = lastGlobal > 0 && payload.seq > lastGlobal + 1;
        if (payload.seq > lastGlobal) lastGlobal = payload.seq;

        if (add(payload)) options.onChange(payload.session_id);
        // Something was missed. It may have been in another Project, so ask
        // every session being watched what it is short of.
        if (skipped) void repair();
      }),

    watch: async (sessionId: string) => {
      const known = transcript(sessionId).size > 0;
      const events = await listEvents(sessionId, known ? lastSeq(sessionId) : undefined);
      for (const event of events) {
        transcript(sessionId).set(event.seq, event);
        if (event.seq > lastGlobal) lastGlobal = event.seq;
      }
      options.onChange(sessionId);
    },

    unwatch: (sessionId: string) => {
      bySession.delete(sessionId);
    },

    events: (sessionId: string) =>
      [...transcript(sessionId).values()].sort((a, b) => a.seq - b.seq),
  };
}
