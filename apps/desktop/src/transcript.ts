// The transcript as rows, from the transcript as events.
//
// Events arrive one at a time and some of them are continuations: a chunk of
// text belongs to the message before it, a tool call update replaces the
// call it updates, a decision closes the question it answers. This turns the
// event list into a list of blocks, each keyed stably, so the transcript is a
// keyed list of rows rather than a re-render of everything per chunk
// (`07-ui-vocabulary.md` §4, rule 3).
//
// Pure, and rebuilt from the whole list each time. That is deliberate: a gap
// re-fetch can insert events *before* ones already held, which an append-only
// reducer would get wrong. What keeps a streamed chunk from re-rendering every
// row is `blocksFor`, which hands back the previous object for any row that
// did not change, so a memoised row sees the same props and stays put.

import type {
  Checkpoint,
  DecidedBy,
  Decision,
  Digest,
  EngineStatus,
  ErrorCode,
  PermissionView,
  Plan,
  PlanEntryView,
  StoredEvent,
  ToolCallView,
  Turn,
  TurnPhase,
} from "./types";

export type Block = { key: string; at: string; turnId: string | null } & (
  | { kind: "request"; text: string }
  | { kind: "text"; text: string }
  | { kind: "thought"; text: string }
  | { kind: "tool"; call: ToolCallView }
  | { kind: "plan_entries"; entries: PlanEntryView[] }
  | {
      kind: "permission";
      request: PermissionView;
      decision: Decision | null;
      by: DecidedBy | null;
    }
  | { kind: "plan"; plan: Plan; vendor: string }
  | { kind: "phase"; phase: TurnPhase }
  | { kind: "checkpoint"; checkpoint: Checkpoint }
  | { kind: "restored"; to: string; newCheckpoint: string; skippedLocked: string[] }
  | { kind: "finished"; stopReason: string; digest: Digest | null }
  | { kind: "engine_status"; engineId: string; status: EngineStatus }
  | { kind: "crashed"; engineId: string; stderrTail: string[] }
  | { kind: "error"; code: ErrorCode; message: string; nextAction: string | null }
);

/**
 * Groups a session's events into blocks. `turns` supplies what the user
 * asked, which the events themselves do not carry.
 */
export function toBlocks(events: StoredEvent[], turns: Turn[]): Block[] {
  const requests = new Map(turns.map((turn) => [turn.id, turn.request]));
  const blocks: Block[] = [];
  const last = () => blocks[blocks.length - 1];

  for (const stored of events) {
    const event = stored.event;
    const base = { at: stored.at, turnId: stored.turn_id };
    const key = `${stored.seq}`;

    switch (event.type) {
      case "turn_started":
        blocks.push({
          ...base,
          key,
          kind: "request",
          text: requests.get(event.turn_id) ?? "",
        });
        break;

      case "agent_text":
      case "agent_thought": {
        const kind = event.type === "agent_text" ? "text" : "thought";
        const previous = last();
        if (
          previous &&
          previous.kind === kind &&
          previous.turnId === stored.turn_id
        ) {
          previous.text += event.text;
        } else {
          blocks.push({ ...base, key, kind, text: event.text });
        }
        break;
      }

      case "tool_call_started":
        blocks.push({ ...base, key, kind: "tool", call: event.call });
        break;

      case "tool_call_updated": {
        const started = findBack(
          blocks,
          (block) =>
            block.kind === "tool" &&
            block.call.id === event.call.id &&
            block.turnId === stored.turn_id,
        );
        if (started && started.kind === "tool") {
          started.call = event.call;
        } else {
          // An update for a call whose start was never seen. Show it rather
          // than lose it — and if the call was announced through a permission
          // request, which carries its title, kind and files, show those
          // rather than the bare id the core had to fall back on.
          const asked = findBack(
            blocks,
            (block) =>
              block.kind === "permission" &&
              block.request.tool_call_id === event.call.id &&
              block.turnId === stored.turn_id,
          );
          const call =
            asked && asked.kind === "permission" && event.call.title === event.call.id
              ? {
                  ...event.call,
                  title: asked.request.title,
                  kind: event.call.kind === "other" ? asked.request.kind : event.call.kind,
                  // The turn engine reclassified the request with the Project
                  // root in hand; the bare update was classified without files.
                  risk: asked.request.risk,
                  locations:
                    event.call.locations.length > 0
                      ? event.call.locations
                      : asked.request.locations,
                }
              : event.call;
          blocks.push({ ...base, key, kind: "tool", call });
        }
        break;
      }

      case "plan_updated": {
        const previous = findBack(
          blocks,
          (block) =>
            block.kind === "plan_entries" && block.turnId === stored.turn_id,
        );
        if (previous && previous.kind === "plan_entries") {
          previous.entries = event.entries;
        } else {
          blocks.push({ ...base, key, kind: "plan_entries", entries: event.entries });
        }
        break;
      }

      case "permission_requested":
        blocks.push({
          ...base,
          key,
          kind: "permission",
          request: event.request,
          decision: null,
          by: null,
        });
        break;

      case "permission_resolved": {
        const asked = findBack(
          blocks,
          (block) =>
            block.kind === "permission" &&
            block.request.request_id === event.request_id,
        );
        if (asked && asked.kind === "permission") {
          asked.decision = event.decision;
          asked.by = event.by;
        }
        break;
      }

      case "plan_ready":
        blocks.push({ ...base, key, kind: "plan", plan: event.plan, vendor: event.vendor });
        break;

      case "phase_changed":
        blocks.push({ ...base, key, kind: "phase", phase: event.phase });
        break;

      case "checkpoint_created":
        blocks.push({ ...base, key, kind: "checkpoint", checkpoint: event.checkpoint });
        break;

      case "restored":
        blocks.push({
          ...base,
          key,
          kind: "restored",
          to: event.to,
          newCheckpoint: event.new_checkpoint,
          skippedLocked: event.skipped_locked,
        });
        break;

      case "turn_finished":
        blocks.push({
          ...base,
          key,
          kind: "finished",
          stopReason: event.stop_reason,
          digest: event.digest,
        });
        break;

      case "engine_status":
        blocks.push({
          ...base,
          key,
          kind: "engine_status",
          engineId: event.engine_id,
          status: event.status,
        });
        break;

      case "engine_crashed":
        blocks.push({
          ...base,
          key,
          kind: "crashed",
          engineId: event.engine_id,
          stderrTail: event.stderr_tail,
        });
        break;

      case "error":
        blocks.push({
          ...base,
          key,
          kind: "error",
          code: event.code,
          message: event.message,
          nextAction: event.next_action,
        });
        break;

      default:
        break;
    }
  }
  return blocks;
}

let lastEvents: StoredEvent[] | null = null;
let lastTurns: Turn[] | null = null;
let lastBlocks: Block[] = [];

/**
 * `toBlocks`, memoised on its inputs and with row identity preserved: a
 * block whose every field is what it was last time is the same object as
 * last time. Event payloads are held by the feed and never copied, so a
 * field-by-field `===` is enough to tell.
 */
export function blocksFor(events: StoredEvent[], turns: Turn[]): Block[] {
  if (events === lastEvents && turns === lastTurns) return lastBlocks;
  const previous = new Map(lastBlocks.map((block) => [block.key, block]));
  const fresh = toBlocks(events, turns);
  const blocks = fresh.map((block) => {
    const before = previous.get(block.key);
    return before && sameBlock(before, block) ? before : block;
  });
  lastEvents = events;
  lastTurns = turns;
  lastBlocks = blocks;
  return blocks;
}

function sameBlock(a: Block, b: Block): boolean {
  const left = a as unknown as Record<string, unknown>;
  const right = b as unknown as Record<string, unknown>;
  const keys = Object.keys(left);
  if (keys.length !== Object.keys(right).length) return false;
  return keys.every((key) => left[key] === right[key]);
}

function findBack(blocks: Block[], matches: (block: Block) => boolean) {
  for (let i = blocks.length - 1; i >= 0; i--) {
    if (matches(blocks[i])) return blocks[i];
  }
  return undefined;
}

/**
 * The last digest in the transcript: what the last finished turn did. `null`
 * once the folder has been put back since, because the files it names are
 * no longer what is on disk.
 */
export function lastDigest(blocks: Block[]): Digest | null {
  for (let i = blocks.length - 1; i >= 0; i--) {
    const block = blocks[i];
    if (block.kind === "restored") return null;
    if (block.kind === "finished") return block.digest;
  }
  return null;
}

/** The last thing the assistant did, as one line for "Working on it…". */
export function latestActivity(blocks: Block[]): Block | null {
  for (let i = blocks.length - 1; i >= 0; i--) {
    const block = blocks[i];
    if (block.kind === "tool" || block.kind === "permission") return block;
  }
  return null;
}

/** The last path segment: what a person calls the file. */
export function fileName(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}
