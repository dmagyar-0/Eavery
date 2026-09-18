// What a turn did, in file terms, with the one button that takes it back
// (`07-ui-vocabulary.md` §3). The outbound list is always there — "Nothing"
// when empty — because that is the line the local-first claim rests on.

import type { Digest as DigestView } from "../types";
import { goBackTo, useStore } from "../store";
import { useT } from "../vocab/useT";

function FileList({ title, files }: { title: string; files: string[] }) {
  if (files.length === 0) return null;
  return (
    <div className="digest-section">
      <h4>{title}</h4>
      <ul className="paths">
        {files.map((file) => (
          <li key={file}>{file}</li>
        ))}
      </ul>
    </div>
  );
}

export function Digest({ digest }: { digest: DigestView }) {
  const t = useT();
  const { turnId } = useStore();
  const changed =
    digest.files_added.length + digest.files_changed.length + digest.files_removed.length;

  return (
    <div className="digest">
      <h3>{t("digestTitle")}</h3>
      {changed === 0 ? <p>{t("digestNothing")}</p> : null}
      <FileList title={t("digestAdded", { count: digest.files_added.length })} files={digest.files_added} />
      <FileList title={t("digestChanged", { count: digest.files_changed.length })} files={digest.files_changed} />
      <FileList title={t("digestRemoved", { count: digest.files_removed.length })} files={digest.files_removed} />
      <div className="digest-section">
        <h4>{t("digestOutbound")}</h4>
        {digest.outbound_actions.length === 0 ? (
          <p className="nothing">{t("nothing")}</p>
        ) : (
          <ul>
            {digest.outbound_actions.map((action) => (
              <li key={action}>{action}</li>
            ))}
          </ul>
        )}
      </div>
      {digest.refused_actions.length > 0 ? (
        <div className="digest-section">
          <h4>{t("digestRefused")}</h4>
          <ul>
            {digest.refused_actions.map((action) => (
              <li key={action}>{action}</li>
            ))}
          </ul>
        </div>
      ) : null}
      {digest.undo_to && changed > 0 ? (
        <button
          type="button"
          disabled={turnId !== null}
          onClick={() => void goBackTo(digest.undo_to as string)}
        >
          {t("digestUndo")}
        </button>
      ) : null}
    </div>
  );
}
