import { confirm } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { api, type OrganizeLayout, type OrganizePreview, type RootInfo } from "../api";
import { fileName } from "../format";
import { describeReport, RecentlyRemoved } from "./RecentlyRemoved";

const LAYOUTS: { id: OrganizeLayout; label: string; example: string }[] = [
  { id: "yearMonth", label: "Year, then month", example: "2023/2023-06/IMG_1234.jpg" },
  { id: "yearMonthNumber", label: "Year, then month number", example: "2023/06/IMG_1234.jpg" },
  { id: "yearDay", label: "Year, then day", example: "2023/2023-06-14/IMG_1234.jpg" },
];

type Props = {
  roots: RootInfo[];
  /** After files moved, so views showing paths can refresh. */
  onChanged: () => void;
  onNotice: (message: string) => void;
  onError: (e: unknown) => void;
};

/** Moves a library folder's files into folders by date: preview first, then undoable. */
export function OrganizePage({ roots, onChanged, onNotice, onError }: Props) {
  const [rootId, setRootId] = useState<number | null>(roots.find((r) => r.online)?.id ?? null);
  const [layout, setLayout] = useState<OrganizeLayout>("yearMonth");
  const [includeUndated, setIncludeUndated] = useState(false);
  const [preview, setPreview] = useState<OrganizePreview | null>(null);
  const [busy, setBusy] = useState(false);
  const [runs, setRuns] = useState(0);
  const root = roots.find((r) => r.id === rootId);

  // The folder list can arrive after the page opens.
  useEffect(() => {
    if (rootId === null) setRootId(roots.find((r) => r.online)?.id ?? null);
  }, [roots, rootId]);

  // A preview is only good for the choices it was made with.
  useEffect(() => setPreview(null), [rootId, layout, includeUndated]);

  const makePreview = async () => {
    if (rootId === null) return;
    setBusy(true);
    try {
      setPreview(await api.organizePreview(rootId, layout, includeUndated));
    } catch (e) {
      onError(e);
    } finally {
      setBusy(false);
    }
  };

  const run = async () => {
    if (rootId === null || !preview || !root) return;
    const ok = await confirm(
      `Move ${preview.moves.toLocaleString()} files into folders by date inside "${fileName(root.path)}"?\n\n` +
        "File names stay the same (a clash gets \" (2)\"). Folders left empty are removed. " +
        "You can move everything back with Undo below.",
      { title: "Organize files", kind: "warning", okLabel: "Move files" },
    );
    if (!ok) return;
    setBusy(true);
    try {
      onNotice(describeReport(await api.organizeRun(rootId, layout, includeUndated), "Moved"));
      setPreview(null);
      setRuns((n) => n + 1);
      onChanged();
    } catch (e) {
      onError(e);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="events-page cleanup-page">
      <section className="cleanup-summary">
        <h2>Organize files</h2>
        <p className="muted small">
          Moves the photos and videos of one library folder into folders by the date they were taken. Nothing moves
          until you've seen the preview, and every move can be undone.
        </p>
        {roots.length === 0 ? (
          <p className="muted">Add a folder to the library first.</p>
        ) : (
          <div className="organize-form">
            <label>
              <span className="muted small">Folder</span>
              <select value={rootId ?? ""} onChange={(e) => setRootId(Number(e.target.value))}>
                {roots.map((r) => (
                  <option key={r.id} value={r.id} disabled={!r.online}>
                    {r.path}
                    {r.online ? "" : " (not connected)"}
                  </option>
                ))}
              </select>
            </label>
            <fieldset>
              <legend className="muted small">Layout</legend>
              <div className="organize-layouts">
                {LAYOUTS.map((l) => (
                  <label key={l.id} className={`organize-layout${layout === l.id ? " chosen" : ""}`}>
                    <span className="organize-layout-name">
                      <input type="radio" checked={layout === l.id} onChange={() => setLayout(l.id)} />
                      {l.label}
                    </span>
                    {/* The example path drawn as the folders it makes. */}
                    <code className="organize-tree">
                      {l.example.split("/").map((part, i, parts) => (
                        <span key={i} style={{ paddingLeft: i * 14 }}>
                          {part}
                          {i < parts.length - 1 ? "/" : ""}
                        </span>
                      ))}
                    </code>
                  </label>
                ))}
              </div>
            </fieldset>
            <label className="organize-option">
              <input type="checkbox" checked={includeUndated} onChange={(e) => setIncludeUndated(e.target.checked)} />
              <span>
                Also move files that only have a file date
                <span className="muted small">Often the date they were copied, not the date they were taken.</span>
              </span>
            </label>
            <div>
              <button className="primary" disabled={busy || !root?.online} onClick={makePreview}>
                Preview
              </button>
            </div>
          </div>
        )}
      </section>

      {preview && (
        <section className="cleanup-summary">
          {preview.moves === 0 ? (
            <p>Everything is already in place.</p>
          ) : (
            <>
              <p>
                <strong>{preview.moves.toLocaleString()}</strong> files would move
                {preview.inPlace > 0 && `, ${preview.inPlace.toLocaleString()} are already in place`}
                {preview.renamed > 0 && `, ${preview.renamed.toLocaleString()} get " (2)" because the name is taken`}.
              </p>
              <button className="primary" disabled={busy} onClick={run}>
                Move {preview.moves.toLocaleString()} files
              </button>
            </>
          )}
          {preview.undated > 0 && (
            <p className="muted small">
              {preview.undated.toLocaleString()} files stay where they are: their only date is the file's own date.
            </p>
          )}
          {preview.sample.length > 0 && (
            <table className="organize-sample">
              <thead>
                <tr>
                  <th>From</th>
                  <th>To</th>
                </tr>
              </thead>
              <tbody>
                {preview.sample.map((m) => (
                  <tr key={m.fileId}>
                    <td className="muted">{m.from}</td>
                    <td>{m.to}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          {preview.moves > preview.sample.length && (
            <p className="muted small">…and {(preview.moves - preview.sample.length).toLocaleString()} more.</p>
          )}
        </section>
      )}

      <RecentlyRemoved kind="move" version={runs} onChanged={onChanged} onNotice={onNotice} onError={onError} />
    </div>
  );
}
