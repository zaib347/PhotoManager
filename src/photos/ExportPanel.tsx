import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { api, type ExportFormat, type ExportOptions, type Watermark, type WatermarkPosition } from "../api";

const SETTINGS_KEY = "export.settings";
const WATERMARK_KEY = "export.watermark";

const FORMATS: { id: ExportFormat; label: string }[] = [
  { id: "original", label: "Same as original" },
  { id: "jpeg", label: "JPEG" },
  { id: "webp", label: "WebP" },
  { id: "png", label: "PNG" },
];

const SIZES: { edge: number | null; label: string }[] = [
  { edge: null, label: "Full size" },
  { edge: 4096, label: "Large (4096 px)" },
  { edge: 2048, label: "Medium (2048 px)" },
  { edge: 1080, label: "Small (1080 px)" },
];

const POSITIONS: { id: WatermarkPosition; label: string }[] = [
  { id: "topLeft", label: "Top left" },
  { id: "topRight", label: "Top right" },
  { id: "center", label: "Center" },
  { id: "bottomLeft", label: "Bottom left" },
  { id: "bottomRight", label: "Bottom right" },
  { id: "tiled", label: "Tiled" },
];

const DEFAULTS: ExportOptions = {
  format: "original",
  quality: 85,
  maxEdge: null,
  removeLocation: false,
  watermark: null,
  folder: "",
};

const MARK_DEFAULTS: Watermark = { text: "", logo: null, position: "bottomRight", size: 5, opacity: 70 };

type MarkSettings = { on: boolean; mark: Watermark };

/** Last used settings, so repeated exports are one click. */
function loadSettings(): ExportOptions {
  try {
    return { ...DEFAULTS, ...JSON.parse(localStorage.getItem(SETTINGS_KEY) ?? "{}"), watermark: null };
  } catch {
    return DEFAULTS;
  }
}

/** The watermark is remembered even while it is switched off. */
function loadMark(): MarkSettings {
  try {
    const saved = JSON.parse(localStorage.getItem(WATERMARK_KEY) ?? "{}");
    return { on: saved.on === true, mark: { ...MARK_DEFAULTS, ...saved.mark } };
  } catch {
    return { on: false, mark: MARK_DEFAULTS };
  }
}

function saveSettings(options: ExportOptions, marking: MarkSettings) {
  try {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify({ ...options, watermark: null }));
    localStorage.setItem(WATERMARK_KEY, JSON.stringify(marking));
  } catch {
    // Remembering is a convenience only.
  }
}

const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;

/** The watermark drawn on `previewId`, redrawn shortly after the settings stop changing. */
function useWatermarkPreview(previewId: number, mark: Watermark | null) {
  const [url, setUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const key = mark ? JSON.stringify(mark) : null;

  useEffect(() => {
    if (!key) return;
    let stale = false;
    const timer = window.setTimeout(() => {
      api
        .watermarkPreview(previewId, JSON.parse(key) as Watermark)
        .then((blob) => {
          if (stale) return;
          setUrl(URL.createObjectURL(blob));
          setError(null);
        })
        .catch((e) => !stale && setError(String(e)));
    }, 200);
    return () => {
      stale = true;
      window.clearTimeout(timer);
    };
  }, [previewId, key]);

  // Each preview replaces the last; free the old image once it is off screen.
  useEffect(() => {
    if (!url) return;
    return () => URL.revokeObjectURL(url);
  }, [url]);

  return { url, error };
}

type Props = {
  count: number;
  /** The photo the watermark preview is drawn on. */
  previewId: number;
  onExport: (options: ExportOptions) => void;
};

/** Format, size, quality, privacy, watermark and destination for exporting copies. */
export function ExportPanel({ count, previewId, onExport }: Props) {
  const [options, setOptions] = useState<ExportOptions>(loadSettings);
  const [marking, setMarking] = useState<MarkSettings>(loadMark);
  const set = (patch: Partial<ExportOptions>) => setOptions((o) => ({ ...o, ...patch }));
  const setMark = (patch: Partial<Watermark>) => setMarking((m) => ({ ...m, mark: { ...m.mark, ...patch } }));
  const lossy = options.format !== "png";
  const { mark } = marking;
  const hasMark = marking.on && (mark.logo !== null || mark.text.trim() !== "");
  const preview = useWatermarkPreview(previewId, hasMark ? mark : null);

  const chooseFolder = async () => {
    const folder = await open({ directory: true, multiple: false, title: "Export to folder" });
    if (typeof folder === "string") set({ folder });
  };

  const chooseLogo = async () => {
    const logo = await open({
      multiple: false,
      title: "Choose a logo",
      filters: [{ name: "Images", extensions: ["png", "jpg", "jpeg", "webp"] }],
    });
    if (typeof logo === "string") setMark({ logo });
  };

  return (
    <div className="filter-panel export-panel">
      <div className="export-column">
        <section>
          <h4>Format</h4>
          <div className="segmented">
            {FORMATS.map((f) => (
              <button key={f.id} className={options.format === f.id ? "on" : ""} onClick={() => set({ format: f.id })}>
                {f.label}
              </button>
            ))}
          </div>
          {options.format === "original" && <p className="muted small">HEIC and other formats become JPEG.</p>}
        </section>

        {lossy && (
          <section>
            <h4>Quality: {options.quality}</h4>
            <input
              type="range"
              min={40}
              max={100}
              value={options.quality}
              onChange={(e) => set({ quality: Number(e.target.value) })}
              aria-label="Quality"
            />
            <p className="muted small">Lower is smaller. 80–90 looks the same as the original for most photos.</p>
          </section>
        )}

        <section>
          <h4>Size</h4>
          <ul className="check-list">
            {SIZES.map((s) => (
              <li key={s.label}>
                <label>
                  <input type="radio" checked={options.maxEdge === s.edge} onChange={() => set({ maxEdge: s.edge })} />
                  <span className="link-label">{s.label}</span>
                </label>
              </li>
            ))}
          </ul>
        </section>

        <section>
          <h4>Privacy</h4>
          <label className="organize-layout">
            <input
              type="checkbox"
              checked={options.removeLocation}
              onChange={(e) => set({ removeLocation: e.target.checked })}
            />
            <span>Remove location</span>
          </label>
          <p className="muted small">Date and camera are always kept.</p>
        </section>

        <section>
          <h4>Save to</h4>
          <button onClick={chooseFolder}>{options.folder ? "Change folder…" : "Choose folder…"}</button>
          {options.folder && (
            <p className="muted small file-path" title={options.folder}>
              {options.folder}
            </p>
          )}
        </section>
      </div>

      <div className="export-column">
        <section>
          <h4>Watermark</h4>
          <label className="organize-layout">
            <input
              type="checkbox"
              checked={marking.on}
              onChange={(e) => setMarking((m) => ({ ...m, on: e.target.checked }))}
            />
            <span>Add a watermark</span>
          </label>
        </section>

        {marking.on && (
          <>
            <section>
              {mark.logo === null ? (
                <>
                  <input
                    type="text"
                    className="watermark-text"
                    placeholder="© Your name 2026"
                    value={mark.text}
                    onChange={(e) => setMark({ text: e.target.value })}
                    aria-label="Watermark text"
                  />
                  <button onClick={chooseLogo}>Use a logo instead…</button>
                </>
              ) : (
                <>
                  <p className="small file-path" title={mark.logo}>
                    Logo: {fileName(mark.logo)}
                  </p>
                  <div className="segmented">
                    <button onClick={chooseLogo}>Change logo…</button>
                    <button onClick={() => setMark({ logo: null })}>Use text instead</button>
                  </div>
                </>
              )}
            </section>

            <section>
              <h4>Position</h4>
              <div className="segmented">
                {POSITIONS.map((p) => (
                  <button
                    key={p.id}
                    className={mark.position === p.id ? "on" : ""}
                    onClick={() => setMark({ position: p.id })}
                  >
                    {p.label}
                  </button>
                ))}
              </div>
            </section>

            <section>
              <h4>Size: {mark.size}%</h4>
              <input
                type="range"
                min={2}
                max={20}
                value={mark.size}
                onChange={(e) => setMark({ size: Number(e.target.value) })}
                aria-label="Watermark size"
              />
              <h4>Opacity: {mark.opacity}%</h4>
              <input
                type="range"
                min={10}
                max={100}
                step={5}
                value={mark.opacity}
                onChange={(e) => setMark({ opacity: Number(e.target.value) })}
                aria-label="Watermark opacity"
              />
            </section>

            {hasMark && (
              <section className="watermark-preview">
                {preview.error ? (
                  <p className="muted small">No preview: {preview.error}</p>
                ) : (
                  preview.url && <img src={preview.url} alt="Watermark on the first selected photo" />
                )}
              </section>
            )}
          </>
        )}
      </div>

      <button
        className="primary export-go"
        disabled={!options.folder}
        onClick={() => {
          saveSettings(options, marking);
          onExport({ ...options, watermark: hasMark ? mark : null });
        }}
      >
        Export {count.toLocaleString()} {count === 1 ? "photo" : "photos"}
      </button>
    </div>
  );
}
