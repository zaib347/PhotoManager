/**
 * Collage layouts. Each template places n photos as rectangles in fractions of the
 * canvas (x, y, w, h from 0 to 1). Templates are drawn for a landscape canvas and
 * turned for a portrait one, so photos stack along the longer side.
 */

export type Rect = { x: number; y: number; w: number; h: number };

export type Template = {
  id: string;
  name: string;
  /** `aspects`: width ÷ height of each photo, in slot order. */
  cells: (n: number, aspects: number[], canvasAspect: number) => Rect[];
};

const transpose = (rects: Rect[]): Rect[] => rects.map((r) => ({ x: r.y, y: r.x, w: r.h, h: r.w }));

/** Lays out for the longer side, turning the result for portrait canvases. */
const oriented =
  (make: (n: number, aspects: number[], aspect: number) => Rect[]) =>
  (n: number, aspects: number[], canvasAspect: number) =>
    canvasAspect >= 1 ? make(n, aspects, canvasAspect) : transpose(make(n, aspects.map((a) => 1 / a), 1 / canvasAspect));

/** Rows of cells; each row's cells share its width equally. */
function rows(counts: number[]): Rect[] {
  const out: Rect[] = [];
  counts.forEach((count, r) => {
    for (let i = 0; i < count; i++) {
      out.push({ x: i / count, y: r / counts.length, w: 1 / count, h: 1 / counts.length });
    }
  });
  return out;
}

/** Splits n into k nearly equal parts, bigger parts last. */
function split(n: number, k: number): number[] {
  return Array.from({ length: k }, (_, i) => Math.floor(n / k) + (i >= k - (n % k) ? 1 : 0));
}

const grid = oriented((n, _aspects, aspect) => {
  // Columns follow the canvas shape: wider canvases get more.
  const cols = Math.max(1, Math.min(n, Math.round(Math.sqrt(n * aspect))));
  const k = Math.ceil(n / cols);
  return rows(split(n, k));
});

const feature = oriented((n) => {
  if (n === 1) return [{ x: 0, y: 0, w: 1, h: 1 }];
  const big: Rect = { x: 0, y: 0, w: 2 / 3, h: 1 };
  const rest = n - 1;
  const cols = rest > 4 ? 2 : 1;
  const per = split(rest, cols);
  const out = [big];
  per.forEach((count, c) => {
    for (let i = 0; i < count; i++) {
      out.push({ x: 2 / 3 + c / (3 * cols), y: i / count, w: 1 / (3 * cols), h: 1 / count });
    }
  });
  return out;
});

/** Justified rows: each photo keeps (nearly) its own shape. */
const mosaic = oriented((n, aspects, aspect) => {
  let best: Rect[] = rows([n]);
  let bestScore = Infinity;
  for (let k = 1; k <= n; k++) {
    const counts = split(n, k);
    // Row heights that fit each row's photos across the width, then scaled to fill.
    let at = 0;
    const heights = counts.map((count) => {
      const sum = aspects.slice(at, at + count).reduce((s, a) => s + a, 0);
      at += count;
      return aspect / Math.max(sum, 0.01);
    });
    const total = heights.reduce((s, h) => s + h, 0);
    // How much the photos must stretch to fill the height.
    const score = Math.abs(Math.log(total));
    if (score >= bestScore) continue;
    bestScore = score;
    const out: Rect[] = [];
    let y = 0;
    at = 0;
    counts.forEach((count, r) => {
      const h = heights[r] / total;
      const row = aspects.slice(at, at + count);
      const sum = row.reduce((s, a) => s + a, 0);
      let x = 0;
      for (const a of row) {
        const w = a / sum;
        out.push({ x, y, w, h });
        x += w;
      }
      y += h;
      at += count;
    });
    best = out;
  }
  return best;
});

const strips = oriented((n) => Array.from({ length: n }, (_, i) => ({ x: i / n, y: 0, w: 1 / n, h: 1 })));

export const TEMPLATES: Template[] = [
  { id: "grid", name: "Grid", cells: grid },
  { id: "mosaic", name: "Mosaic", cells: mosaic },
  { id: "feature", name: "Feature", cells: feature },
  { id: "strips", name: "Strips", cells: strips },
];

export type Size = { id: string; name: string; w: number; h: number; dpi: number };

/** Sizes as landscape; turned for portrait. Prints are 300 dpi. */
export const SIZES: Size[] = [
  { id: "square", name: "Square", w: 2048, h: 2048, dpi: 72 },
  { id: "story", name: "Story (9:16)", w: 1920, h: 1080, dpi: 72 },
  { id: "wide", name: "Wide (16:9)", w: 1920, h: 1080, dpi: 72 },
  { id: "4x6", name: "Print 4×6 in", w: 1800, h: 1200, dpi: 300 },
  { id: "5x7", name: "Print 5×7 in", w: 2100, h: 1500, dpi: 300 },
  { id: "8x10", name: "Print 8×10 in", w: 3000, h: 2400, dpi: 300 },
  { id: "a4", name: "Print A4", w: 3508, h: 2480, dpi: 300 },
];

/** The part of an iw×ih image a cw×ch cell shows, matching the backend's `crop_for`. */
export function cropFor(iw: number, ih: number, cw: number, ch: number, panX: number, panY: number, zoom: number) {
  const scale = Math.max(cw / iw, ch / ih) * Math.min(10, Math.max(1, zoom));
  const sw = cw / scale;
  const sh = ch / scale;
  return { x: (iw - sw) * panX, y: (ih - sh) * panY, w: sw, h: sh, scale };
}

/** A cell's rectangle in pixels, matching the backend's `cell_rect`. */
export function cellRect(r: Rect, w: number, h: number, gap: number) {
  const cw = w - gap;
  const ch = h - gap;
  const x0 = gap / 2 + r.x * cw + gap / 2;
  const y0 = gap / 2 + r.y * ch + gap / 2;
  const x1 = gap / 2 + (r.x + r.w) * cw - gap / 2;
  const y1 = gap / 2 + (r.y + r.h) * ch - gap / 2;
  return { x: x0, y: y0, w: Math.max(1, x1 - x0), h: Math.max(1, y1 - y0) };
}
