// `t(mode, key, vars?)`: one string, in the vocabulary of the mode
// (`docs/plan/07-ui-vocabulary.md` §1–2).
//
// Pure on purpose. Reading the mode from the store here would make the
// vocabulary depend on the window's state, and the store needs the vocabulary
// too — for the messages it writes when something goes wrong. `useT` in
// `useT.ts` is the one that binds this to the store.

import { dictionary, type Key, type Rendering } from "./dictionary";
import type { UiMode } from "../types";

export type Vars = Record<string, string | number>;

/** Whether a key has anything to show in this mode. `null` in the dictionary means hidden. */
export function shown(mode: UiMode, key: Key): boolean {
  return (dictionary[key] as Rendering)[mode] !== null;
}

/**
 * The string for a key, with `{name}` placeholders filled from `vars`. A
 * hidden key renders as an empty string; ask `shown` first when it matters.
 */
export function t(mode: UiMode, key: Key, vars?: Vars): string {
  const text = (dictionary[key] as Rendering)[mode];
  if (text === null) return "";
  if (!vars) return text;
  return text.replace(/\{(\w+)\}/g, (whole, name: string) =>
    name in vars ? String(vars[name]) : whole,
  );
}
