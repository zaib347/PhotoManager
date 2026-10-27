import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { api, type ExportFormat, type ExportOptions, type Watermark, type WatermarkPosition } from "../api";
import { Icon } from "../ui/Icon";

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
  { edge: 4096, label: "Large" },
  { edge: 2048, label: "Medium" },
  { edge: 1080, label: "Small" },
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
  enlarge: null,
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
  // Enlarging is offered once the AI pack's Upscale is installed.
  const [canEnlarge, setCanEnlarge] = useState(false);
  useEffect(() => {
    api
      .aiStatus()
      .then((s) => setCanEnlarge(s.features.some((f) => f.id === "upscale" && f.installed)))
      .catch(() => setCanEnlarge(false));
  }, []);
  const enlarge = canEnlarge ? options.enlarge : null;

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

  const size = SIZES.find((s) => s.edge === options.maxEdge) ?? SIZES[0];
  const photos = `${count.toLocaleString()} ${count === 1 ? "photo" : "photos"}`;

  return (
    <div className="export-panel">
      <header className="panel-head">
        <h3>Export {photos}</h3>
        <p className="muted small">Copies go to a folder you choose. The originals are not changed.</p>
      </header>

      <div className="export-body">
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
            {options.format === "original" && <p className="hint">HEIC and other formats become JPEG.</p>}
          </section>

          {lossy && (
            <section>
              <div className="field-head">
                <h4>Quality</h4>
                <span className="field-value">{options.quality}</span>
              </div>
              <input
                type="range"
                min={40}
                max={100}
                value={options.quality}
                onChange={(e) => set({ quality: Number(e.target.value) })}
                aria-label="Quality"
              />
              <p className="hint">Lower is smaller. 80–90 looks the same as the original for most photos.</p>
            </section>
          )}

          <section>
            <h4>Size</h4>
            <div className="segmented">
              {SIZES.map((s) => (
                <button key={s.label} className={options.maxEdge === s.edge ? "on" : ""} onClick={() => set({ maxEdge: s.edge })}>
                  {s.label}
                </button>
              ))}
            </div>
            <p className="hint">{size.edge === null ? "Each photo keeps its size." : `Longest side ${size.edge} px at most.`}</p>
          </section>

          {canEnlarge && (
            <section>
              <h4>Enlarge (AI)</h4>
              <div className="segmented">
                {([null, 2, 4] as const).map((f) => (
                  <button key={f ?? 0} className={enlarge === f ? "on" : ""} onClick={() => set({ enlarge: f })}>
                    {f === null ? "Off" : `${f}×`}
                  </button>
                ))}
              </div>
              {enlarge !== null && (
                <p className="hint">
                  Photos up to 2048 px are enlarged with added detail, each taking from a few seconds to a minute or
                  two. Bigger ones are exported as they are.
                </p>
              )}
            </section>
          )}

          <section>
            <h4>Privacy</h4>
            <label className="switch">
              <input
                type="checkbox"
                checked={options.removeLocation}
                onChange={(e) => set({ removeLocation: e.target.checked })}
              />
              <span className="switch-track" aria-hidden="true" />
              <span>Remove location</span>
            </label>
            <p className="hint">Date and camera are always kept.</p>
          </section>
        </div>

        <div className="export-column">
          <section>
            <h4>Watermark</h4>
            <label className="switch">
              <input
                type="checkbox"
                checked={marking.on}
                onChange={(e) => setMarking((m) => ({ ...m, on: e.target.checked }))}
              />
              <span className="switch-track" aria-hidden="true" />
              <span>Add a watermark</span>
            </label>
            {!marking.on && <p className="hint">Your name or logo on every copy.</p>}
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
                    <button className="link small" onClick={chooseLogo}>
                      Use a logo instead…
                    </button>
                  </>
                ) : (
                  <>
                    <p className="small file-path" title={mark.logo}>
                      Logo: {fileName(mark.logo)}
                    </p>
                    <div className="inline-actions">
                      <button className="link small" onClick={chooseLogo}>
                        Change logo…
                      </button>
                      <button className="link small" onClick={() => setMark({ logo: null })}>
                        Use text instead
                      </button>
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
                <div className="field-head">
                  <h4>Size</h4>
                  <span className="field-value">{mark.size}%</span>
                </div>
                <input
                  type="range"
                  min={2}
                  max={20}
                  value={mark.size}
                  onChange={(e) => setMark({ size: Number(e.target.value) })}
                  aria-label="Watermark size"
                />
                <div className="field-head">
                  <h4>Opacity</h4>
                  <span className="field-value">{mark.opacity}%</span>
                </div>
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
                    <p className="hint">No preview: {preview.error}</p>
                  ) : (
                    preview.url && <img src={preview.url} alt="Watermark on the first selected photo" />
                  )}
                </section>
              )}
            </>
          )}
        </div>
      </div>

      <footer className="export-foot">
        <button className="folder-button" onClick={chooseFolder} title={options.folder || undefined}>
          <Icon name="folder" />
          <span className="link-label">{options.folder ? fileName(options.folder) : "Choose a folder…"}</span>
        </button>
        <div className="spacer" />
        <button
          className="primary"
          disabled={!options.folder}
          title={options.folder ? undefined : "Choose a folder first"}
          onClick={() => {
            saveSettings(options, marking);
            onExport({ ...options, enlarge, watermark: hasMark ? mark : null });
          }}
        >
          Export {photos}
        </button>
      </footer>
    </div>
  );
}
