import { confirm } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { api, type FileOp, type FileOpKind, type FileOpReport } from "../api";
import { formatBytes } from "../format";

const REASONS: Record<string, string> = {
  duplicates: "Duplicate copies",
  similar: "Similar photos",
  removed: "Removed photos",
  organize: "Organized by date",
  rename: "Renamed",
};

/** "Moved 3 files (12 MB). 1 skipped: …" */
export function describeReport(r: FileOpReport, verb: string) {
  const parts = [`${verb} ${r.done.toLocaleString()} ${r.done === 1 ? "file" : "files"} (${formatBytes(r.bytes)})`];
  if (r.failed) parts.push(`${r.failed.toLocaleString()} skipped: ${r.errors[0] ?? "see the list"}`);
  return parts.join(". ");
}

type Props = {
  /** Bumped by the parent after it removed files. */
  version: number;
  /** After an undo or empty, so the parent can reload what it shows. */
  onChanged: () => void;
  onNotice: (message: string) => void;
  onError: (e: unknown) => void;
  /** Shown as its own page (no heading; says so when empty). */
  standalone?: boolean;
  /** Removals (default) or organizer moves, which can be undone but not emptied. */
  kind?: FileOpKind;
};

/** Recent removals (or moves), with Undo and, for removals, Send to Recycle Bin. */
export function RecentlyRemoved({ version, onChanged, onNotice, onError, standalone, kind = "trash" }: Props) {
  const moves = kind === "move";
  const [ops, setOps] = useState<FileOp[]>([]);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api.listFileOps(kind).then(setOps, onError);
  }, [version, kind, onError]);

  const act = async (action: () => Promise<string>) => {
    setBusy(true);
    try {
      onNotice(await action());
      setOps(await api.listFileOps(kind));
      onChanged();
    } catch (e) {
      onError(e);
    } finally {
      setBusy(false);
    }
  };

  const undo = (op: FileOp) => act(async () => describeReport(await api.undoFileOp(op.id), "Put back"));

  const empty = async (op: FileOp) => {
    const ok = await confirm(
      `Send ${op.inTrash.toLocaleString()} files (${formatBytes(op.inTrashBytes)}) to the Recycle Bin?\n\n` +
        "This frees the space. You can still restore them from the Recycle Bin, but not from here. " +
        "Drives without a Recycle Bin (network drives, most USB sticks) are left as they are.",
      { title: "Send to Recycle Bin", kind: "warning", okLabel: "Send to Recycle Bin" },
    );
    if (ok) await act(async () => describeReport(await api.emptyFileOp(op.id), "Sent"));
  };

  if (ops.length === 0) {
    return standalone ? <p className="cleanup-ops muted">Nothing has been removed yet.</p> : null;
  }
  return (
    <section className="cleanup-ops">
      {!standalone && <h3>{moves ? "Recent moves" : "Recently removed"}</h3>}
      <ul>
        {ops.map((op) => (
          <li key={op.id}>
            <span>{new Date(op.createdAt).toLocaleString()}</span>
            <span className="muted">{REASONS[op.reason] ?? op.reason}</span>
            <span className="muted small">
              {op.status === "undone"
                ? moves
                  ? "Moved back"
                  : "Put back"
                : op.status === "emptied"
                  ? "Sent to the Recycle Bin"
                  : op.inTrash > 0
                    ? moves
                      ? `${op.inTrash.toLocaleString()} files moved`
                      : `${op.inTrash.toLocaleString()} files · ${formatBytes(op.inTrashBytes)} in the trash folder`
                    : moves
                      ? "Nothing moved"
                      : "Nothing left in the trash folder"}
              {op.failed > 0 && ` · ${op.failed.toLocaleString()} skipped`}
            </span>
            <div className="spacer" />
            {op.status === "done" && op.inTrash > 0 && (
              <>
                <button disabled={busy} onClick={() => undo(op)}>
                  Undo
                </button>
                {!moves && (
                  <button disabled={busy} onClick={() => empty(op)}>
                    Send to Recycle Bin
                  </button>
                )}
              </>
            )}
          </li>
        ))}
      </ul>
    </section>
  );
}
