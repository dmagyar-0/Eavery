// The Documents pane (`07-ui-vocabulary.md` §3): the folder, what the last
// run changed, and what Undo does not cover. Clicking a file opens it with
// whatever the OS opens it with; nothing is rendered in the window itself.
//
// The tree is "flat-ish" on purpose (M5-T04). Folders down to depth 2 are
// open when the pane appears, because that is the part of a Project someone
// recognises at a glance; deeper ones are shut until asked for. A folder
// holding a file the last run touched is open whatever its depth — the dot
// is the thing the pane exists to show, and a dot nobody can see is no use.

import { useMemo, useState } from "react";
import { useStore } from "../store";
import { lastDigest, blocksFor } from "../transcript";
import * as os from "../os";
import { useT, type T } from "../vocab/useT";
import type { DocumentNode, Unprotected } from "../types";

/** How deep the tree is open when the pane first appears. */
const OPEN_TO_DEPTH = 2;

function join(root: string, relative: string): string {
  const separator = root.includes("\\") ? "\\" : "/";
  const native = separator === "\\" ? relative.replace(/\//g, "\\") : relative;
  return root.endsWith(separator) ? root + native : root + separator + native;
}

function why(t: T, entry: Unprotected): string {
  return entry.reason === "too_large"
    ? t("notProtectedWhy_too_large")
    : t("notProtectedWhy_not_downloaded");
}

/** What the last run did to a file, as the mark that goes beside its name. */
type Marks = Map<string, "+" | "•" | "−">;

/**
 * Whether a folder is open before anyone has clicked it: the shallow ones,
 * and any folder holding something the last run touched, however deep.
 * `path` carries its own depth — it is the number of separators in it — so
 * this is the same answer whether it is asked while drawing the row or while
 * toggling it.
 */
function openByDefault(path: string, holders: Set<string>): boolean {
  return path.split("/").length - 1 < OPEN_TO_DEPTH || holders.has(path);
}

/** Every folder that holds a marked file, at any depth, so it can be opened. */
function foldersHolding(marks: Marks): Set<string> {
  const holders = new Set<string>();
  for (const path of marks.keys()) {
    const parts = path.split("/");
    for (let index = 1; index < parts.length; index += 1) {
      holders.add(parts.slice(0, index).join("/"));
    }
  }
  return holders;
}

function Row({
  node,
  root,
  marks,
  holders,
  opened,
  toggle,
}: {
  node: DocumentNode;
  root: string;
  marks: Marks;
  holders: Set<string>;
  opened: Record<string, boolean>;
  toggle: (path: string) => void;
}) {
  const mark = marks.get(node.path);
  const open = opened[node.path] ?? openByDefault(node.path, holders);

  if (node.directory) {
    return (
      <li className="doc-folder">
        <button
          type="button"
          className="doc-row link"
          aria-expanded={open}
          onClick={() => toggle(node.path)}
        >
          <span className="doc-twist" aria-hidden="true">
            {open ? "▾" : "▸"}
          </span>
          <span className="doc-name">{node.name}</span>
        </button>
        {open && node.children.length > 0 ? (
          <ul className="doc-children">
            {node.children.map((child) => (
              <Row
                key={child.path}
                node={child}
                root={root}
                marks={marks}
                holders={holders}
                opened={opened}
                toggle={toggle}
              />
            ))}
          </ul>
        ) : null}
      </li>
    );
  }

  return (
    <li className="doc-file">
      <button
        type="button"
        className="doc-row link"
        onClick={() => void os.openWithOs(join(root, node.path))}
      >
        {mark ? (
          <span className="mark" aria-hidden="true">
            {mark}
          </span>
        ) : (
          <span className="doc-twist" aria-hidden="true" />
        )}
        <span className="doc-name">{node.name}</span>
      </button>
    </li>
  );
}

export function DocumentsPane() {
  const { projects, projectId, transcript, turns, unprotected, documents } = useStore();
  const t = useT();
  const [opened, setOpened] = useState<Record<string, boolean>>({});
  const project = projects.find((candidate) => candidate.id === projectId);
  const digest = lastDigest(blocksFor(transcript, turns));

  const marks: Marks = useMemo(() => {
    const marks: Marks = new Map();
    if (!digest) return marks;
    for (const path of digest.files_added) marks.set(path, "+");
    for (const path of digest.files_changed) marks.set(path, "•");
    for (const path of digest.files_removed) marks.set(path, "−");
    return marks;
  }, [digest]);
  const holders = useMemo(() => foldersHolding(marks), [marks]);

  if (!project) return null;

  const toggle = (path: string) =>
    setOpened((was) => ({
      ...was,
      [path]: !(was[path] ?? openByDefault(path, holders)),
    }));

  // A file the run removed is gone from the folder, so the tree cannot show
  // it. It is listed on its own, because "it is not there any more" is the
  // answer to the question someone is asking when they look for it.
  const removed = digest?.files_removed ?? [];

  return (
    <div className="documents">
      <div className="documents-folder">
        <span className="path">{project.root}</span>
        <button type="button" className="ghost" onClick={() => void os.showInFolder(project.root)}>
          {t("showFolder")}
        </button>
      </div>

      {documents === null ? (
        <p className="muted small">{t("loading")}</p>
      ) : documents.entries.length === 0 ? (
        <p className="muted small">{t("folderEmpty")}</p>
      ) : (
        <ul className="doc-tree">
          {documents.entries.map((node) => (
            <Row
              key={node.path}
              node={node}
              root={project.root}
              marks={marks}
              holders={holders}
              opened={opened}
              toggle={toggle}
            />
          ))}
        </ul>
      )}
      {documents?.truncated ? (
        <p className="muted small">{t("folderTooBigToList", { count: documents.files })}</p>
      ) : null}

      {removed.length > 0 ? (
        <>
          <h3>{t("removedLastRun")}</h3>
          <ul className="files">
            {removed.map((path) => (
              <li key={path}>
                <span className="mark" aria-hidden="true">
                  −
                </span>
                <span className="path">{path}</span>
              </li>
            ))}
          </ul>
        </>
      ) : null}

      {marks.size === 0 ? <p className="muted small">{t("noChangesYet")}</p> : null}

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
