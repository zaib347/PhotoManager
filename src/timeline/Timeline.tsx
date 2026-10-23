import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { api, MediaKind, thumbUrl, type DayAssets, type Layout } from "../api";
import { buildGrid, formatDay, GAP, HEADER_H, sectionAt } from "./grid";
import { Scrubber } from "./Scrubber";
import { useFps } from "./useFps";

const OVERSCAN = 600;
const MAX_CACHED_DAYS = 4000;

/** Asset ids for a day, tagged with the layout generation they were fetched for. */
type CachedDay = { gen: number; data: DayAssets };

/**
 * Virtualized photo timeline. It owns its scroll offset instead of using native
 * scrolling, because a million-photo grid is taller than browsers allow an element to be.
 */
export function Timeline({ layout }: { layout: Layout }) {
  const viewportRef = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });
  const [offset, setOffset] = useState(0);
  const [, setCacheVersion] = useState(0);
  const cache = useRef(new Map<number, CachedDay>());
  const pending = useRef(new Set<number>());
  const generation = useRef(0);
  const [lastFetchMs, setLastFetchMs] = useState<number | null>(null);
  const fps = useFps();

  useLayoutEffect(() => {
    const el = viewportRef.current!;
    const observer = new ResizeObserver(() => setSize({ width: el.clientWidth, height: el.clientHeight }));
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  const grid = useMemo(() => buildGrid(layout, size.width), [layout, size.width]);
  const maxOffset = Math.max(0, grid.total - size.height);
  const clamp = useCallback((y: number) => Math.min(maxOffset, Math.max(0, y)), [maxOffset]);

  useEffect(() => setOffset((o) => clamp(o)), [clamp]);

  // A new layout (e.g. a scan added photos) marks cached days stale but keeps showing
  // them, and keeps the scroll position, so live updates don't flicker or jump.
  useEffect(() => {
    generation.current++;
    pending.current.clear();
  }, [layout]);

  useEffect(() => {
    const el = viewportRef.current!;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const unit = e.deltaMode === 1 ? 40 : e.deltaMode === 2 ? el.clientHeight : 1;
      setOffset((o) => clamp(o + e.deltaY * unit));
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [clamp]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const page = size.height * 0.9;
      const deltas: Record<string, number> = {
        PageDown: page,
        PageUp: -page,
        ArrowDown: 80,
        ArrowUp: -80,
        " ": e.shiftKey ? -page : page,
      };
      if (e.key === "Home") setOffset(0);
      else if (e.key === "End") setOffset(maxOffset);
      else if (e.key in deltas) setOffset((o) => clamp(o + deltas[e.key]));
      else return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [clamp, size.height, maxOffset]);

  const top = offset - OVERSCAN;
  const bottom = offset + size.height + OVERSCAN;
  const visible: number[] = [];
  if (size.width > 0) {
    for (let i = sectionAt(grid.tops, Math.max(0, top)); i < layout.days.length && grid.tops[i] < bottom; i++) {
      visible.push(i);
    }
  }
  const visibleKey = visible.join(",");

  useEffect(() => {
    const missing = visible
      .map((i) => layout.days[i])
      .filter((d) => cache.current.get(d)?.gen !== generation.current && !pending.current.has(d));
    if (missing.length === 0) return;
    missing.forEach((d) => pending.current.add(d));
    const gen = generation.current;
    const started = performance.now();
    api
      .dayAssets(missing)
      .then((result) => {
        if (gen !== generation.current) return;
        result.forEach((d) => cache.current.set(d.day, { gen, data: d }));
        evict(cache.current, new Set(visible.map((i) => layout.days[i])));
        setLastFetchMs(performance.now() - started);
        setCacheVersion((v) => v + 1);
      })
      .catch((e) => console.error("dayAssets failed", e))
      .finally(() => missing.forEach((d) => pending.current.delete(d)));
    // `visibleKey` captures `visible`; listing the array itself would refetch every render.
  }, [visibleKey, layout]);

  const nodes: ReactNode[] = [];
  const stride = grid.cell + GAP;
  for (const i of visible) {
    const day = layout.days[i];
    const count = layout.counts[i];
    const sectionTop = grid.tops[i];
    nodes.push(
      <div key={`h${day}`} className="day-header" style={{ transform: `translate3d(0, ${sectionTop - offset}px, 0)` }}>
        <span>{formatDay(day)}</span>
        <span className="muted">{count.toLocaleString()}</span>
      </div>,
    );
    const data = cache.current.get(day)?.data;
    const rows = Math.ceil(count / grid.cols);
    const firstRow = Math.max(0, Math.floor((top - sectionTop - HEADER_H) / stride));
    const lastRow = Math.min(rows, Math.ceil((bottom - sectionTop - HEADER_H) / stride));
    for (let r = firstRow; r < lastRow; r++) {
      for (let c = 0; c < grid.cols; c++) {
        const idx = r * grid.cols + c;
        if (idx >= count) break;
        const id = data?.ids[idx];
        const y = sectionTop + HEADER_H + r * stride - offset;
        nodes.push(
          <div
            key={id ?? `p${day}-${idx}`}
            className="cell"
            style={{ width: grid.cell, height: grid.cell, transform: `translate3d(${c * stride}px, ${y}px, 0)` }}
          >
            {id !== undefined && <img src={thumbUrl(id)} alt="" draggable={false} decoding="async" />}
            {data?.kinds[idx] === MediaKind.Video && <span className="badge">▶</span>}
          </div>,
        );
      }
    }
  }

  return (
    <div className="timeline">
      <div ref={viewportRef} className="timeline-viewport" tabIndex={0}>
        {nodes}
      </div>
      <Scrubber layout={layout} grid={grid} offset={offset} maxOffset={maxOffset} onSeek={(y) => setOffset(clamp(y))} />
      <div className="perf">
        {fps} fps · {nodes.length} nodes · {cache.current.size} days cached
        {lastFetchMs !== null && ` · last fetch ${lastFetchMs.toFixed(1)} ms`}
      </div>
    </div>
  );
}

function evict(cache: Map<number, CachedDay>, keep: Set<number>) {
  if (cache.size <= MAX_CACHED_DAYS) return;
  // Map iterates in insertion order, so the oldest fetches go first.
  for (const day of cache.keys()) {
    if (cache.size <= MAX_CACHED_DAYS * 0.75) break;
    if (!keep.has(day)) cache.delete(day);
  }
}
