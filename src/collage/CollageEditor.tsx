import { save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { api, mediaUrl } from "../api";
import { cellRect, cropFor, SIZES, TEMPLATES } from "./templates";

/** A photo in a cell: which part of it shows. */
type Slot = { id: number; panX: number; panY: number; zoom: number };

type Props = {
  ids: number[];
  onClose: () => void;
  onNotice: (message: string) => void;
  onError: (e: unknown) => void;
};

const fresh = (id: number): Slot => ({ id, panX: 0.5, panY: 0.5, zoom: 1 });

/**
 * Arranges 2–9 photos on one canvas: pick a layout and a size, swap photos by
 * dragging, move or zoom a photo inside its frame, then save a JPEG at full size.
 */
export function CollageEditor({ ids, onClose, onNotice, onError }: Props) {
  const [slots, setSlots] = useState<Slot[]>(() => ids.map(fresh));
  const [template, setTemplate] = useState(TEMPLATES[0].id);
  const [sizeId, setSizeId] = useState(SIZES[0].id);
  const [portrait, setPortrait] = useState(false);
  const [spacing, setSpacing] = useState(0.02);
  const [radius, setRadius] = useState(0.01);
  const [background, setBackground] = useState("#ffffff");
  const [aspects, setAspects] = useState<Record<number, number>>({});
  const [selected, setSelected] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const stage = useRef<HTMLDivElement>(null);
  const [stageSize, setStageSize] = useState({ w: 800, h: 600 });
  const drag = useRef<{ index: number; x: number; y: number; slot: Slot; moved: boolean; pan: boolean } | null>(null);
  const [dragOver, setDragOver] = useState<number | null>(null);

  const size = SIZES.find((s) => s.id === sizeId) ?? SIZES[0];
  const turned = portrait && size.w !== size.h;
  const width = turned ? size.h : size.w;
  const height = turned ? size.w : size.h;

  useEffect(() => {
    const el = stage.current;
    if (!el) return;
    const observer = new ResizeObserver(([e]) => setStageSize({ w: e.contentRect.width, h: e.contentRect.height }));
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopPropagation();
      if (selected !== null) setSelected(null);
      else onClose();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [selected, onClose]);

  const rects = useMemo(() => {
    const t = TEMPLATES.find((t) => t.id === template) ?? TEMPLATES[0];
    return t.cells(
      slots.length,
      slots.map((s) => aspects[s.id] ?? 1.5),
      width / height,
    );
  }, [template, slots, aspects, width, height]);

  // The canvas drawn as big as fits the stage.
  const scale = Math.min((stageSize.w - 32) / width, (stageSize.h - 32) / height);
  const pw = width * scale;
  const ph = height * scale;
  const short = Math.min(pw, ph);
  const gap = Math.round(spacing * Math.min(width, height)) * scale;

  const update = (index: number, patch: Partial<Slot>) =>
    setSlots((list) => list.map((s, i) => (i === index ? { ...s, ...patch } : s)));

  const onPointerDown = (e: ReactPointerEvent, index: number) => {
    if (e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    drag.current = { index, x: e.clientX, y: e.clientY, slot: slots[index], moved: false, pan: selected === index };
  };

  const onPointerMove = (e: ReactPointerEvent, cw: number, ch: number) => {
    const d = drag.current;
    if (!d) return;
    const dx = e.clientX - d.x;
    const dy = e.clientY - d.y;
    if (Math.abs(dx) + Math.abs(dy) > 4) d.moved = true;
    if (!d.moved) return;
    if (d.pan) {
      // Moving the photo inside its frame: the slack is how far it can go.
      const a = aspects[d.slot.id] ?? 1.5;
      const c = cropFor(a, 1, cw, ch, d.slot.panX, d.slot.panY, d.slot.zoom);
      const slackX = (a - c.w) * c.scale;
      const slackY = (1 - c.h) * c.scale;
      update(d.index, {
        panX: slackX > 0 ? Math.min(1, Math.max(0, d.slot.panX - dx / slackX)) : 0.5,
        panY: slackY > 0 ? Math.min(1, Math.max(0, d.slot.panY - dy / slackY)) : 0.5,
      });
    } else {
      const target = document.elementFromPoint(e.clientX, e.clientY)?.closest<HTMLElement>("[data-cell]");
      setDragOver(target ? Number(target.dataset.cell) : null);
    }
  };

  const onPointerUp = (e: ReactPointerEvent) => {
    const d = drag.current;
    drag.current = null;
    setDragOver(null);
    if (!d) return;
    if (!d.moved) {
      setSelected((s) => (s === d.index ? null : d.index));
      return;
    }
    if (d.pan) return;
    const target = document.elementFromPoint(e.clientX, e.clientY)?.closest<HTMLElement>("[data-cell]");
    const to = target ? Number(target.dataset.cell) : null;
    if (to === null || to === d.index) return;
    setSlots((list) => {
      const next = [...list];
      [next[d.index], next[to]] = [fresh(list[to].id), fresh(list[d.index].id)];
      return next;
    });
    setSelected(null);
  };

  const saveAs = async () => {
    const path = await save({
      title: "Save collage",
      defaultPath: "Collage.jpg",
      filters: [{ name: "JPEG image", extensions: ["jpg", "jpeg"] }],
    });
    if (!path) return;
    setBusy(true);
    try {
      await api.saveCollage(
        {
          width,
          height,
          background,
          spacing,
          radius,
          dpi: size.dpi,
          cells: rects.map((r, i) => ({ ...r, assetId: slots[i].id, ...slots[i] })),
        },
        path,
      );
      onNotice(`Collage saved: ${path}`);
      revealItemInDir(path).catch(() => {});
      onClose();
    } catch (e) {
      onError(e);
      setBusy(false);
    }
  };

  return (
    <div className="viewer editor collage" role="dialog" aria-label="Collage">
      <header className="viewer-bar">
        <button className="ghost" onClick={onClose} title="Cancel (Esc)">
          Cancel
        </button>
        <span className="viewer-title">
          Collage · {slots.length} photos · {width} × {height}
          {size.dpi >= 300 && ` · ${(width / size.dpi).toFixed(1)} × ${(height / size.dpi).toFixed(1)} in`}
        </span>
        <div className="spacer" />
        <button className="primary" onClick={saveAs} disabled={busy}>
          {busy ? "Saving…" : "Save…"}
        </button>
      </header>
      <div className="viewer-body">
        <div className="viewer-stage collage-stage" ref={stage} onClick={(e) => e.target === e.currentTarget && setSelected(null)}>
          <div className="collage-canvas" style={{ width: pw, height: ph, background }}>
            {rects.map((r, i) => {
              const c = cellRect(r, pw, ph, gap);
              const slot = slots[i];
              const a = aspects[slot.id] ?? 1.5;
              const crop = cropFor(a, 1, c.w, c.h, slot.panX, slot.panY, slot.zoom);
              return (
                <div
                  key={i}
                  data-cell={i}
                  className={`collage-cell${selected === i ? " selected" : ""}${dragOver === i ? " drop" : ""}`}
                  style={{
                    left: c.x,
                    top: c.y,
                    width: c.w,
                    height: c.h,
                    borderRadius: Math.min(radius * Math.min(width, height) * scale, Math.min(c.w, c.h) / 2),
                  }}
                  onPointerDown={(e) => onPointerDown(e, i)}
                  onPointerMove={(e) => onPointerMove(e, c.w, c.h)}
                  onPointerUp={onPointerUp}
                  onWheel={(e) =>
                    selected === i && update(i, { zoom: Math.min(4, Math.max(1, slot.zoom - e.deltaY * 0.002)) })
                  }
                >
                  <img
                    src={mediaUrl(slot.id)}
                    alt=""
                    draggable={false}
                    onLoad={(e) => {
                      const img = e.currentTarget;
                      const ratio = img.naturalWidth / img.naturalHeight;
                      if (aspects[slot.id] !== ratio) setAspects((m) => ({ ...m, [slot.id]: ratio }));
                    }}
                    style={{
                      width: a * crop.scale,
                      height: crop.scale,
                      left: -crop.x * crop.scale,
                      top: -crop.y * crop.scale,
                    }}
                  />
                </div>
              );
            })}
          </div>
          {short > 0 && (
            <p className="collage-hint muted small">
              {selected === null
                ? "Drag a photo onto another to swap them. Click a photo to move or zoom it in its frame."
                : "Drag to move the photo in its frame, scroll to zoom. Click it again (or Esc) when done."}
            </p>
          )}
        </div>
        <aside className="editor-panel">
          <section>
            <h4>Layout</h4>
            <div className="segmented">
              {TEMPLATES.map((t) => (
                <button key={t.id} className={template === t.id ? "on" : ""} onClick={() => setTemplate(t.id)}>
                  {t.name}
                </button>
              ))}
            </div>
          </section>
          <section>
            <h4>Size</h4>
            <select
              value={sizeId}
              onChange={(e) => {
                setSizeId(e.target.value);
                if (e.target.value === "story") setPortrait(true);
              }}
            >
              {SIZES.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.name}
                </option>
              ))}
            </select>
            {size.w !== size.h && (
              <div className="segmented">
                <button className={portrait ? "" : "on"} onClick={() => setPortrait(false)}>
                  Landscape
                </button>
                <button className={portrait ? "on" : ""} onClick={() => setPortrait(true)}>
                  Portrait
                </button>
              </div>
            )}
          </section>
          <section>
            <label className="edit-slider">
              <span>Spacing</span>
              <span className="edit-value">{Math.round(spacing * 1000) / 10}%</span>
              <input type="range" min={0} max={0.06} step={0.002} value={spacing} onChange={(e) => setSpacing(Number(e.target.value))} />
            </label>
            <label className="edit-slider">
              <span>Corners</span>
              <span className="edit-value">{Math.round(radius * 1000) / 10}%</span>
              <input type="range" min={0} max={0.1} step={0.002} value={radius} onChange={(e) => setRadius(Number(e.target.value))} />
            </label>
            <label className="collage-color">
              <span>Background</span>
              <input type="color" value={background} onChange={(e) => setBackground(e.target.value)} />
            </label>
          </section>
          {selected !== null && (
            <section>
              <h4>Selected photo</h4>
              <label className="edit-slider">
                <span>Zoom</span>
                <span className="edit-value">{slots[selected].zoom.toFixed(1)}×</span>
                <input
                  type="range"
                  min={1}
                  max={4}
                  step={0.05}
                  value={slots[selected].zoom}
                  onChange={(e) => update(selected, { zoom: Number(e.target.value) })}
                />
              </label>
              <button onClick={() => update(selected, { panX: 0.5, panY: 0.5, zoom: 1 })}>Reset</button>
            </section>
          )}
          <p className="muted small">
            Prints are saved at 300 dpi. The file gets the first photo's date and camera, not its location.
          </p>
        </aside>
      </div>
    </div>
  );
}
