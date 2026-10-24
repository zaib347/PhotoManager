import { useEffect, useState } from "react";
import { api, type ImportInfo, type ImportReport } from "../api";

type Props = {
  rootId: number;
  /** Dates, places, albums or favorites changed: reload what shows them. */
  onChanged: () => void;
  onNotice: (message: string) => void;
  onError: (e: unknown) => void;
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
export function ImportPanel({ rootId, onChanged, onNotice, onError }: Props) {
  const [list, setList] = useState<ImportInfo[] | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api.listImports(rootId).then(setList).catch(onError);
  }, [rootId, onError]);

  const run = async () => {
    setBusy(true);
    try {
      const r = await api.importMetadata(rootId);
      onNotice(
        `Matched ${r.matched.toLocaleString()} of ${r.found.toLocaleString()} items from the export: ${describeImport(r)}.`,
      );
      setList(await api.listImports(rootId));
      onChanged();
    } catch (e) {
      onError(e);
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
        For a folder that holds a <strong>Google Takeout</strong> or <strong>iCloud Photos</strong> export: reads the
        dates, places, favorites, hidden photos, descriptions and albums those services keep beside the photos.
      </p>
      <p className="muted small">
        Only fills in what's missing: a date only when the photo has none of its own, a place only without GPS. Scan
        the folder first.
      </p>
      <button className="primary" onClick={run} disabled={busy}>
        {busy ? "Working…" : "Import from export"}
      </button>
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
