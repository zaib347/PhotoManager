import { open } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
import { api, thumbUrl, viewKey, type BuiltinView, type RootInfo, type ScanProgress, type View } from "../api";
import { useCollections } from "../collections/context";

const VIEWS: { id: BuiltinView; label: string }[] = [
  { id: "all", label: "Photos" },
  { id: "events", label: "Events" },
  { id: "favorites", label: "Favorites" },
  { id: "videos", label: "Videos" },
  { id: "hidden", label: "Hidden" },
];

type Props = {
  view: View;
  onView: (view: View) => void;
  roots: RootInfo[];
  progress: Map<number, ScanProgress>;
  onAdd: (path: string) => void;
  onRescan: (rootId: number) => void;
  onCancel: (rootId: number) => void;
  onRemove: (root: RootInfo) => void;
};

export function LibraryPanel({ view, onView, roots, progress, onAdd, onRescan, onCancel, onRemove }: Props) {
  const { albums, tags, reload } = useCollections();
  const [newAlbum, setNewAlbum] = useState<string | null>(null);
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
    if (typeof path === "string") onAdd(path);
  };

  return (
    <aside className="sidebar">
      <nav className="views" aria-label="Library">
        {VIEWS.map((v) => (
          <button
            key={v.id}
            className={`view-link${v.id === current ? " active" : ""}`}
            aria-current={v.id === current ? "page" : undefined}
            onClick={() => onView(v.id)}
          >
            {v.label}
          </button>
        ))}
      </nav>

      <div className="sidebar-title">
        <span>Albums</span>
        <button className="ghost" onClick={() => setNewAlbum("")} title="New album">
          +
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
                <span className="album-cover" />
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
                  <span className="link-label"># {t.name}</span>
                  <span className="muted count">{t.count.toLocaleString()}</span>
                </button>
              );
            })}
          </nav>
        </>
      )}

      <div className="sidebar-title">
        <span>Folders</span>
        <button onClick={pickFolder}>Add folder</button>
      </div>
      {roots.length === 0 && (
        <p className="muted sidebar-hint">
          Add the folders or drives that hold your photos. Files stay where they are.
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
                  {root.lastScanAt && (
                    <span className="muted">scanned {new Date(root.lastScanAt).toLocaleDateString()}</span>
                  )}
                </div>
              )}
            </li>
          );
        })}
      </ul>
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
