import { confirm } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useState } from "react";
import { api, thumbUrl, type DuplicateGroup, type DuplicateOverview } from "../api";
import { fileName, formatBytes, formatTakenAt } from "../format";
import { describeReport, RecentlyRemoved } from "./RecentlyRemoved";

const PAGE = 50;

type Props = {
  /** Bumped by the parent after scans, which can find new copies. */
  version: number;
  onNotice: (message: string) => void;
  onError: (e: unknown) => void;
};

/** Photos stored more than once, and the removals that can still be undone. */
export function DuplicatesPage({ version, onNotice, onError }: Props) {
  const [overview, setOverview] = useState<DuplicateOverview | null>(null);
  const [groups, setGroups] = useState<DuplicateGroup[]>([]);
  /** The user's choice of copy to keep, by asset; otherwise the first (suggested) one. */
  const [keep, setKeep] = useState<Record<number, number>>({});
  const [busy, setBusy] = useState(false);
  // Bumped after removals so Recently removed reloads.
  const [removals, setRemovals] = useState(0);

  const reload = useCallback(async (count = PAGE) => {
    const [o, g] = await Promise.all([api.duplicateOverview(), api.duplicateGroups(0, count)]);
    setOverview(o);
    setGroups(g);
  }, []);

  useEffect(() => {
    reload().catch(onError);
  }, [version, reload, onError]);

  const act = async (action: () => Promise<string>) => {
    setBusy(true);
    try {
      onNotice(await action());
      setRemovals((n) => n + 1);
      await reload(Math.max(PAGE, groups.length));
    } catch (e) {
      onError(e);
    } finally {
      setBusy(false);
    }
  };

  const removeAll = async () => {
    if (!overview) return;
    const ok = await confirm(
      `Remove ${overview.extraFiles.toLocaleString()} extra copies (${formatBytes(overview.extraBytes)})?\n\n` +
        "One copy of every photo stays where it is. The others move to a hidden .photomanager-trash folder " +
        "next to them, and you can put them back from Recently removed.",
      { title: "Remove extra copies", kind: "warning", okLabel: "Remove copies" },
    );
    if (ok) await act(async () => describeReport(await api.removeDuplicates(keep), "Moved"));
  };

  const removeOne = (g: DuplicateGroup) =>
    act(async () => describeReport(await api.removeDuplicates(keep, [g.assetId]), "Moved"));

  const more = () =>
    api
      .duplicateGroups(groups.length, PAGE)
      .then((next) => setGroups((g) => [...g, ...next]))
      .catch(onError);

  if (!overview) return null;

  return (
    <div className="events-page cleanup-page">
      <section className="cleanup-summary">
        <h2>Duplicates</h2>
        {overview.groups === 0 ? (
          <p className="muted">No photos or videos are stored more than once.</p>
        ) : (
          <>
            <p>
              {overview.groups.toLocaleString()} {overview.groups === 1 ? "item has" : "items have"}{" "}
              {overview.extraFiles.toLocaleString()} extra {overview.extraFiles === 1 ? "copy" : "copies"}, using{" "}
              <strong>{formatBytes(overview.extraBytes)}</strong>.
            </p>
            <button className="primary" disabled={busy} onClick={removeAll}>
              Remove extra copies
            </button>
            <p className="muted small">
              Keeps one copy of each: the one that looks like the original unless you pick another below. Removed
              copies can be put back.
            </p>
          </>
        )}
      </section>

      {groups.map((g) => {
        const kept = keep[g.assetId] ?? g.copies[0]?.id;
        return (
          <section key={g.assetId} className="dupe-group">
            <img className="dupe-thumb" src={thumbUrl(g.assetId)} alt="" loading="lazy" />
            <div className="dupe-body">
              <div className="dupe-head">
                <span>{formatTakenAt(g.takenAt, g.tzOffsetMin)}</span>
                <span className="muted small">
                  {formatBytes(g.size)} × {g.copies.length}
                </span>
                <div className="spacer" />
                <button disabled={busy} onClick={() => removeOne(g)}>
                  Remove other copies
                </button>
              </div>
              <ul className="dupe-copies">
                {g.copies.map((f) => (
                  <li key={f.id}>
                    <label title={f.path}>
                      <input
                        type="radio"
                        name={`keep-${g.assetId}`}
                        checked={kept === f.id}
                        onChange={() => setKeep((k) => ({ ...k, [g.assetId]: f.id }))}
                      />
                      <span className={kept === f.id ? "dupe-kept" : ""}>{fileName(f.path)}</span>
                      <span className="muted small file-path">{f.path}</span>
                      {!f.online && <span className="warn small">drive not connected</span>}
                    </label>
                    {f.online && (
                      <button className="link small" onClick={() => revealItemInDir(f.path)}>
                        Show
                      </button>
                    )}
                  </li>
                ))}
              </ul>
            </div>
          </section>
        );
      })}
      {groups.length < overview.groups && (
        <div className="cleanup-more">
          <button onClick={more}>Show more</button>
        </div>
      )}

      <RecentlyRemoved
        version={removals}
        onChanged={() => reload(Math.max(PAGE, groups.length)).catch(onError)}
        onNotice={onNotice}
        onError={onError}
      />
    </div>
  );
}
