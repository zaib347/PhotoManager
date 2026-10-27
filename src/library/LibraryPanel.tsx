import { open } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
import { api, thumbUrl, viewKey, type BuiltinView, type RootInfo, type ScanProgress, type View } from "../api";
import { useCollections } from "../collections/context";
import { Icon, type IconName } from "../ui/Icon";
import { Popover } from "../ui/Popover";
import { ImportPanel } from "./ImportPanel";
import { ScanSection, ScanSettings } from "./ScanSettings";
import { XmpSettings } from "./XmpSettings";

type ViewLink = { id: BuiltinView; label: string; icon: IconName };

const SECTIONS: { title: string; views: ViewLink[] }[] = [
  {
    title: "Library",
    views: [
      { id: "all", label: "Photos", icon: "photos" },
      { id: "favorites", label: "Favorites", icon: "heart" },
      { id: "videos", label: "Videos", icon: "video" },
      { id: "people", label: "People", icon: "people" },
      { id: "nopeople", label: "No people", icon: "mountain" },
      { id: "hidden", label: "Hidden", icon: "eyeOff" },
    ],
  },
  {
    title: "Memories",
    views: [
      { id: "events", label: "Events", icon: "calendar" },
      { id: "trips", label: "Trips", icon: "trip" },
      { id: "places", label: "Places", icon: "pin" },
      { id: "map", label: "Map", icon: "map" },
    ],
  },
  {
    title: "Clean up",
    views: [
      { id: "duplicates", label: "Duplicates", icon: "copy" },
      { id: "similar", label: "Similar photos", icon: "layers" },
      { id: "organize", label: "Organize files", icon: "folder" },
      { id: "removed", label: "Recently removed", icon: "trash" },
    ],
  },
  { title: "Smart", views: [{ id: "ai", label: "AI features", icon: "sparkles" }] },
];

type Props = {
  view: View;
  onView: (view: View) => void;
  roots: RootInfo[];
  progress: Map<number, ScanProgress>;
  /** Adds a folder (with folders inside it left out) and scans it. */
  onAdd: (path: string, excluded: string[]) => void;
  onRescan: (rootId: number) => void;
  onCancel: (rootId: number) => void;
  onRemove: (root: RootInfo) => void;
  onError: (e: unknown) => void;
  onNotice: (message: string) => void;
  /** An import changed dates, places, albums or favorites. */
  onImported: () => void;
  /** Ready-made searches (name, description), once search by description is ready. */
  things: [string, string][];
  onThing: (name: string, description: string) => void;
  /** People (face grouping) is installed. */
  people: boolean;
};

export function LibraryPanel({
  view,
  onView,
  roots,
  progress,
  onAdd,
  onRescan,
  onCancel,
  onRemove,
  onError,
  onNotice,
  onImported,
  things,
  onThing,
  people,
}: Props) {
  const { albums, tags, reload } = useCollections();
  const [newAlbum, setNewAlbum] = useState<string | null>(null);
  // A folder picked to add, while its scan settings are shown.
  const [adding, setAdding] = useState<string | null>(null);
  const current = viewKey(view);

  const createAlbum = async () => {
    const name = newAlbum?.trim();
    setNewAlbum(null);
    if (!name) return;
    const id = await api.createAlbum(name, []);
    await reload();
    onView({ album: id });
  };

  const pickFolder = async () => {
    const path = await open({ directory: true, multiple: false, title: "Add a folder to your library" });
    // Scanned once its settings are confirmed.
    if (typeof path === "string") setAdding(path);
  };

  return (
    <aside className="sidebar">
      {SECTIONS.map((section) => (
        <nav key={section.title} className="views" aria-label={section.title}>
          <div className="sidebar-title">
            <span>{section.title}</span>
          </div>
          {section.views
            .filter((v) => (v.id !== "people" && v.id !== "nopeople") || people)
            .map((v) => (
              <button
                key={v.id}
                className={`view-link${v.id === current ? " active" : ""}`}
                aria-current={v.id === current ? "page" : undefined}
                onClick={() => onView(v.id)}
              >
                <Icon name={v.icon} />
                <span className="link-label">{v.label}</span>
              </button>
            ))}
        </nav>
      ))}

      <div className="sidebar-title">
        <span>Albums</span>
        <button className="icon-button" onClick={() => setNewAlbum("")} title="New album">
          <Icon name="plus" size={14} />
        </button>
      </div>
      {newAlbum !== null && (
        <input
          className="sidebar-input"
          autoFocus
          placeholder="Album name"
          value={newAlbum}
          onChange={(e) => setNewAlbum(e.target.value)}
          onBlur={() => setNewAlbum(null)}
          onKeyDown={(e) => {
            if (e.key === "Enter") createAlbum();
            if (e.key === "Escape") setNewAlbum(null);
          }}
        />
      )}
      <nav className="views" aria-label="Albums">
        {albums.map((a) => {
          const active = current === viewKey({ album: a.id });
          return (
            <button
              key={a.id}
              className={`view-link album-link${active ? " active" : ""}`}
              aria-current={active ? "page" : undefined}
              onClick={() => onView({ album: a.id })}
            >
              {a.coverAssetId !== null ? (
                <img className="album-cover" src={thumbUrl(a.coverAssetId)} alt="" />
              ) : (
                <span className="album-cover">
                  <Icon name="album" size={14} />
                </span>
              )}
              <span className="link-label">{a.name}</span>
              <span className="muted count">{a.count.toLocaleString()}</span>
            </button>
          );
        })}
        {albums.length === 0 && newAlbum === null && (
          <p className="muted sidebar-hint">Select photos and choose Add to album, or press +.</p>
        )}
      </nav>

      {things.length > 0 && (
        <>
          <div className="sidebar-title">
            <span>Things</span>
          </div>
          <nav className="views" aria-label="Things">
            {things.map(([name, description]) => (
              <button key={name} className="view-link" onClick={() => onThing(name, description)}>
                <Icon name="sparkles" />
                <span className="link-label">{name}</span>
              </button>
            ))}
          </nav>
        </>
      )}

      {tags.length > 0 && (
        <>
          <div className="sidebar-title">
            <span>Tags</span>
          </div>
          <nav className="views" aria-label="Tags">
            {tags.map((t) => {
              const active = current === viewKey({ tag: t.id });
              return (
                <button
                  key={t.id}
                  className={`view-link${active ? " active" : ""}`}
                  aria-current={active ? "page" : undefined}
                  onClick={() => onView({ tag: t.id })}
                >
                  <Icon name="tag" />
                  <span className="link-label">{t.name}</span>
                  <span className="muted count">{t.count.toLocaleString()}</span>
                </button>
              );
            })}
          </nav>
        </>
      )}

      <div className="sidebar-title">
        <span>Folders</span>
        <button className="icon-button" onClick={pickFolder} title="Add folder">
          <Icon name="plus" size={14} />
        </button>
      </div>
      {roots.length === 0 && (
        <p className="muted sidebar-hint">
          Add the folders or drives that hold your photos. Files stay where they are.
          <button className="primary sidebar-add" onClick={pickFolder}>
            <Icon name="plus" size={14} />
            Add folder
          </button>
        </p>
      )}
      <ul className="roots">
        {roots.map((root) => {
          const p = progress.get(root.id);
          return (
            <li key={root.id} className="root">
              <div className="root-head">
                <span
                  className={`dot ${root.online ? "online" : "offline"}`}
                  title={root.online ? "Connected" : "Drive not connected"}
                />
                <span className="root-path" title={root.path}>
                  {folderName(root.path)}
                </span>
              </div>
              <div className="root-meta muted">
                {root.volumeLabel ? `${root.volumeLabel} · ` : ""}
                {root.files.toLocaleString()} files
                {!root.online && " · offline"}
              </div>
              {p ? (
                <ScanStatus progress={p} onCancel={() => onCancel(root.id)} />
              ) : (
                <div className="root-actions">
                  <button disabled={!root.online} onClick={() => onRescan(root.id)}>
                    Rescan
                  </button>
                  <button onClick={() => onRemove(root)}>Remove</button>
                  <Popover label="Import…" title="Google Takeout or iCloud export" buttonClass="ghost">
                    {() => (
                      <ImportPanel rootId={root.id} onChanged={onImported} onNotice={onNotice} onError={onError} />
                    )}
                  </Popover>
                  {root.lastScanAt && (
                    <span className="muted">scanned {new Date(root.lastScanAt).toLocaleDateString()}</span>
                  )}
                </div>
              )}
            </li>
          );
        })}
      </ul>

      <div className="sidebar-title">
        <span>Scanning</span>
      </div>
      <ScanSection roots={roots} onError={onError} />

      <div className="sidebar-title">
        <span>Sidecars</span>
      </div>
      <XmpSettings onError={onError} />
      {adding !== null && (
        <ScanSettings
          target={{ kind: "add", path: adding }}
          roots={roots}
          onStart={onAdd}
          onClose={() => setAdding(null)}
          onError={onError}
        />
      )}
    </aside>
  );
}

function ScanStatus({ progress: p, onCancel }: { progress: ScanProgress; onCancel: () => void }) {
  const label =
    p.phase === "discovering"
      ? `Finding files… ${p.discovered.toLocaleString()}`
      : p.phase === "processing"
        ? `Indexing ${p.processed.toLocaleString()} / ${p.toProcess.toLocaleString()}`
        : "Finishing…";
  const fraction = p.phase === "processing" && p.toProcess > 0 ? p.processed / p.toProcess : null;
  return (
    <div className="scan-status">
      <div className="progress">
        <div
          className={fraction === null ? "progress-bar indeterminate" : "progress-bar"}
          style={fraction === null ? undefined : { width: `${fraction * 100}%` }}
        />
      </div>
      <div className="root-actions">
        <span className="muted">
          {label}
          {p.errors > 0 && ` · ${p.errors} unreadable`}
        </span>
        <button onClick={onCancel}>Stop</button>
      </div>
    </div>
  );
}

function folderName(path: string) {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}
