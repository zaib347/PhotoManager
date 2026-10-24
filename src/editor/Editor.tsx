import { confirm } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useRef, useState } from "react";
import { api, NO_EDIT, type Recipe } from "../api";
import { photoLooks } from "../looks";
import { CropBox, fitAspect } from "./CropBox";

type Tone = Omit<Recipe, "crop" | "straighten">;
type Slider = { key: keyof Tone; label: string; min: number; max: number; step: number };

const LIGHT: Slider[] = [
  { key: "exposure", label: "Exposure", min: -3, max: 3, step: 0.05 },
  { key: "contrast", label: "Contrast", min: -100, max: 100, step: 1 },
  { key: "highlights", label: "Highlights", min: -100, max: 100, step: 1 },
  { key: "shadows", label: "Shadows", min: -100, max: 100, step: 1 },
];

const COLOR: Slider[] = [
  { key: "temperature", label: "Warmth", min: -100, max: 100, step: 1 },
  { key: "tint", label: "Tint", min: -100, max: 100, step: 1 },
  { key: "vibrance", label: "Vibrance", min: -100, max: 100, step: 1 },
  { key: "saturation", label: "Saturation", min: -100, max: 100, step: 1 },
];

const DETAIL: Slider[] = [{ key: "sharpen", label: "Sharpen", min: 0, max: 100, step: 1 }];

/** Looks that set light and colour only, keeping crop and straightening. */
const PRESETS: { name: string; tone: Partial<Tone> }[] = [
  { name: "Vivid", tone: { contrast: 20, vibrance: 35, saturation: 10 } },
  { name: "Warm", tone: { temperature: 35, vibrance: 10 } },
  { name: "Cool", tone: { temperature: -30, vibrance: 5 } },
  { name: "Matte", tone: { contrast: -25, shadows: 25, saturation: -15 } },
  { name: "Dramatic", tone: { contrast: 40, highlights: -35, shadows: -10, vibrance: 10 } },
  { name: "B&W", tone: { saturation: -100, contrast: 20 } },
];

const TONE_DEFAULTS: Tone = (({ crop: _c, straighten: _s, ...tone }) => tone)(NO_EDIT);

/** Width ÷ height choices for the crop; null is any shape, "original" the photo's own. */
const ASPECTS: { label: string; value: number | "original" | null }[] = [
  { label: "Free", value: null },
  { label: "Original", value: "original" },
  { label: "Square", value: 1 },
  { label: "4:3", value: 4 / 3 },
  { label: "3:2", value: 3 / 2 },
  { label: "16:9", value: 16 / 9 },
];

const same = (a: Recipe, b: Recipe) => JSON.stringify(a) === JSON.stringify(b);

/** The photo rendered with `recipe`, redrawn shortly after it stops changing. */
function usePreview(id: number, recipe: Recipe | null) {
  const [url, setUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const key = recipe ? JSON.stringify(recipe) : null;

  useEffect(() => {
    if (!key) return;
    let stale = false;
    const timer = window.setTimeout(() => {
      api
        .editPreview(id, JSON.parse(key) as Recipe)
        .then((blob) => {
          if (stale) return;
          setUrl(URL.createObjectURL(blob));
          setError(null);
        })
        .catch((e) => !stale && setError(String(e)));
    }, 80);
    return () => {
      stale = true;
      window.clearTimeout(timer);
    };
  }, [id, key]);

  useEffect(() => {
    if (!url) return;
    return () => URL.revokeObjectURL(url);
  }, [url]);

  return { url, error };
}

type SliderProps = { slider: Slider; value: number; onChange: (v: number) => void };

function SliderRow({ slider, value, onChange }: SliderProps) {
  const shown = slider.key === "exposure" ? value.toFixed(2) : Math.round(value).toString();
  return (
    <label className="edit-slider" onDoubleClick={() => onChange(TONE_DEFAULTS[slider.key])} title="Double-click to reset">
      <span>{slider.label}</span>
      <span className="edit-value">{value > 0 ? `+${shown}` : shown}</span>
      <input
        type="range"
        min={slider.min}
        max={slider.max}
        step={slider.step}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
      />
    </label>
  );
}

type Props = {
  id: number;
  /** `saved`: the photo's look changed (saved or reverted). */
  onClose: (saved: boolean) => void;
  onError: (e: unknown) => void;
};

/**
 * Non-destructive editor: crop and straighten, light, colour and sharpness. The edit
 * is stored as a recipe in the library and the file is never changed.
 */
export function Editor({ id, onClose, onError }: Props) {
  // What's saved now (null: shown as shot), and what the sliders say.
  const [saved, setSaved] = useState<Recipe | null>(null);
  const [recipe, setRecipe] = useState<Recipe | null>(null);
  const [cropping, setCropping] = useState(false);
  const [aspect, setAspect] = useState<(typeof ASPECTS)[number]["value"]>(null);
  const [comparing, setComparing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [imageAspect, setImageAspect] = useState(1);

  const reportError = useRef(onError);
  reportError.current = onError;

  useEffect(() => {
    api
      .editGet(id)
      .then((r) => {
        setSaved(r);
        setRecipe(r ?? NO_EDIT);
      })
      .catch((e) => reportError.current(e));
  }, [id]);

  // While cropping, the whole straightened photo shows with the crop box on top.
  const shown = recipe && (comparing ? NO_EDIT : cropping ? { ...recipe, crop: null } : recipe);
  const preview = usePreview(id, shown);
  const dirty = recipe !== null && !same(recipe, saved ?? NO_EDIT);

  const set = (patch: Partial<Recipe>) => setRecipe((r) => (r ? { ...r, ...patch } : r));

  const close = useCallback(async () => {
    if (dirty && !(await confirm("Discard your changes to this photo?", { title: "Edit", kind: "warning", okLabel: "Discard" })))
      return;
    onClose(false);
  }, [dirty, onClose]);

  const save = async (next: Recipe | null) => {
    setBusy(true);
    try {
      photoLooks.setVersion(id, await api.editSave(id, next));
      onClose(true);
    } catch (e) {
      onError(e);
      setBusy(false);
    }
  };

  const revert = async () => {
    const ok = await confirm("Show this photo as it was taken? Your edits to it are removed.", {
      title: "Revert to original",
      kind: "warning",
      okLabel: "Revert",
    });
    if (ok) await save(null);
  };

  const auto = async () => {
    if (!recipe) return;
    try {
      setRecipe(await api.editAuto(id, recipe));
    } catch (e) {
      onError(e);
    }
  };

  const pickAspect = (value: (typeof ASPECTS)[number]["value"]) => {
    setAspect(value);
    if (value === null) return;
    const ratio = value === "original" ? imageAspect : imageAspect < 1 ? 1 / value : value;
    set({ crop: fitAspect(ratio, imageAspect) });
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        if (cropping) setCropping(false);
        else close();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [cropping, close]);

  const aspectValue =
    aspect === null ? null : aspect === "original" ? imageAspect : imageAspect < 1 ? 1 / aspect : aspect;

  return (
    <div className="viewer editor" role="dialog" aria-label="Edit photo">
      <header className="viewer-bar">
        <button className="ghost" onClick={close} title="Cancel (Esc)">
          Cancel
        </button>
        <div className="spacer" />
        <button
          className="ghost"
          onPointerDown={() => setComparing(true)}
          onPointerUp={() => setComparing(false)}
          onPointerLeave={() => setComparing(false)}
          disabled={!recipe || cropping}
          title="Hold to see the original"
        >
          Compare
        </button>
        {saved !== null && (
          <button className="ghost" onClick={revert} disabled={busy}>
            Revert to original
          </button>
        )}
        <button className="primary" onClick={() => recipe && save(recipe)} disabled={!dirty || busy}>
          {busy ? "Saving…" : "Save"}
        </button>
      </header>

      <div className="viewer-body">
        <div className="viewer-stage editor-stage">
          {preview.error ? (
            <p className="viewer-message">Can't edit this photo: {preview.error}</p>
          ) : (
            preview.url && (
              <div className="editor-canvas">
                <img
                  src={preview.url}
                  alt=""
                  draggable={false}
                  onLoad={(e) => {
                    // Straightening keeps the shape, so any uncropped render gives it.
                    const img = e.currentTarget;
                    if (cropping || comparing || !recipe?.crop) setImageAspect(img.naturalWidth / img.naturalHeight);
                  }}
                />
                {cropping && recipe && !comparing && (
                  <CropBox
                    crop={recipe.crop}
                    aspect={aspectValue}
                    imageAspect={imageAspect}
                    onChange={(crop) => set({ crop })}
                  />
                )}
              </div>
            )
          )}
        </div>

        {recipe && (
          <aside className="editor-panel">
            <section>
              <div className="segmented">
                <button className={cropping ? "" : "on"} onClick={() => setCropping(false)}>
                  Adjust
                </button>
                <button className={cropping ? "on" : ""} onClick={() => setCropping(true)}>
                  Crop
                </button>
              </div>
            </section>

            {cropping ? (
              <>
                <section>
                  <h4>Shape</h4>
                  <div className="segmented">
                    {ASPECTS.map((a) => (
                      <button key={a.label} className={aspect === a.value ? "on" : ""} onClick={() => pickAspect(a.value)}>
                        {a.label}
                      </button>
                    ))}
                  </div>
                </section>
                <section>
                  <label
                    className="edit-slider"
                    onDoubleClick={() => set({ straighten: 0 })}
                    title="Double-click to reset"
                  >
                    <span>Straighten</span>
                    <span className="edit-value">{recipe.straighten.toFixed(1)}°</span>
                    <input
                      type="range"
                      min={-45}
                      max={45}
                      step={0.1}
                      value={recipe.straighten}
                      onChange={(e) => set({ straighten: Number(e.target.value) })}
                    />
                  </label>
                </section>
                <section>
                  <button
                    onClick={() => {
                      set({ crop: null, straighten: 0 });
                      setAspect(null);
                    }}
                  >
                    Reset crop
                  </button>
                </section>
              </>
            ) : (
              <>
                <section>
                  <button onClick={auto}>✨ Auto-enhance</button>
                </section>
                <section>
                  <h4>Looks</h4>
                  <div className="segmented">
                    {PRESETS.map((p) => (
                      <button key={p.name} onClick={() => set({ ...TONE_DEFAULTS, ...p.tone })}>
                        {p.name}
                      </button>
                    ))}
                  </div>
                </section>
                {[
                  { title: "Light", sliders: LIGHT },
                  { title: "Color", sliders: COLOR },
                  { title: "Detail", sliders: DETAIL },
                ].map((group) => (
                  <section key={group.title}>
                    <h4>{group.title}</h4>
                    {group.sliders.map((s) => (
                      <SliderRow key={s.key} slider={s} value={recipe[s.key]} onChange={(v) => set({ [s.key]: v })} />
                    ))}
                  </section>
                ))}
                <section>
                  <button onClick={() => set(TONE_DEFAULTS)}>Reset light and color</button>
                </section>
              </>
            )}
          </aside>
        )}
      </div>
    </div>
  );
}
