import { useEffect, useRef, useState, type PointerEvent } from "react";

const MAX_SCALE = 8;
const DOUBLE_CLICK_SCALE = 2.5;

type Transform = { scale: number; x: number; y: number };
const IDENTITY: Transform = { scale: 1, x: 0, y: 0 };

type Props = {
  /** Low-resolution image shown instantly while the full one loads. */
  placeholder: string;
  src: string;
  onError: () => void;
};

/** Fit-to-screen image with wheel zoom at the cursor, drag to pan, double-click to toggle. */
export function ZoomableImage({ placeholder, src, onError }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [t, setT] = useState(IDENTITY);
  const [loaded, setLoaded] = useState(false);
  const drag = useRef<{ x: number; y: number; t: Transform } | null>(null);

  /** Cursor position relative to the container's centre (the transform origin). */
  const local = (clientX: number, clientY: number) => {
    const r = ref.current!.getBoundingClientRect();
    return { x: clientX - r.left - r.width / 2, y: clientY - r.top - r.height / 2 };
  };

  /** Zooms so the image point under `p` stays under `p`. */
  const zoomAt = (p: { x: number; y: number }, scale: number) =>
    setT((cur) => {
      const s = Math.min(MAX_SCALE, Math.max(1, scale));
      if (s === 1) return IDENTITY;
      const k = s / cur.scale;
      return { scale: s, x: p.x - k * (p.x - cur.x), y: p.y - k * (p.y - cur.y) };
    });

  useEffect(() => {
    const el = ref.current!;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const p = local(e.clientX, e.clientY);
      setT((cur) => {
        const s = Math.min(MAX_SCALE, Math.max(1, cur.scale * Math.exp(-e.deltaY * 0.0015)));
        if (s === 1) return IDENTITY;
        const k = s / cur.scale;
        return { scale: s, x: p.x - k * (p.x - cur.x), y: p.y - k * (p.y - cur.y) };
      });
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  const onPointerDown = (e: PointerEvent) => {
    if (t.scale === 1) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    drag.current = { x: e.clientX, y: e.clientY, t };
  };
  const onPointerMove = (e: PointerEvent) => {
    const d = drag.current;
    if (!d) return;
    setT({ ...d.t, x: d.t.x + e.clientX - d.x, y: d.t.y + e.clientY - d.y });
  };

  const style = { transform: `translate(${t.x}px, ${t.y}px) scale(${t.scale})` };
  return (
    <div
      ref={ref}
      className={`zoomable${t.scale > 1 ? " zoomed" : ""}`}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={() => (drag.current = null)}
      onDoubleClick={(e) => zoomAt(local(e.clientX, e.clientY), t.scale > 1 ? 1 : DOUBLE_CLICK_SCALE)}
    >
      <img className="viewer-img placeholder" src={placeholder} alt="" draggable={false} style={style} />
      <img
        className={`viewer-img${loaded ? " loaded" : ""}`}
        src={src}
        alt=""
        draggable={false}
        style={style}
        onLoad={() => setLoaded(true)}
        onError={onError}
      />
    </div>
  );
}
