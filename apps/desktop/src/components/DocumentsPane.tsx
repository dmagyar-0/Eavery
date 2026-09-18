// The Documents pane (`07-ui-vocabulary.md` §3): the folder, what the last
// run changed, and what Undo does not cover. Clicking a file opens it with
// whatever the OS opens it with; nothing is rendered in the window itself.
//
// The folder tree with per-file markers is M5-T04; this is the part of the
// pane the core can already answer for.

import { useStore } from "../store";
import { lastDigest, blocksFor } from "../transcript";
import * as os from "../os";
import { useT, type T } from "../vocab/useT";
import type { Unprotected } from "../types";

function join(root: string, relative: string): string {
  const separator = root.includes("\\") ? "\\" : "/";
  return root.endsWith(separator) ? root + relative : root + separator + relative;
}

function why(t: T, entry: Unprotected): string {
  return entry.reason === "too_large"
    ? t("notProtectedWhy_too_large")
    : t("notProtectedWhy_not_downloaded");
}

export function DocumentsPane() {
  const { projects, projectId, transcript, turns, unprotected } = useStore();
  const t = useT();
  const project = projects.find((candidate) => candidate.id === projectId);
  const digest = lastDigest(blocksFor(transcript, turns));

  if (!project) return null;

  const changed = digest
    ? [
        ...digest.files_added.map((path) => ({ path, mark: "+" })),
        ...digest.files_changed.map((path) => ({ path, mark: "•" })),
        ...digest.files_removed.map((path) => ({ path, mark: "−" })),
      ]
    : [];

  return (
    <div className="documents">
      <div className="documents-folder">
        <span className="path">{project.root}</span>
        <button type="button" className="ghost" onClick={() => void os.showInFolder(project.root)}>
          {t("showFolder")}
        </button>
      </div>

      <h3>{t("changedLastRun")}</h3>
      {changed.length === 0 ? (
        <p className="muted small">{t("noChangesYet")}</p>
      ) : (
        <ul className="files">
          {changed.map(({ path, mark }) => (
            <li key={path}>
              <span className="mark" aria-hidden="true">
                {mark}
              </span>
              {mark === "−" ? (
                <span className="path">{path}</span>
              ) : (
                <button
                  type="button"
                  className="link"
                  onClick={() => void os.openWithOs(join(project.root, path))}
                >
                  {path}
                </button>
              )}
            </li>
          ))}
        </ul>
      )}

      {unprotected.length > 0 ? (
        <>
          <h3>{t("notProtected")}</h3>
          <ul className="files">
            {unprotected.map((entry) => (
              <li key={entry.path}>
                <span className="path">{entry.path}</span>
                <span className="chip">{why(t, entry)}</span>
              </li>
            ))}
          </ul>
        </>
      ) : null}
    </div>
  );
}
