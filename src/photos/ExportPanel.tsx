import { open } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
import type { ExportFormat, ExportOptions } from "../api";

const SETTINGS_KEY = "export.settings";

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

const DEFAULTS: ExportOptions = { format: "original", quality: 85, maxEdge: null, removeLocation: false, folder: "" };

/** Last used settings, so repeated exports are one click. */
function loadSettings(): ExportOptions {
  try {
    return { ...DEFAULTS, ...JSON.parse(localStorage.getItem(SETTINGS_KEY) ?? "{}") };
  } catch {
    return DEFAULTS;
  }
}

function saveSettings(options: ExportOptions) {
  try {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(options));
  } catch {
    // Remembering is a convenience only.
  }
}

type Props = {
  count: number;
  onExport: (options: ExportOptions) => void;
};

/** Format, size, quality, privacy and destination for exporting copies. */
export function ExportPanel({ count, onExport }: Props) {
  const [options, setOptions] = useState<ExportOptions>(loadSettings);
  const set = (patch: Partial<ExportOptions>) => setOptions((o) => ({ ...o, ...patch }));
  const lossy = options.format !== "png";

  const chooseFolder = async () => {
    const folder = await open({ directory: true, multiple: false, title: "Export to folder" });
    if (typeof folder === "string") set({ folder });
  };

  return (
    <div className="filter-panel export-panel">
      <section>
        <h4>Format</h4>
        <div className="segmented">
          {FORMATS.map((f) => (
            <button key={f.id} className={options.format === f.id ? "on" : ""} onClick={() => set({ format: f.id })}>
              {f.label}
            </button>
          ))}
        </div>
        {options.format === "original" && (
          <p className="muted small">HEIC and other formats become JPEG.</p>
        )}
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

      <button
        className="primary"
        disabled={!options.folder}
        onClick={() => {
          saveSettings(options);
          onExport(options);
        }}
      >
        Export {count.toLocaleString()} {count === 1 ? "photo" : "photos"}
      </button>
    </div>
  );
}
