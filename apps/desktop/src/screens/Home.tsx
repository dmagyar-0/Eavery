// Home (`07-ui-vocabulary.md` §3): the Projects, and "Open a folder".
// Developer mode also says where each one's history is kept.

import { useEffect } from "react";
import {
  chooseFolder,
  describeJournals,
  forgetProject,
  selectProject,
  useStore,
} from "../store";
import { useShown, useT } from "../vocab/useT";

/** A long path, shortened from the left so the folder's own name survives. */
export function shortenPath(path: string, max = 48): string {
  if (path.length <= max) return path;
  return "…" + path.slice(path.length - max + 1);
}

function whenAdded(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleDateString(undefined, { dateStyle: "medium" });
}

export function Home() {
  const { projects, engines, journals, loading, settings } = useStore();
  const t = useT();
  const shown = useShown();
  const developer = settings.mode === "developer";

  useEffect(() => {
    if (developer && projects.length > 0) void describeJournals();
  }, [developer, projects]);

  const engineName = (engineId: string | null) => {
    if (!engineId) return t("projectEngineDefault");
    return engines.find((engine) => engine.id === engineId)?.display_name ?? engineId;
  };

  return (
    <main className="screen home">
      <header className="screen-head">
        <h1>{t("homeTitle")}</h1>
        <button type="button" className="primary" onClick={() => void chooseFolder()}>
          {t("openFolder")}
        </button>
      </header>
      <p className="lead">{t("homeLead")}</p>

      {loading ? null : projects.length === 0 ? (
        <p className="muted">{t("homeEmpty")}</p>
      ) : (
        <ul className="project-list">
          {projects.map((project) => (
            <li key={project.id} className="project-card">
              <button
                type="button"
                className="project-open"
                onClick={() => void selectProject(project.id)}
              >
                <span className="project-name">{project.name}</span>
                <span className="path" title={project.root}>
                  {shortenPath(project.root)}
                </span>
                <span className="muted small">
                  {t("projectAdded", { when: whenAdded(project.created_at) })}
                  {" · "}
                  {t("projectEngine", { engine: engineName(project.engine_id) })}
                </span>
                {shown("journalPath") && journals[project.id] ? (
                  <span className="muted small path">
                    {t("journalPath", { path: journals[project.id].path })}
                  </span>
                ) : null}
              </button>
              <button
                type="button"
                className="ghost"
                title={t("forgetProjectHint")}
                onClick={() => void forgetProject(project.id)}
              >
                {t("forgetProject")}
              </button>
            </li>
          ))}
        </ul>
      )}
    </main>
  );
}
