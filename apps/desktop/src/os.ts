// What the window asks the operating system for, through Tauri's plugins.
//
// Kept apart from `ipc.ts`, which is Eavery's own commands: these are the
// folder picker and "open this with whatever the OS opens it with", and they
// are the only places the plugins are called from.

import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";

/** The directory picker. `null` when the person cancelled. */
export async function pickFolder(title: string): Promise<string | null> {
  const chosen = await openDialog({ directory: true, multiple: false, title });
  return typeof chosen === "string" ? chosen : null;
}

/** Opens a file with the OS default app, or a folder in the file manager. */
export const openWithOs = (path: string) => openPath(path);

/** Shows a file or folder in the file manager, selected. */
export const showInFolder = (path: string) => revealItemInDir(path);
