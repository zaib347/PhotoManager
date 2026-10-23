import { useLayoutEffect, useMemo, useRef, useState, type PointerEvent } from "react";
import type { Layout } from "../api";
import { formatMonth, yearOf } from "../format";
import { sectionAt, type Grid } from "./grid";

const MIN_LABEL_GAP = 16;

type Props = {
  layout: Layout;
  grid: Grid;
  offset: number;
  maxOffset: number;
  onSeek: (offset: number) => void;
};

/** Date scrubber: maps the full timeline onto the track height, with year labels. */
export function Scrubber({ layout, grid, offset, maxOffset, onSeek }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [height, setHeight] = useState(0);
  const [hoverY, setHoverY] = useState<number | null>(null);
  const dragging = useRef(false);

  useLayoutEffect(() => {
    const el = ref.current!;
    const observer = new ResizeObserver(() => setHeight(el.clientHeight));
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  const toY = (o: number) => (maxOffset > 0 ? Math.min(1, o / maxOffset) * height : 0);
  const toOffset = (y: number) => (height > 0 ? (y / height) * maxOffset : 0);

  const years = useMemo(() => {
    const out: { y: number; label: number }[] = [];
    let lastYear: number | null = null;
    let lastY = -Infinity;
    layout.days.forEach((day, i) => {
      const year = yearOf(day);
      if (year === lastYear) return;
      lastYear = year;
      const y = maxOffset > 0 ? Math.min(1, grid.tops[i] / maxOffset) * height : 0;
      if (y - lastY >= MIN_LABEL_GAP) {
        out.push({ y, label: year });
        lastY = y;
      }
    });
    return out;
  }, [layout, grid, maxOffset, height]);

  const localY = (e: PointerEvent) => {
    const rect = ref.current!.getBoundingClientRect();
    return Math.min(rect.height, Math.max(0, e.clientY - rect.top));
  };

  const hoverLabel =
    hoverY === null || layout.days.length === 0
      ? null
      : formatMonth(layout.days[sectionAt(grid.tops, toOffset(hoverY))]);

  return (
    <div
      ref={ref}
      className="scrubber"
      onPointerDown={(e) => {
        e.currentTarget.setPointerCapture(e.pointerId);
        dragging.current = true;
        onSeek(toOffset(localY(e)));
      }}
      onPointerMove={(e) => {
        const y = localY(e);
        setHoverY(y);
        if (dragging.current) onSeek(toOffset(y));
      }}
      onPointerUp={() => (dragging.current = false)}
      onPointerLeave={() => !dragging.current && setHoverY(null)}
    >
      {years.map(({ y, label }) => (
        <span key={label} className="scrub-year" style={{ top: y }}>
          {label}
        </span>
      ))}
      <div className="scrub-handle" style={{ top: toY(offset) }} />
      {hoverLabel && (
        <div className="scrub-tip" style={{ top: hoverY! }}>
          {hoverLabel}
        </div>
      )}
    </div>
  );
}
