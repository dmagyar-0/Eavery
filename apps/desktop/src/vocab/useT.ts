// The vocabulary, bound to the mode the window is in.
//
// A component that renders text calls `const t = useT()` and re-renders when
// the mode changes, because `useStore` is what it reads the mode through.

import { useStore } from "../store";
import type { Key } from "./dictionary";
import { shown as shownIn, t as render, type Vars } from "./t";

export type T = (key: Key, vars?: Vars) => string;

export function useT(): T {
  const { settings } = useStore();
  return (key, vars) => render(settings.mode, key, vars);
}

/** Whether a key is rendered at all in the current mode. */
export function useShown(): (key: Key) => boolean {
  const { settings } = useStore();
  return (key) => shownIn(settings.mode, key);
}
