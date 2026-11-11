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
  return parts.length ? parts.join(", ") : "nothing to change";
}

/** A sentence on what an import did, saying why when it changed nothing. */
export function importOutcome(r: ImportReport) {
  const found = r.found.toLocaleString();
  const matched = r.matched.toLocaleString();
  if (r.matched === 0)
    return `None of the export's ${found} items matched a photo in this folder. Scan the folder first, then import again.`;
  const of = r.matched === r.found ? `all ${found}` : `${matched} of ${found}`;
  if (describeImport(r) === "nothing to change")
    return `Matched ${of} items from the export. The photos already had their own dates and places, and the export had no favorites, descriptions or albums to add, so nothing needed changing.`;
  return `Matched ${of} items from the export: ${describeImport(r)}.`;
}

/**
 * Reads what a Google Takeout or iCloud export knows about its photos (dates, places,
 * favorites, albums…) into the library, with a list of past imports to undo.
 */
export function ImportPanel({ rootId, onChanged, onNotice, onError, onImportZips }: Props) {
  const [list, setList] = useState<ImportInfo[] | null>(null);
  const [busy, setBusy] = useState(false);
  /** What the last import did, or why it did not run, shown in the panel (the app's notice is easy to miss). */
  const [outcome, setOutcome] = useState<{ text: string; failed: boolean } | null>(null);

  useEffect(() => {
    api.listImports(rootId).then(setList).catch(onError);
  }, [rootId, onError]);

  const run = async () => {
    setBusy(true);
    setOutcome(null);
    try {
      const r = await api.importMetadata(rootId);
      const text = importOutcome(r);
      onNotice(text);
      setOutcome({ text, failed: false });
      setList(await api.listImports(rootId));
      onChanged();
    } catch (e) {
      setOutcome({ text: String(e), failed: true });
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
      {outcome && <p className={`small${outcome.failed ? " import-problem" : ""}`}>{outcome.text}</p>}
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
