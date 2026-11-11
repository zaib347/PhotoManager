import { useEffect, useState } from "react";
import { api, type ImportInfo, type ImportReport } from "../api";

type Props = {
  rootId: number;
  /** Dates, places, albums or favorites changed: reload what shows them. */
  onChanged: () => void;
  onNotice: (message: string) => void;
  onError: (e: unknown) => void;
  /** Unzips an export into a new library folder, then scans and imports it. */
  onImportZips: () => void;
};

/** "3 dates, 2 places, …" for a report. */
export function describeImport(r: ImportReport) {
  const parts: string[] = [];
  const add = (n: number, one: string, many: string) => n > 0 && parts.push(`${n.toLocaleString()} ${n === 1 ? one : many}`);
  add(r.dates, "date", "dates");
  add(r.places, "place", "places");
  add(r.favorites, "favorite", "favorites");
  add(r.hidden, "hidden item", "hidden items");
  add(r.descriptions, "description", "descriptions");
  add(r.albums, "new album", "new albums");
  add(r.albumItems, "album item", "album items");
  return parts.length ? parts.join(", ") : "nothing new";
}

/**
 * Reads what a Google Takeout or iCloud export knows about its photos (dates, places,
 * favorites, albums…) into the library, with a list of past imports to undo.
 */
export function ImportPanel({ rootId, onChanged, onNotice, onError, onImportZips }: Props) {
  const [list, setList] = useState<ImportInfo[] | null>(null);
  const [busy, setBusy] = useState(false);
  /** Why the last import didn't run, shown in the panel (the app's notice is easy to miss). */
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    api.listImports(rootId).then(setList).catch(onError);
  }, [rootId, onError]);

  const run = async () => {
    setBusy(true);
    setProblem(null);
    try {
      const r = await api.importMetadata(rootId);
      onNotice(
        `Matched ${r.matched.toLocaleString()} of ${r.found.toLocaleString()} items from the export: ${describeImport(r)}.`,
      );
      setList(await api.listImports(rootId));
      onChanged();
    } catch (e) {
      setProblem(String(e));
    } finally {
      setBusy(false);
    }
  };

  const undo = async (id: number) => {
    setBusy(true);
    try {
      const n = await api.undoImport(id);
      onNotice(`Import undone (${n.toLocaleString()} changes taken back).`);
      setList(await api.listImports(rootId));
      onChanged();
    } catch (e) {
      onError(e);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="import-panel">
      <p className="small">
        Reads the dates, places, favorites, hidden photos, descriptions and albums that a <strong>Google Takeout</strong>{" "}
        or <strong>iCloud Photos</strong> export keeps beside the photos.
      </p>
      <p className="muted small">
        Only fills in what's missing: a date only when the photo has none of its own, a place only without GPS.
      </p>
      <h4 className="import-heading">Still in zip files?</h4>
      <p className="small">
        Choose all the export's zips and an empty folder: they're unzipped there, the folder is added and scanned,
        and then imported. Needs as much free space as the photos take.
      </p>
      <button className="primary" onClick={onImportZips} disabled={busy}>
        Choose zip files…
      </button>
      <h4 className="import-heading">Already unzipped in this folder?</h4>
      <button onClick={run} disabled={busy}>
        {busy ? "Working…" : "Import from this folder"}
      </button>
      {problem && <p className="small import-problem">{problem}</p>}
      {list && list.length > 0 && (
        <ul className="import-list">
          {list.map((i) => (
            <li key={i.id}>
              <span className="small">
                {new Date(i.createdAt).toLocaleString()} · {describeImport(i.report)}
              </span>
              {i.undone ? (
                <span className="muted small">undone</span>
              ) : (
                i.report.dates +
                  i.report.places +
                  i.report.favorites +
                  i.report.hidden +
                  i.report.descriptions +
                  i.report.albumItems >
                  0 && (
                  <button className="link small" onClick={() => undo(i.id)} disabled={busy}>
                    Undo
                  </button>
                )
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
