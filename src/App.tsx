import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { confirm } from "./ui/confirm";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  api,
  events,
  filterKey,
  isFilterEmpty,
  isPage,
  viewKey,
  type AlbumInfo,
  type ExportReport,
  type Filter,
  type Layout,
  type RootInfo,
  type ScanFinished,
  type ScanProgress,
  type Stats,
  type TagInfo,
  type View,
  type AiPack,
  type AiStatus,
} from "./api";
import { CollectionsContext, type Collections } from "./collections/context";
import { DuplicatesPage } from "./cleanup/DuplicatesPage";
import { OrganizePage } from "./cleanup/OrganizePage";
import { RecentlyRemoved } from "./cleanup/RecentlyRemoved";
import { SimilarPage } from "./cleanup/SimilarPage";
import { EventsPage } from "./events/EventsPage";
import { formatBytes } from "./format";
import { LibraryPanel } from "./library/LibraryPanel";
import { FilterBar } from "./photos/FilterBar";
import { PhotosView } from "./photos/PhotosView";
import { PlacesPage } from "./places/PlacesPage";
import { MapPage } from "./map/MapPage";
import { AiPage } from "./ai/AiPage";
import { SearchBox } from "./ai/SearchBox";
import { NoPeopleBar } from "./people/NoPeopleBar";
import { PeoplePage } from "./people/PeoplePage";
import { TripsPage } from "./trips/TripsPage";
import { ThemePicker } from "./ui/ThemePicker";
import "./App.css";

/** How often the timeline refreshes while a scan is adding photos. */
const LIVE_RELOAD_MS = 3000;

function emptyState(view: View): { title: string; hint: string } {
  if (typeof view === "object") {
    if ("album" in view)
      return { title: "This album is empty", hint: "Select photos in the timeline and choose Add to album." };
    if ("tag" in view) return { title: "No photos with this tag", hint: "Tags appear here as you add them to photos." };
    if ("trip" in view) return { title: "This trip is empty", hint: "Its photos may be hidden or no longer in your folders." };
    if ("person" in view)
      return { title: "No photos of this person", hint: "Their photos may be hidden or no longer in your folders." };
    return { title: "This event is empty", hint: "Its photos may be hidden or no longer in your folders." };
  }
  return {
    all: { title: "No photos yet", hint: "Add a folder on the left to start building your timeline." },
    favorites: { title: "No favorites yet", hint: "Press F in the viewer, or select photos and choose Favorite." },
    videos: { title: "No videos", hint: "Videos in your folders will appear here." },
    hidden: { title: "Nothing hidden", hint: "Hidden photos stay in your folders but leave the timeline." },
    nopeople: {
      title: "No photos without people yet",
      hint: "People looks at your photos in the background; those with no faces in them appear here.",
    },
    events: { title: "", hint: "" },
    trips: { title: "", hint: "" },
    places: { title: "", hint: "" },
    map: { title: "", hint: "" },
    people: { title: "", hint: "" },
    ai: { title: "", hint: "" },
    duplicates: { title: "", hint: "" },
    similar: { title: "", hint: "" },
    removed: { title: "", hint: "" },
    organize: { title: "", hint: "" },
  }[view];
}

export default function App() {
  const [view, setView] = useState<View>("all");
  // The layout is tagged with its view so a view switch never renders a stale layout.
  const [loaded, setLoaded] = useState<{ key: string; layout: Layout } | null>(null);
  const [filter, setFilter] = useState<Filter>({});
  const filterRef = useRef(filter);
  filterRef.current = filter;
  // Layout identity: the view plus the filters applied to it.
  const key = `${viewKey(view)}|${filterKey(filter)}`;
  const keyRef = useRef(key);
  keyRef.current = key;
  const viewRef = useRef(view);
  viewRef.current = view;
  const [albums, setAlbums] = useState<AlbumInfo[]>([]);
  const [tags, setTags] = useState<TagInfo[]>([]);
  const [stats, setStats] = useState<Stats | null>(null);
  const [roots, setRoots] = useState<RootInfo[]>([]);
  const [progress, setProgress] = useState(new Map<number, ScanProgress>());
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const lastLiveReload = useRef(0);
  // Bumped when scans or background detection may have changed events.
  const [eventsVersion, setEventsVersion] = useState(0);
  const [exporting, setExporting] = useState<{ done: number; total: number } | null>(null);
  const [makingVideo, setMakingVideo] = useState<{ done: number; total: number } | null>(null);
  // Items being placed on the map by hand, and the view to go back to.
  const [placing, setPlacing] = useState<{ ids: number[]; from: View; filter: Filter } | null>(null);

  const reloadTimeline = useCallback(async () => {
    const current = viewRef.current;
    const currentFilter = filterRef.current;
    const currentKey = `${viewKey(current)}|${filterKey(currentFilter)}`;
    if (isPage(current)) {
      setStats(await api.stats());
      return;
    }
    const [s, layout] = await Promise.all([api.stats(), api.layout(current, currentFilter)]);
    setStats(s);
    if (keyRef.current === currentKey) setLoaded({ key: currentKey, layout });
  }, []);

  // The splash in index.html covers startup until the library's first numbers arrive.
  useEffect(() => {
    if (stats) window.hideSplash?.();
  }, [stats]);

  const reloadCollections = useCallback(async () => {
    const [a, t] = await Promise.all([api.listAlbums(), api.listTags()]);
    setAlbums(a);
    setTags(t);
  }, []);

  const reloadRoots = useCallback(async () => setRoots(await api.listRoots()), []);

  const report = useCallback((e: unknown) => setNotice(`Failed: ${e}`), []);

  const [ai, setAi] = useState<AiStatus | null>(null);
  const [aiInstalling, setAiInstalling] = useState<{ pack: AiPack; done: number; total: number } | null>(null);
  const [things, setThings] = useState<[string, string][]>([]);
  // Bumped when faces were found or grouped.
  const [peopleVersion, setPeopleVersion] = useState(0);
  const reloadAi = useCallback(() => {
    api.aiStatus().then(setAi).catch(() => {});
  }, []);
  const installed = (pack: AiPack) => !!ai?.features.some((f) => f.id === pack && f.installed);
  const searchReady = !!ai?.progress.search && ai.progress.search.indexed > 0;

  useEffect(() => {
    reloadAi();
    api.aiSmartAlbums().then(setThings).catch(() => {});
    const unlisten = [
      events.aiInstallProgress((p) => setAiInstalling(p)),
      events.aiInstallFinished(({ pack, error }) => {
        setAiInstalling(null);
        setNotice(
          error
            ? error === "Stopped"
              ? "Install stopped; it resumes where it left off."
              : `Install failed: ${error}`
            : INSTALLED[pack],
        );
        reloadAi();
      }),
      events.aiIndexProgress((p) => setAi((s) => (s ? { ...s, progress: p, indexing: true } : s))),
      events.aiIndexFinished(reloadAi),
      events.aiIndexPaused((paused) => setAi((s) => (s ? { ...s, paused } : s))),
      events.aiPeopleChanged(() => setPeopleVersion((v) => v + 1)),
    ];
    return () => unlisten.forEach((p) => p.then((off) => off()));
  }, [reloadAi]);

  /** Shows photos matching a description (and, when `text`, the words in them), labelled `label`. */
  const searchPhotos = useCallback(
    async (query: string, label: string, text: boolean) => {
      try {
        const ids = await api.aiSearch(query, text);
        if (ids.length === 0) {
          setNotice(`Nothing matched “${label}”.`);
          return;
        }
        setView("all");
        setFilter({ ids, searchText: label });
      } catch (e) {
        report(e);
      }
    },
    [report],
  );

  useEffect(() => {
    reloadTimeline().catch(report);
  }, [key, reloadTimeline, report]);

  useEffect(() => {
    reloadCollections().catch(report);
  }, [reloadCollections, report]);

  const collections = useMemo<Collections>(
    () => ({
      albums,
      tags,
      reload: reloadCollections,
      navigate: setView,
      showDay: (day: number) => {
        setView("all");
        setFilter({ fromDay: day, toDay: day });
      },
      showPlace: (where) => {
        setView("all");
        setFilter(where);
      },
    }),
    [albums, tags, reloadCollections],
  );

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
        reloadCollections().catch(report);
        reloadRoots().catch(report);
        setEventsVersion((v) => v + 1);
      }),
      events.eventsChanged(() => setEventsVersion((v) => v + 1)),
      events.exportProgress((p) => setExporting(p)),
      events.exportFinished(({ report: r, error }) => {
        setExporting(null);
        setNotice(error ? `Export failed: ${error}` : r ? describeExport(r) : "Export finished");
      }),
      events.videoProgress((p) => setMakingVideo(p)),
      events.videoFinished(({ report: r, error }) => {
        setMakingVideo(null);
        if (error) setNotice(error === "Stopped" ? "Video stopped." : `The video couldn't be made: ${error}`);
        else if (r) {
          setNotice(
            `Video saved: ${r.path} (${Math.round(r.seconds)} s, ${r.photos} photos)` +
              (r.musicSkipped ? ". The music file couldn't be read, so it has no sound." : ""),
          );
          revealItemInDir(r.path).catch(() => {});
        }
      }),
      events.volumesChanged(() => reloadRoots().catch(report)),
    ];
    return () => unlisten.forEach((p) => p.then((off) => off()));
  }, [reloadTimeline, reloadRoots, reloadCollections, report]);

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

  const addRoot = (path: string, excluded: string[]) =>
    api
      .addRoot(path, excluded)
      .then(reloadRoots)
      .catch((e) => setNotice(String(e)));

  const removeRoot = async (root: RootInfo) => {
    const ok = await confirm(
      `Remove "${root.path}" from the library?\n\nYour files are not touched. Favorites, tags and albums for photos that only exist in this folder are removed from the library.`,
      { title: "Remove folder", kind: "warning", okLabel: "Remove" },
    );
    if (!ok) return;
    try {
      await api.removeRoot(root.id);
      await Promise.all([reloadRoots(), reloadTimeline(), reloadCollections()]);
    } catch (e) {
      report(e);
    }
  };

  const current = loaded && loaded.key === key ? loaded.layout : null;
  const empty = emptyState(view);

  return (
    <CollectionsContext.Provider value={collections}>
    <div className="app">
      <header className="toolbar">
        <span className="muted small">
          {stats ? `${stats.assets.toLocaleString()} items · ${stats.days.toLocaleString()} days` : "Loading…"}
        </span>
        <div className="spacer" />
        {(installed("search") || installed("text")) && (
          <SearchBox
            placeholder={
              installed("search") && installed("text")
                ? "Search: what's in a photo, or words in it"
                : installed("text")
                  ? "Search words in photos"
                  : undefined
            }
            onSearch={(q) => searchPhotos(q, q, true)}
          />
        )}
        {exporting && (
          <span className="export-status muted">
            Exporting {exporting.done.toLocaleString()} of {exporting.total.toLocaleString()}
            <button className="link small" onClick={() => api.cancelExport().catch(report)}>
              Stop
            </button>
          </span>
        )}
        {makingVideo && (
          <span className="export-status muted">
            Making video {Math.round((makingVideo.done / Math.max(1, makingVideo.total)) * 100)}%
            <button className="link small" onClick={() => api.cancelVideo().catch(report)}>
              Stop
            </button>
          </span>
        )}
        <ThemePicker />
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
          onError={report}
          onNotice={setNotice}
          things={searchReady ? things : []}
          onThing={(name, description) => searchPhotos(description, name, false)}
          people={installed("people")}
          onImported={() => {
            Promise.all([reloadTimeline(), reloadCollections()]).catch(report);
            setEventsVersion((v) => v + 1);
          }}
        />
        <main className="content">
          {!busy && !isPage(view) && stats && stats.assets > 0 && <FilterBar filter={filter} roots={roots} onChange={setFilter} />}
          {!busy && view === "nopeople" && (
            <NoPeopleBar
              things={searchReady ? things : []}
              filter={filter}
              onFilter={setFilter}
              progress={ai?.progress.people ?? null}
              bodies={ai?.progress.bodies ?? null}
              onError={report}
            />
          )}
          {busy ? (
            <div className="empty">{busy}</div>
          ) : view === "events" ? (
            <EventsPage version={eventsVersion} onError={report} />
          ) : view === "trips" ? (
            <TripsPage version={eventsVersion} onError={report} />
          ) : view === "places" ? (
            <PlacesPage version={eventsVersion} onError={report} />
          ) : view === "people" ? (
            <PeoplePage version={peopleVersion} progress={ai?.progress.people ?? null} onError={report} />
          ) : view === "ai" ? (
            <AiPage status={ai} installing={aiInstalling} onChanged={reloadAi} onNotice={setNotice} onError={report} />
          ) : view === "map" ? (
            <MapPage
              placing={placing?.ids ?? null}
              onPlace={(lat, lon) => {
                if (!placing) return;
                api
                  .setLocation(placing.ids, lat, lon)
                  .then((n) => {
                    setNotice(`Location set for ${n.toLocaleString()} ${n === 1 ? "item" : "items"}.`);
                    setPlacing(null);
                    setView(placing.from);
                    setFilter(placing.filter);
                    setEventsVersion((v) => v + 1);
                  })
                  .catch(report);
              }}
              onCancelPlacing={() => {
                if (!placing) return;
                setPlacing(null);
                setView(placing.from);
                setFilter(placing.filter);
              }}
              onError={report}
            />
          ) : view === "duplicates" ? (
            <DuplicatesPage version={eventsVersion} onNotice={setNotice} onError={report} />
          ) : view === "similar" ? (
            <SimilarPage onNotice={setNotice} onError={report} />
          ) : view === "organize" ? (
            <OrganizePage
              roots={roots}
              onChanged={() => reloadRoots().catch(report)}
              onNotice={setNotice}
              onError={report}
            />
          ) : view === "removed" ? (
            <div className="events-page cleanup-page">
              <section className="cleanup-summary">
                <h2>Recently removed</h2>
                <p className="muted small">
                  Removed photos wait in a hidden .photomanager-trash folder next to where they were. Undo puts them
                  back; Send to Recycle Bin frees the space.
                </p>
              </section>
              <RecentlyRemoved
                standalone
                version={eventsVersion}
                onChanged={() => reloadTimeline().catch(report)}
                onNotice={setNotice}
                onError={report}
              />
            </div>
          ) : current && current.days.length > 0 ? (
            <PhotosView
              key={key}
              view={view}
              filter={filter}
              layout={current}
              onLibraryChanged={reloadTimeline}
              onNotice={setNotice}
              onError={report}
              onPlaceOnMap={(ids) => {
                setPlacing({ ids, from: view, filter });
                setView("map");
              }}
            />
          ) : current ? (
            <div className="empty">
              <h2>{isFilterEmpty(filter) ? empty.title : "Nothing matches these filters"}</h2>
              {isFilterEmpty(filter) ? (
                <p className="muted">{empty.hint}</p>
              ) : (
                <button onClick={() => setFilter({})}>Clear filters</button>
              )}
            </div>
          ) : null}
        </main>
      </div>
    </div>
    </CollectionsContext.Provider>
  );
}

/** Said when a feature finishes installing. */
const INSTALLED: Record<AiPack, string> = {
  search: "Search by description is installed. Your photos are being indexed in the background.",
  people: "People is installed. Faces are being found in the background; people appear under People as they're grouped.",
  bodies:
    "People without a visible face is installed. Photos with no face are being looked at in the background; those with someone in them leave No people.",
  upscale: "Upscale is installed. Choose Enlarge when exporting photos.",
  text: "Text in photos is on. Your photos are being read in the background; search for words in them from the search box.",
};

function describeExport(r: ExportReport) {
  const parts = [`Exported ${r.exported.toLocaleString()} photos to ${r.folder}`];
  if (r.bytesIn > 0 && r.exported > 0) {
    parts.push(`${formatBytes(r.bytesIn)} → ${formatBytes(r.bytesOut)}`);
  }
  if (r.skipped) parts.push(`${r.skipped.toLocaleString()} skipped (videos and stopped ones aren't exported)`);
  if (r.failed) parts.push(`${r.failed.toLocaleString()} failed: ${r.errors[0] ?? ""}`);
  return parts.join(". ");
}

function describeFinished({ summary: s, error }: ScanFinished) {
  if (error) return `Scan failed: ${error}`;
  if (!s) return "Scan finished";
  if (!s.completed) return `Scan stopped. ${s.added.toLocaleString()} new items were kept.`;
  const parts = [`${s.added.toLocaleString()} new`, `${s.updated.toLocaleString()} changed`];
  if (s.missing) parts.push(`${s.missing.toLocaleString()} no longer found`);
  if (s.errors) parts.push(`${s.errors.toLocaleString()} unreadable`);
  if (s.skipped) parts.push(`${s.skipped.toLocaleString()} left out by your scan settings`);
  const skipped = s.missingCheckSkipped ? " Some folders couldn't be read, so nothing was marked as missing." : "";
  return `Scan complete: ${parts.join(", ")}.${skipped}`;
}
