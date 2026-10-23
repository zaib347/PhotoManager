import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type MouseEvent,
  type ReactNode,
} from "react";
import { MediaKind, thumbUrl } from "../api";
import { formatDay, formatDuration } from "../format";
import type { DayStore, Position } from "./dayStore";
import { buildGrid, GAP, HEADER_H, sectionAt } from "./grid";
import { Scrubber } from "./Scrubber";
import { useFps } from "./useFps";

const OVERSCAN = 600;

type Props = {
  store: DayStore;
  selection: ReadonlySet<number>;
  /** False while an overlay (the viewer) owns the keyboard. */
  keyboard: boolean;
  onActivate: (e: MouseEvent, pos: Position, id: number) => void;
  onToggle: (pos: Position, id: number) => void;
  onToggleDay: (day: number) => void;
  /** Scroll so this position is visible (e.g. after closing the viewer). */
  reveal?: Position | null;
};

/**
 * Virtualized photo timeline. It owns its scroll offset instead of using native
 * scrolling, because a million-photo grid is taller than browsers allow an element to be.
 */
export function Timeline({ store, selection, keyboard, onActivate, onToggle, onToggleDay, reveal }: Props) {
  useSyncExternalStore(store.subscribe, store.getVersion);
  const layout = store.layout;
  const viewportRef = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });
  const [offset, setOffset] = useState(0);
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
  const stride = grid.cell + GAP;

  useEffect(() => setOffset((o) => clamp(o)), [clamp]);

  useEffect(() => {
    if (!reveal) return;
    const i = store.indexOf(reveal.day);
    if (i < 0) return;
    const y = grid.tops[i] + HEADER_H + Math.floor(reveal.index / grid.cols) * stride;
    setOffset((o) => (y < o || y + stride > o + size.height ? clamp(y - size.height / 3) : o));
  }, [reveal, store, grid, stride, size.height, clamp]);

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
    if (!keyboard) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLInputElement) return;
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
  }, [keyboard, clamp, size.height, maxOffset]);

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
    const days = visible.map((i) => layout.days[i]);
    store.ensure(days).catch((e) => console.error("dayAssets failed", e));
    // `visibleKey` captures `visible`; listing the array itself would refetch every render.
  }, [visibleKey, layout, store]);

  const selecting = selection.size > 0;
  const nodes: ReactNode[] = [];
  for (const i of visible) {
    const day = layout.days[i];
    const count = layout.counts[i];
    const sectionTop = grid.tops[i];
    const data = store.get(day);
    const daySelected = !!data && data.ids.length > 0 && data.ids.every((id) => selection.has(id));
    nodes.push(
      <div
        key={`h${day}`}
        className={`day-header${daySelected ? " selected" : ""}`}
        style={{ transform: `translate3d(0, ${sectionTop - offset}px, 0)` }}
      >
        <button
          className="check day-check"
          aria-label={daySelected ? "Deselect day" : "Select day"}
          onClick={() => onToggleDay(day)}
        />
        <span>{formatDay(day)}</span>
        <span className="muted">{count.toLocaleString()}</span>
      </div>,
    );
    const rows = Math.ceil(count / grid.cols);
    const firstRow = Math.max(0, Math.floor((top - sectionTop - HEADER_H) / stride));
    const lastRow = Math.min(rows, Math.ceil((bottom - sectionTop - HEADER_H) / stride));
    for (let r = firstRow; r < lastRow; r++) {
      for (let c = 0; c < grid.cols; c++) {
        const index = r * grid.cols + c;
        if (index >= count) break;
        const id = data?.ids[index];
        const y = sectionTop + HEADER_H + r * stride - offset;
        const style = { width: grid.cell, height: grid.cell, transform: `translate3d(${c * stride}px, ${y}px, 0)` };
        if (id === undefined) {
          nodes.push(<div key={`p${day}-${index}`} className="cell" style={style} />);
          continue;
        }
        const selected = selection.has(id);
        const duration = data?.durations[index];
        nodes.push(
          <div
            key={id}
            className={`cell${selected ? " selected" : ""}`}
            style={style}
            onClick={(e) => onActivate(e, { day, index }, id)}
          >
            <img src={thumbUrl(id)} alt="" draggable={false} decoding="async" />
            <button
              className="check"
              aria-label={selected ? "Deselect" : "Select"}
              onClick={(e) => {
                e.stopPropagation();
                onToggle({ day, index }, id);
              }}
            />
            {data?.kinds[index] === MediaKind.Video && (
              <span className="badge">▶ {duration ? formatDuration(duration) : ""}</span>
            )}
            {data?.favorites[index] === 1 && <span className="fav-badge">♥</span>}
          </div>,
        );
      }
    }
  }

  return (
    <div className="timeline">
      <div ref={viewportRef} className={`timeline-viewport${selecting ? " selecting" : ""}`}>
        {nodes}
      </div>
      <Scrubber layout={layout} grid={grid} offset={offset} maxOffset={maxOffset} onSeek={(y) => setOffset(clamp(y))} />
      <div className="perf">
        {fps} fps · {nodes.length} nodes
        {store.lastFetchMs !== null && ` · last fetch ${store.lastFetchMs.toFixed(1)} ms`}
      </div>
    </div>
  );
}
