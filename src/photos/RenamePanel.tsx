import { useEffect, useRef, useState } from "react";
import { api, type RenamePreview } from "../api";

const PATTERN_KEY = "rename.pattern";

const PRESETS: { label: string; pattern: string }[] = [
  { label: "Date + time", pattern: "{date} {time}" },
  { label: "Date + number", pattern: "{date} {counter}" },
  { label: "Date + place", pattern: "{date} {place} {counter}" },
  { label: "Event", pattern: "{date} {event} {counter}" },
  { label: "Camera", pattern: "{date} {camera} {counter}" },
  { label: "Add date", pattern: "{date} {original}" },
];

const PARTS: { token: string; hint: string }[] = [
  { token: "date", hint: "2023-06-14" },
  { token: "time", hint: "10.15.30" },
  { token: "year", hint: "2023" },
  { token: "month", hint: "06" },
  { token: "day", hint: "14" },
  { token: "place", hint: "the town it was taken in" },
  { token: "event", hint: "the event's name, else its place" },
  { token: "trip", hint: "the trip's name, else its main town" },
  { token: "camera", hint: "camera model" },
  { token: "counter", hint: "001, 002… for photos that would get the same name" },
  { token: "original", hint: "the current name" },
];

function loadPattern() {
  try {
    return localStorage.getItem(PATTERN_KEY) ?? PRESETS[2].pattern;
  } catch {
    return PRESETS[2].pattern;
  }
}

const baseName = (path: string) => path.split("/").pop() ?? path;

type Props = {
  ids: number[];
  onRename: (pattern: string) => void;
};

/** Pattern, preview and confirm for renaming the selected photos' files. */
export function RenamePanel({ ids, onRename }: Props) {
  const [pattern, setPattern] = useState(loadPattern);
  const [preview, setPreview] = useState<RenamePreview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => {
    let stale = false;
    const timer = window.setTimeout(() => {
      api
        .renamePreview(ids, pattern)
        .then((p) => {
          if (stale) return;
          setPreview(p);
          setError(null);
        })
        .catch((e) => {
          if (stale) return;
          setPreview(null);
          setError(String(e));
        });
    }, 250);
    return () => {
      stale = true;
      window.clearTimeout(timer);
    };
  }, [ids, pattern]);

  /** Puts a part at the cursor (or the end). */
  const insert = (token: string) => {
    const el = input.current;
    const text = `{${token}}`;
    const at = el?.selectionStart ?? pattern.length;
    const end = el?.selectionEnd ?? at;
    const before = pattern.slice(0, at);
    const space = before && !before.endsWith(" ") ? " " : "";
    setPattern(`${before}${space}${text}${pattern.slice(end)}`);
    el?.focus();
  };

  const rename = () => {
    try {
      localStorage.setItem(PATTERN_KEY, pattern);
    } catch {
      // Remembering is a convenience only.
    }
    onRename(pattern);
  };

  return (
    <div className="filter-panel rename-panel">
      <section>
        <h4>New names</h4>
        <div className="segmented">
          {PRESETS.map((p) => (
            <button key={p.label} className={pattern === p.pattern ? "on" : ""} onClick={() => setPattern(p.pattern)}>
              {p.label}
            </button>
          ))}
        </div>
      </section>

      <section>
        <input
          ref={input}
          type="text"
          className="watermark-text"
          value={pattern}
          onChange={(e) => setPattern(e.target.value)}
          aria-label="Name pattern"
          spellCheck={false}
        />
        <div className="rename-parts">
          {PARTS.map((p) => (
            <button key={p.token} className="link small" title={p.hint} onClick={() => insert(p.token)}>
              {`{${p.token}}`}
            </button>
          ))}
        </div>
      </section>

      <section>
        {error ? (
          <p className="small rename-error">{error}</p>
        ) : preview ? (
          <>
            <p className="muted small">
              {preview.files === 0
                ? "These photos already have those names."
                : `${preview.files.toLocaleString()} ${preview.files === 1 ? "file" : "files"} will be renamed` +
                  (preview.sidecars ? `, including ${preview.sidecars.toLocaleString()} sidecar files` : "") +
                  "."}
              {preview.inPlace > 0 && ` ${preview.inPlace.toLocaleString()} already named.`}
              {preview.numbered > 0 && ` ${preview.numbered.toLocaleString()} get a number because the name is taken.`}
            </p>
            {preview.sample.length > 0 && (
              <ul className="rename-sample">
                {preview.sample.slice(0, 8).map((r) => (
                  <li key={r.from} title={`${r.from} → ${r.to}`}>
                    <span className="muted">{baseName(r.from)}</span> → {baseName(r.to)}
                  </li>
                ))}
              </ul>
            )}
          </>
        ) : null}
        <p className="muted small">
          Files stay in their folders. RAW+JPEG pairs, Live Photo clips and .xmp/.aae sidecars are renamed together.
          Undo from Organize files. Apps that keep their own list of your files (like Lightroom) may lose track of
          renamed ones.
        </p>
      </section>

      <button className="primary" disabled={!preview || preview.files === 0} onClick={rename}>
        Rename {preview?.files ? `${preview.files.toLocaleString()} ${preview.files === 1 ? "file" : "files"}` : ""}
      </button>
    </div>
  );
}
