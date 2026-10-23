import { confirm } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useState } from "react";
import {
  api,
  thumbUrl,
  type DuplicateGroup,
  type DuplicateOverview,
  type FileOp,
  type FileOpReport,
} from "../api";
import { fileName, formatBytes, formatTakenAt } from "../format";

const PAGE = 50;

type Props = {
  /** Bumped by the parent after scans, which can find new copies. */
  version: number;
  onNotice: (message: string) => void;
  onError: (e: unknown) => void;
};

function describe(r: FileOpReport, verb: string) {
  const parts = [`${verb} ${r.done.toLocaleString()} ${r.done === 1 ? "file" : "files"} (${formatBytes(r.bytes)})`];
  if (r.failed) parts.push(`${r.failed.toLocaleString()} skipped: ${r.errors[0] ?? "see the list"}`);
  return parts.join(". ");
}

/** Photos stored more than once, and the removals that can still be undone. */
export function DuplicatesPage({ version, onNotice, onError }: Props) {
  const [overview, setOverview] = useState<DuplicateOverview | null>(null);
  const [groups, setGroups] = useState<DuplicateGroup[]>([]);
  const [ops, setOps] = useState<FileOp[]>([]);
  /** The user's choice of copy to keep, by asset; otherwise the first (suggested) one. */
  const [keep, setKeep] = useState<Record<number, number>>({});
  const [busy, setBusy] = useState(false);

  const reload = useCallback(
    async (count = PAGE) => {
      const [o, g, f] = await Promise.all([api.duplicateOverview(), api.duplicateGroups(0, count), api.listFileOps()]);
      setOverview(o);
      setGroups(g);
      setOps(f);
    },
    [],
  );

  useEffect(() => {
    reload().catch(onError);
  }, [version, reload, onError]);

  const act = async (action: () => Promise<string>) => {
    setBusy(true);
    try {
      onNotice(await action());
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
    if (ok) await act(async () => describe(await api.removeDuplicates(keep), "Moved"));
  };

  const removeOne = (g: DuplicateGroup) =>
    act(async () => describe(await api.removeDuplicates(keep, [g.assetId]), "Moved"));

  const undo = (op: FileOp) => act(async () => describe(await api.undoFileOp(op.id), "Put back"));

  const empty = async (op: FileOp) => {
    const ok = await confirm(
      `Send ${op.inTrash.toLocaleString()} files (${formatBytes(op.inTrashBytes)}) to the Recycle Bin?\n\n` +
        "This frees the space. You can still restore them from the Recycle Bin, but not from here. " +
        "Drives without a Recycle Bin (network drives, most USB sticks) are left as they are.",
      { title: "Send to Recycle Bin", kind: "warning", okLabel: "Send to Recycle Bin" },
    );
    if (ok) await act(async () => describe(await api.emptyFileOp(op.id), "Sent"));
  };

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

      {ops.length > 0 && (
        <section className="cleanup-ops">
          <h3>Recently removed</h3>
          <ul>
            {ops.map((op) => (
              <li key={op.id}>
                <span>{new Date(op.createdAt).toLocaleString()}</span>
                <span className="muted">{op.reason === "duplicates" ? "Duplicate copies" : op.reason}</span>
                <span className="muted small">
                  {op.status === "undone"
                    ? "Put back"
                    : op.status === "emptied"
                      ? "Sent to the Recycle Bin"
                      : op.inTrash > 0
                        ? `${op.inTrash.toLocaleString()} files · ${formatBytes(op.inTrashBytes)} in the trash folder`
                        : "Nothing left in the trash folder"}
                  {op.failed > 0 && ` · ${op.failed.toLocaleString()} skipped`}
                </span>
                <div className="spacer" />
                {op.status === "done" && op.inTrash > 0 && (
                  <>
                    <button disabled={busy} onClick={() => undo(op)}>
                      Undo
                    </button>
                    <button disabled={busy} onClick={() => empty(op)}>
                      Send to Recycle Bin
                    </button>
                  </>
                )}
              </li>
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}
