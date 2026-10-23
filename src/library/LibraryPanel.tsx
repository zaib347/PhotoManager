import { open } from "@tauri-apps/plugin-dialog";
import type { RootInfo, ScanProgress } from "../api";

type Props = {
  roots: RootInfo[];
  progress: Map<number, ScanProgress>;
  onAdd: (path: string) => void;
  onRescan: (rootId: number) => void;
  onCancel: (rootId: number) => void;
  onRemove: (root: RootInfo) => void;
};

export function LibraryPanel({ roots, progress, onAdd, onRescan, onCancel, onRemove }: Props) {
  const pickFolder = async () => {
    const path = await open({ directory: true, multiple: false, title: "Add a folder to your library" });
    if (typeof path === "string") onAdd(path);
  };

  return (
    <aside className="sidebar">
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
