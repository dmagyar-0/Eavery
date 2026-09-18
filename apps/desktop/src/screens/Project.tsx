// The Project screen: three panes (`07-ui-vocabulary.md` §3).
// Documents on the left, the conversation in the middle, Activity on the
// right. The window's state decides what is in them; this lays them out.

import { useStore } from "../store";
import { useT } from "../vocab/useT";
import { Activity } from "../components/Activity";
import { Composer } from "../components/Composer";
import { DocumentsPane } from "../components/DocumentsPane";
import { Transcript } from "../components/Transcript";

function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

export function ProjectScreen() {
  const { projects, projectId, journal } = useStore();
  const t = useT();
  const project = projects.find((candidate) => candidate.id === projectId);
  if (!project) return null;

  return (
    <main className="screen project">
      <aside className="pane pane-documents" aria-label={t("paneDocuments")}>
        <h2>{t("paneDocuments")}</h2>
        <DocumentsPane />
        {journal ? (
          <p className="muted small pane-foot">
            {t("journalSize", { size: bytes(journal.size_bytes), loose: journal.loose_objects })}
          </p>
        ) : null}
      </aside>
      <section className="pane pane-conversation" aria-label={t("paneConversation")}>
        <h2>{project.name}</h2>
        <Transcript />
        <Composer />
      </section>
      <aside className="pane pane-activity" aria-label={t("paneActivity")}>
        <h2>{t("paneActivity")}</h2>
        <Activity />
      </aside>
    </main>
  );
}
