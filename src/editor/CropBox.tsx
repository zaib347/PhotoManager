import { useRef, type PointerEvent } from "react";
import type { Recipe } from "../api";

export type Crop = NonNullable<Recipe["crop"]>;

const FULL: Crop = { x: 0, y: 0, w: 1, h: 1 };
const MIN = 0.05;

type Handle = "move" | "nw" | "ne" | "sw" | "se";

type Props = {
  crop: Crop | null;
  /** Width ÷ height the crop must keep, in pixels; null for any shape. */
  aspect: number | null;
  /** Width ÷ height of the photo being cropped. */
  imageAspect: number;
  onChange: (crop: Crop) => void;
};

/** The largest centred crop of `aspect` (pixels) in a photo of `imageAspect`. */
export function fitAspect(aspect: number, imageAspect: number): Crop {
  // In fractions of the photo, a crop of w×h has the pixel shape (w / h) × imageAspect.
  const ratio = aspect / imageAspect;
  const [w, h] = ratio >= 1 ? [1, 1 / ratio] : [ratio, 1];
  return { x: (1 - w) / 2, y: (1 - h) / 2, w, h };
}

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

/** A draggable crop rectangle over the photo, with the outside dimmed. */
export function CropBox({ crop, aspect, imageAspect, onChange }: Props) {
  const box = useRef<HTMLDivElement>(null);
  const drag = useRef<{ handle: Handle; x: number; y: number; start: Crop } | null>(null);
  const c = crop ?? FULL;

  const begin = (handle: Handle) => (e: PointerEvent) => {
    e.preventDefault();
    e.stopPropagation();
    (e.target as Element).setPointerCapture(e.pointerId);
    drag.current = { handle, x: e.clientX, y: e.clientY, start: c };
  };

  const move = (e: PointerEvent) => {
    const d = drag.current;
    const rect = box.current?.getBoundingClientRect();
    if (!d || !rect) return;
    const dx = (e.clientX - d.x) / rect.width;
    const dy = (e.clientY - d.y) / rect.height;
    const s = d.start;
    if (d.handle === "move") {
      onChange({ ...s, x: clamp(s.x + dx, 0, 1 - s.w), y: clamp(s.y + dy, 0, 1 - s.h) });
      return;
    }
    // The corner opposite the one dragged stays put.
    const west = d.handle === "nw" || d.handle === "sw";
    const north = d.handle === "nw" || d.handle === "ne";
    const right = s.x + s.w;
    const bottom = s.y + s.h;
    let w = clamp(west ? s.w - dx : s.w + dx, MIN, west ? right : 1 - s.x);
    let h = clamp(north ? s.h - dy : s.h + dy, MIN, north ? bottom : 1 - s.y);
    if (aspect !== null) {
      const ratio = aspect / imageAspect;
      // Follow whichever edge moved more, then shrink both to fit.
      if (Math.abs(dx) >= Math.abs(dy)) h = w / ratio;
      else w = h * ratio;
      const maxW = west ? right : 1 - s.x;
      const maxH = north ? bottom : 1 - s.y;
      const k = Math.min(1, maxW / w, maxH / h);
      w *= k;
      h *= k;
    }
    onChange({ x: west ? right - w : s.x, y: north ? bottom - h : s.y, w, h });
  };

  const end = () => {
    drag.current = null;
  };

  const pct = (v: number) => `${v * 100}%`;
  return (
    <div ref={box} className="crop-layer" onPointerMove={move} onPointerUp={end} onPointerCancel={end}>
      <div
        className="crop-rect"
        style={{ left: pct(c.x), top: pct(c.y), width: pct(c.w), height: pct(c.h) }}
        onPointerDown={begin("move")}
      >
        <div className="crop-thirds" />
        {(["nw", "ne", "sw", "se"] as const).map((h) => (
          <div key={h} className={`crop-handle ${h}`} onPointerDown={begin(h)} />
        ))}
      </div>
    </div>
  );
}
