import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { api, type RootInfo, type ScanRules } from "../api";
import { Icon } from "../ui/Icon";

const KB = 1024;
const SIZES: [number, string][] = [
  [0, "Off"],
  [20 * KB, "20 KB"],
  [50 * KB, "50 KB"],
  [100 * KB, "100 KB"],
];
const SIDES: [number, string][] = [
  [0, "Off"],
  [200, "200 px"],
  [400, "400 px"],
  [800, "800 px"],
];
const LENGTHS: [number, string][] = [
  [0, "Off"],
  [3000, "3 s"],
  [10000, "10 s"],
];

/** Adding a folder (scanned on Start), or changing the settings from the sidebar. */
export type ScanTarget = { kind: "add"; path: string } | { kind: "edit" };

/** The scan settings in a few words, for the sidebar. */
export function describeRules(r: ScanRules, excludedFolders: number): string {
  const parts: string[] = [];
  const size = SIZES.find(([v]) => v === r.minBytes)?.[1] ?? `${Math.round(r.minBytes / KB)} KB`;
  if (r.minBytes) parts.push(`images under ${size}`);
  if (r.minSide) parts.push(`images under ${r.minSide} px`);
  const types = [!r.png && "PNG", !r.gif && "GIF", !r.raw && "RAW", !r.videos && "videos"].filter(Boolean);
  if (types.length) parts.push(types.join(", "));
  if (r.videos && r.minVideoMs) parts.push(`videos under ${r.minVideoMs / 1000} s`);
  if (excludedFolders) parts.push(`${excludedFolders} ${excludedFolders === 1 ? "folder" : "folders"}`);
  return parts.length ? `Leaving out ${parts.join(" · ")}` : "Every photo and video is added";
}

/** The sidebar's Scanning section: the settings in a line, and Change… to edit them. */
export function ScanSection({ roots, onError }: { roots: RootInfo[]; onError: (e: unknown) => void }) {
  const [summary, setSummary] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const [version, setVersion] = useState(0);
  const rootIds = roots.map((r) => r.id).join(",");

  useEffect(() => {
    const ids = rootIds ? rootIds.split(",").map(Number) : [];
    Promise.all([api.scanSettings(), Promise.all(ids.map((id) => api.rootExcluded(id)))])
      .then(([rules, lists]) => setSummary(describeRules(rules, lists.reduce((n, l) => n + l.length, 0))))
      .catch(onError);
  }, [rootIds, version, onError]);

  return (
    <div className="scan-section">
      <p className="muted small">{summary ?? "…"}</p>
      <button className="link small" onClick={() => setEditing(true)}>
        Change…
      </button>
      {editing && (
        <ScanSettings
          target={{ kind: "edit" }}
          roots={roots}
          onClose={() => {
            setEditing(false);
            setVersion((v) => v + 1);
          }}
          onError={onError}
        />
      )}
    </div>
  );
}

/** Whether `folder` is strictly inside `base` (Windows paths: any case, either slash). */
function inside(base: string, folder: string) {
  const norm = (p: string) => p.replace(/[\\/]+$/, "").replace(/\//g, "\\").toLowerCase();
  return norm(folder).startsWith(norm(base) + "\\");
}

function shown(base: string, folder: string) {
  return folder.slice(base.replace(/[\\/]+$/, "").length + 1);
}

type Props = {
  target: ScanTarget;
  roots: RootInfo[];
  /** Adding: the folder is added and scanned with these folders left out. */
  onStart?: (path: string, excluded: string[]) => void;
  onClose: () => void;
  onError: (e: unknown) => void;
};

/**
 * What scans add: small images, file types and short videos (for the whole library),
 * and folders left out (per library folder). Shown before a new folder is scanned.
 */
export function ScanSettings({ target, roots, onStart, onClose, onError }: Props) {
  const [rules, setRules] = useState<ScanRules | null>(null);
  // Folders left out, by library folder ("new" for the one being added).
  const [excluded, setExcluded] = useState<Record<string, string[]>>({});
  const [saving, setSaving] = useState(false);
  const adding = target.kind === "add";
  // Saving before the lists arrive would empty them.
  const [foldersLoaded, setFoldersLoaded] = useState(adding);
  const folders: { key: string; path: string; online: boolean; rootId?: number }[] =
    target.kind === "add"
      ? [{ key: "new", path: target.path, online: true }]
      : roots.map((r) => ({ key: String(r.id), path: r.path, online: r.online, rootId: r.id }));

  useEffect(() => {
    api.scanSettings().then(setRules).catch(onError);
    if (target.kind === "edit") {
      Promise.all(roots.map((r) => api.rootExcluded(r.id).then((list) => [String(r.id), list] as const)))
        .then((pairs) => {
          setExcluded(Object.fromEntries(pairs));
          setFoldersLoaded(true);
        })
        .catch(onError);
    }
    // Loaded once per opening.
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  const set = (patch: Partial<ScanRules>) => setRules((r) => (r ? { ...r, ...patch } : r));

  const addExcluded = async (key: string, base: string) => {
    const picked = await open({ directory: true, multiple: false, defaultPath: base, title: "Leave out a folder" });
    if (typeof picked !== "string") return;
    if (!inside(base, picked)) {
      onError(`Choose a folder inside ${base}`);
      return;
    }
    setExcluded((m) => {
      const list = m[key] ?? [];
      return list.some((p) => p.toLowerCase() === picked.toLowerCase()) ? m : { ...m, [key]: [...list, picked] };
    });
  };

  const save = async () => {
    if (!rules) return;
    setSaving(true);
    try {
      await api.setScanSettings(rules);
      if (target.kind === "add") {
        onStart?.(target.path, excluded.new ?? []);
      } else {
        for (const f of folders) {
          if (f.rootId !== undefined && f.online) await api.setRootExcluded(f.rootId, excluded[f.key] ?? []);
        }
      }
      onClose();
    } catch (e) {
      onError(e);
    } finally {
      setSaving(false);
    }
  };

  const choice = (options: [number, string][], value: number, pick: (v: number) => void, disabled = false) => (
    <div className="segmented">
      {options.map(([v, label]) => (
        <button key={v} className={value === v ? "on" : ""} disabled={disabled} onClick={() => pick(v)}>
          {label}
        </button>
      ))}
    </div>
  );

  return createPortal(
    <div className="confirm-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="confirm-dialog scan-settings" role="dialog" aria-modal="true" aria-labelledby="scan-settings-title">
        <div className="scan-settings-body">
          <h3 id="scan-settings-title">Scan settings</h3>
          {adding && <p className="muted small scan-settings-path">{target.path}</p>}
          {!rules ? (
            <p className="muted">Loading…</p>
          ) : (
            <>
              <section>
                <h4>Skip images smaller than</h4>
                <div className="scan-settings-row">
                  <span className="muted small">File size</span>
                  {choice(SIZES, rules.minBytes, (v) => set({ minBytes: v }))}
                </div>
                <div className="scan-settings-row">
                  <span className="muted small">Longest side</span>
                  {choice(SIDES, rules.minSide, (v) => set({ minSide: v }))}
                </div>
                <p className="muted small">
                  Pixel size is the better guide: icons, stickers and thumbnails are small in pixels, while a real photo
                  can be a small file (WhatsApp, old phones).
                </p>
              </section>
              <section>
                <h4>File types</h4>
                <div className="scan-settings-types">
                  <label>
                    <input type="checkbox" checked disabled /> Photos (JPEG, HEIC, WebP…)
                  </label>
                  {(
                    [
                      ["png", "PNG (often screenshots)"],
                      ["gif", "GIF"],
                      ["raw", "RAW (DNG, CR2, NEF, ARW…)"],
                      ["videos", "Videos"],
                    ] as const
                  ).map(([key, label]) => (
                    <label key={key}>
                      <input type="checkbox" checked={rules[key]} onChange={(e) => set({ [key]: e.target.checked })} />{" "}
                      {label}
                    </label>
                  ))}
                </div>
                <div className="scan-settings-row">
                  <span className="muted small">Skip videos shorter than</span>
                  {choice(LENGTHS, rules.minVideoMs, (v) => set({ minVideoMs: v }), !rules.videos)}
                </div>
              </section>
              <section>
                <h4>Leave out folders</h4>
                {folders.map((f) => (
                  <div key={f.key} className="scan-settings-folder">
                    {!adding && <div className="small">{f.path}</div>}
                    {!f.online ? (
                      <p className="muted small">Drive not connected.</p>
                    ) : (
                      <>
                        {(excluded[f.key] ?? []).map((p) => (
                          <span key={p} className="chip filter-chip">
                            {shown(f.path, p)}
                            <button
                              className="chip-remove"
                              aria-label={`Stop leaving out ${p}`}
                              onClick={() =>
                                setExcluded((m) => ({ ...m, [f.key]: (m[f.key] ?? []).filter((x) => x !== p) }))
                              }
                            >
                              ×
                            </button>
                          </span>
                        ))}
                        <button className="link small" onClick={() => addExcluded(f.key, f.path)}>
                          <Icon name="plus" size={12} /> Leave out a folder…
                        </button>
                      </>
                    )}
                  </div>
                ))}
              </section>
              <p className="muted small">
                These apply to new files found from now on. Photos already in your library stay.
              </p>
            </>
          )}
        </div>
        <div className="confirm-actions">
          <button onClick={onClose}>Cancel</button>
          <button className="primary" disabled={!rules || !foldersLoaded || saving} onClick={save}>
            {adding ? "Start scan" : "Save"}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
