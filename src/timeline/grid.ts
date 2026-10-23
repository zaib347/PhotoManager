import type { Layout } from "../api";

export const HEADER_H = 44;
export const GAP = 4;
const TARGET_CELL = 180;

/** Pixel geometry of the whole timeline, derived only from day counts. */
export type Grid = {
  cols: number;
  cell: number;
  /** tops[i] is the y of section i; tops[n] is the total height. */
  tops: Float64Array;
  total: number;
};

export function buildGrid(layout: Layout, width: number): Grid {
  const cols = Math.max(1, Math.floor((width + GAP) / (TARGET_CELL + GAP)));
  const cell = (width - GAP * (cols - 1)) / cols;
  const n = layout.days.length;
  const tops = new Float64Array(n + 1);
  let y = 0;
  for (let i = 0; i < n; i++) {
    tops[i] = y;
    y += HEADER_H + Math.ceil(layout.counts[i] / cols) * (cell + GAP);
  }
  tops[n] = y;
  return { cols, cell, tops, total: y };
}

/** Index of the section containing y (binary search over section tops). */
export function sectionAt(tops: Float64Array, y: number): number {
  let lo = 0;
  let hi = tops.length - 2;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (tops[mid] <= y) lo = mid;
    else hi = mid - 1;
  }
  return Math.max(0, lo);
}
