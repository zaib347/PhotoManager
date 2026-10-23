import { useCallback, useEffect, useRef, useState } from "react";
import {
  api,
  events,
  type Layout,
  type RootInfo,
  type ScanFinished,
  type ScanProgress,
  type Stats,
  type View,
} from "./api";
import { LibraryPanel } from "./library/LibraryPanel";
import { PhotosView } from "./photos/PhotosView";
import "./App.css";

/** How often the timeline refreshes while a scan is adding photos. */
const LIVE_RELOAD_MS = 3000;

const EMPTY: Record<View, { title: string; hint: string }> = {
  all: { title: "No photos yet", hint: "Add a folder on the left to start building your timeline." },
  favorites: { title: "No favorites yet", hint: "Press F in the viewer, or select photos and choose Favorite." },
  videos: { title: "No videos", hint: "Videos in your folders will appear here." },
  hidden: { title: "Nothing hidden", hint: "Hidden photos stay in your folders but leave the timeline." },
};

export default function App() {
  const [view, setView] = useState<View>("all");
  // The layout is tagged with its view so a view switch never renders a stale layout.
  const [loaded, setLoaded] = useState<{ view: View; layout: Layout } | null>(null);
  const viewRef = useRef(view);
  viewRef.current = view;
  const [stats, setStats] = useState<Stats | null>(null);
  const [roots, setRoots] = useState<RootInfo[]>([]);
  const [progress, setProgress] = useState(new Map<number, ScanProgress>());
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const lastLiveReload = useRef(0);

  const reloadTimeline = useCallback(async () => {
    const current = viewRef.current;
    const [s, layout] = await Promise.all([api.stats(), api.layout(current)]);
    setStats(s);
    if (viewRef.current === current) setLoaded({ view: current, layout });
  }, []);

  const reloadRoots = useCallback(async () => setRoots(await api.listRoots()), []);

  const report = useCallback((e: unknown) => setNotice(`Failed: ${e}`), []);

  useEffect(() => {
    reloadTimeline().catch(report);
  }, [view, reloadTimeline, report]);

  useEffect(() => {
    reloadRoots().catch(report);
    // Scans resumed by the backend at startup emit progress, but seed the map in case
    // a scan is between events.
    api.scanningRoots().then((ids) =>
      setProgress((m) => {
        const next = new Map(m);
        for (const rootId of ids) {
          if (!next.has(rootId)) {
            next.set(rootId, { rootId, phase: "discovering", discovered: 0, toProcess: 0, processed: 0, errors: 0 });
          }
        }
        return next;
      }),
    );
  }, [reloadRoots, report]);

  useEffect(() => {
    const unlisten = [
      events.scanProgress((p) => {
        setProgress((m) => new Map(m).set(p.rootId, p));
        if (p.phase === "processing" && Date.now() - lastLiveReload.current > LIVE_RELOAD_MS) {
          lastLiveReload.current = Date.now();
          reloadTimeline().catch(report);
        }
      }),
      events.scanFinished((f: ScanFinished) => {
        setProgress((m) => {
          const next = new Map(m);
          next.delete(f.rootId);
          return next;
        });
        setNotice(describeFinished(f));
        reloadTimeline().catch(report);
        reloadRoots().catch(report);
      }),
      events.volumesChanged(() => reloadRoots().catch(report)),
    ];
    return () => unlisten.forEach((p) => p.then((off) => off()));
  }, [reloadTimeline, reloadRoots, report]);

  const run = async (label: string, action: () => Promise<string>) => {
    setBusy(label);
    try {
      setNotice(await action());
    } catch (e) {
      report(e);
    } finally {
      setBusy(null);
    }
  };

  const generate = (count: number) =>
    run(`Generating ${count.toLocaleString()} synthetic assets…`, async () => {
      const ms = await api.generateSynthetic(count);
      await reloadTimeline();
      return `Generated ${count.toLocaleString()} test assets in ${(ms / 1000).toFixed(1)} s`;
    });

  const clearAll = () =>
    run("Clearing test data…", async () => {
      await api.clear();
      await reloadTimeline();
      return "Test data cleared";
    });

  const addRoot = (path: string) =>
    api
      .addRoot(path)
      .then(reloadRoots)
      .catch((e) => setNotice(String(e)));

  const removeRoot = async (root: RootInfo) => {
    const ok = window.confirm(
      `Remove "${root.path}" from the library?\n\nYour files are not touched. Favorites, tags and albums for photos that only exist in this folder are removed from the library.`,
    );
    if (!ok) return;
    try {
      await api.removeRoot(root.id);
      await Promise.all([reloadRoots(), reloadTimeline()]);
    } catch (e) {
      report(e);
    }
  };

  const current = loaded && loaded.view === view ? loaded.layout : null;

  return (
    <div className="app">
      <header className="toolbar">
        <strong>PhotoManager</strong>
        <span className="muted">
          {stats ? `${stats.assets.toLocaleString()} items · ${stats.days.toLocaleString()} days` : "Loading…"}
        </span>
        <div className="spacer" />
        <details className="dev-menu">
          <summary className="muted">Test data</summary>
          <div className="dev-menu-items">
            <button disabled={!!busy} onClick={() => generate(100_000)}>Generate 100k</button>
            <button disabled={!!busy} onClick={() => generate(1_000_000)}>Generate 1M</button>
            <button disabled={!!busy} onClick={clearAll}>Clear test data</button>
          </div>
        </details>
      </header>
      {notice && (
        <div className="notice" onClick={() => setNotice(null)} title="Dismiss">
          {notice}
        </div>
      )}
      <div className="body">
        <LibraryPanel
          view={view}
          onView={setView}
          roots={roots}
          progress={progress}
          onAdd={addRoot}
          onRescan={(id) => api.scanRoot(id).catch(report)}
          onCancel={(id) => api.cancelScan(id).catch(report)}
          onRemove={removeRoot}
        />
        <main className="content">
          {busy ? (
            <div className="empty">{busy}</div>
          ) : current && current.days.length > 0 ? (
            <PhotosView key={view} view={view} layout={current} onLibraryChanged={reloadTimeline} onError={report} />
          ) : current ? (
            <div className="empty">
              <h2>{EMPTY[view].title}</h2>
              <p className="muted">{EMPTY[view].hint}</p>
            </div>
          ) : null}
        </main>
      </div>
    </div>
  );
}

function describeFinished({ summary: s, error }: ScanFinished) {
  if (error) return `Scan failed: ${error}`;
  if (!s) return "Scan finished";
  if (!s.completed) return `Scan stopped. ${s.added.toLocaleString()} new items were kept.`;
  const parts = [`${s.added.toLocaleString()} new`, `${s.updated.toLocaleString()} changed`];
  if (s.missing) parts.push(`${s.missing.toLocaleString()} no longer found`);
  if (s.errors) parts.push(`${s.errors.toLocaleString()} unreadable`);
  const skipped = s.missingCheckSkipped ? " Some folders couldn't be read, so nothing was marked as missing." : "";
  return `Scan complete: ${parts.join(", ")}.${skipped}`;
}
