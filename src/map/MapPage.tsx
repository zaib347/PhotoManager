import { useCallback, useEffect, useRef, useState, type PointerEvent, type WheelEvent } from "react";
import { api, thumbUrl, type Area, type CityInfo, type MapCluster, type MapSummary } from "../api";
import { useCollections } from "../collections/context";
import {
  fromScreen,
  projectX,
  projectY,
  toScreen,
  unprojectLat,
  unprojectLon,
  WorldMap,
  type Colors,
  type View,
} from "./world";

const MIN_ZOOM = 1;
const MAX_ZOOM = 13;

/** Loaded once per session; it doesn't change. */
let worldPromise: Promise<WorldMap> | null = null;
function loadWorld() {
  worldPromise ??= Promise.all([api.mapData(), api.mapCities()]).then(([data, cities]) => {
    const world = new WorldMap(data);
    world.setCities(cities);
    return world;
  });
  worldPromise.catch(() => (worldPromise = null));
  return worldPromise;
}

function colors(el: Element): Colors {
  const s = getComputedStyle(el);
  const v = (name: string) => s.getPropertyValue(name).trim();
  return {
    water: v("--map-water"),
    land: v("--map-land"),
    urban: v("--map-urban"),
    border: v("--map-border"),
    state: v("--map-state"),
    road: v("--map-road"),
    text: v("--map-text"),
    halo: v("--map-halo"),
  };
}

const clampView = (v: View): View => ({
  ...v,
  zoom: Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, v.zoom)),
  cx: Math.max(0, Math.min(1, v.cx)),
  cy: Math.max(0, Math.min(1, v.cy)),
});

/** A view framing `area` (or the whole world). */
function framing(area: Area | null, width: number, height: number): View {
  if (!area) return { cx: 0.5, cy: 0.4, zoom: 1.6, width, height };
  const x0 = projectX(area.west);
  const x1 = projectX(area.east);
  const y0 = projectY(area.north);
  const y1 = projectY(area.south);
  const span = Math.max((x1 - x0) * 256, 1e-6);
  const spanY = Math.max((y1 - y0) * 256, 1e-6);
  const zoom = Math.log2(Math.min(width / span, height / spanY)) - 0.4;
  return clampView({ cx: (x0 + x1) / 2, cy: (y0 + y1) / 2, zoom: Math.min(zoom, 11), width, height });
}

type Props = {
  /** Photos waiting to be placed: clicking the map (or picking a town) places them. */
  placing: number[] | null;
  onPlace: (lat: number, lon: number) => void;
  onCancelPlacing: () => void;
  onError: (e: unknown) => void;
};

/** Photos on an offline world map, grouped into clusters that open the timeline. */
export function MapPage({ placing, onPlace, onCancelPlacing, onError }: Props) {
  const { showPlace } = useCollections();
  const wrap = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const [world, setWorld] = useState<WorldMap | null>(null);
  const [summary, setSummary] = useState<MapSummary | null>(null);
  const [view, setView] = useState<View | null>(null);
  const [clusters, setClusters] = useState<MapCluster[]>([]);
  const drag = useRef<{ x: number; y: number; cx: number; cy: number; moved: boolean } | null>(null);

  useEffect(() => {
    Promise.all([loadWorld(), api.mapLoad()])
      .then(([w, s]) => {
        setWorld(w);
        setSummary(s);
      })
      .catch(onError);
  }, [onError]);

  // Size follows the container; the first size frames the photos.
  useEffect(() => {
    const el = wrap.current;
    if (!el || !summary) return;
    const observer = new ResizeObserver(([entry]) => {
      const { width, height } = entry.contentRect;
      setView((v) => (v ? { ...v, width, height } : framing(summary.bounds, width, height)));
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, [summary]);

  // Draw on the next frame after any change.
  useEffect(() => {
    const c = canvas.current;
    if (!c || !world || !view) return;
    const frame = requestAnimationFrame(() => {
      const dpr = window.devicePixelRatio || 1;
      c.width = Math.round(view.width * dpr);
      c.height = Math.round(view.height * dpr);
      const ctx = c.getContext("2d");
      if (!ctx) return;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      world.draw(ctx, view, colors(c));
    });
    return () => cancelAnimationFrame(frame);
  }, [world, view]);

  // Clusters for the view, once it stops moving.
  useEffect(() => {
    if (!view || !summary?.count) return;
    let stale = false;
    const timer = window.setTimeout(() => {
      const tl = fromScreen(view, -100, -100);
      const br = fromScreen(view, view.width + 100, view.height + 100);
      api
        .mapClusters(view.zoom, tl.x, tl.y, br.x, br.y)
        .then((list) => !stale && setClusters(list))
        .catch(onError);
    }, 120);
    return () => {
      stale = true;
      window.clearTimeout(timer);
    };
  }, [view, summary, onError]);

  const zoomAt = useCallback((sx: number, sy: number, delta: number) => {
    setView((v) => {
      if (!v) return v;
      const zoom = Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, v.zoom + delta));
      // Keep the point under the cursor where it is.
      const before = fromScreen(v, sx, sy);
      const scaled = { ...v, zoom };
      const after = fromScreen(scaled, sx, sy);
      return clampView({ ...scaled, cx: v.cx + before.x - after.x, cy: v.cy + before.y - after.y });
    });
  }, []);

  const onWheel = (e: WheelEvent) => {
    const r = e.currentTarget.getBoundingClientRect();
    zoomAt(e.clientX - r.left, e.clientY - r.top, -e.deltaY * 0.002);
  };

  const onPointerDown = (e: PointerEvent) => {
    if (!view || e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    drag.current = { x: e.clientX, y: e.clientY, cx: view.cx, cy: view.cy, moved: false };
  };

  const onPointerMove = (e: PointerEvent) => {
    const d = drag.current;
    if (!d) return;
    const dx = e.clientX - d.x;
    const dy = e.clientY - d.y;
    if (Math.abs(dx) + Math.abs(dy) > 4) d.moved = true;
    if (!d.moved) return;
    setView((v) => {
      if (!v) return v;
      const k = 256 * 2 ** v.zoom;
      return clampView({ ...v, cx: d.cx - dx / k, cy: d.cy - dy / k });
    });
  };

  const onPointerUp = (e: PointerEvent) => {
    const d = drag.current;
    drag.current = null;
    if (!d || d.moved || !view || !placing) return;
    const r = e.currentTarget.getBoundingClientRect();
    const p = fromScreen(view, e.clientX - r.left, e.clientY - r.top);
    onPlace(unprojectLat(p.y), unprojectLon(p.x));
  };

  const goTo = (city: CityInfo) => {
    if (placing) onPlace(city.lat, city.lon);
    else setView((v) => v && clampView({ ...v, cx: projectX(city.lon), cy: projectY(city.lat), zoom: 10 }));
  };

  return (
    <div className="map-page" ref={wrap}>
      <canvas
        ref={canvas}
        className={`map-canvas${placing ? " placing" : ""}`}
        onWheel={onWheel}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onDoubleClick={(e) => {
          const r = e.currentTarget.getBoundingClientRect();
          zoomAt(e.clientX - r.left, e.clientY - r.top, e.shiftKey ? -1 : 1);
        }}
      />
      {view &&
        !placing &&
        clusters.map((c) => {
          const { sx, sy } = toScreen(view, projectX(c.lon), projectY(c.lat));
          if (sx < -60 || sy < -60 || sx > view.width + 60 || sy > view.height + 60) return null;
          return (
            <button
              key={`${c.area.west},${c.area.north}`}
              className="map-cluster"
              style={{ left: sx, top: sy }}
              onClick={() => showPlace({ area: c.area })}
              title={`${c.count.toLocaleString()} ${c.count === 1 ? "item" : "items"} here`}
            >
              <img src={thumbUrl(c.coverId)} alt="" loading="lazy" draggable={false} />
              {c.count > 1 && <span className="map-count">{c.count.toLocaleString()}</span>}
            </button>
          );
        })}

      <div className="map-tools">
        <CitySearch onPick={goTo} placeholder={placing ? "Place them in a town…" : "Find a town"} />
        <div className="map-zoom">
          <button onClick={() => view && zoomAt(view.width / 2, view.height / 2, 1)} aria-label="Zoom in">
            +
          </button>
          <button onClick={() => view && zoomAt(view.width / 2, view.height / 2, -1)} aria-label="Zoom out">
            −
          </button>
          {summary?.bounds && (
            <button
              onClick={() => view && setView(framing(summary.bounds, view.width, view.height))}
              title="Show all photos"
            >
              ⤢
            </button>
          )}
        </div>
      </div>

      {placing && (
        <div className="map-banner">
          Click where {placing.length === 1 ? "this photo was" : `these ${placing.length.toLocaleString()} photos were`}{" "}
          taken, or pick a town.
          <button onClick={onCancelPlacing}>Cancel</button>
        </div>
      )}
      {summary && summary.count === 0 && !placing && (
        <div className="map-banner">
          No photos with a location yet. Phone photos usually have one; others can be placed by hand (select them,
          then Location).
        </div>
      )}
      <div className="map-credit muted">Natural Earth · GeoNames</div>
    </div>
  );
}

type SearchProps = { onPick: (city: CityInfo) => void; placeholder: string };

/** Town search over the bundled list. */
export function CitySearch({ onPick, placeholder }: SearchProps) {
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<CityInfo[]>([]);

  useEffect(() => {
    if (!query.trim()) {
      setHits([]);
      return;
    }
    let stale = false;
    api.searchCities(query).then((list) => !stale && setHits(list));
    return () => {
      stale = true;
    };
  }, [query]);

  const pick = (city: CityInfo) => {
    setQuery("");
    onPick(city);
  };

  return (
    <div className="city-search">
      <input
        type="search"
        placeholder={placeholder}
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && hits[0] && pick(hits[0])}
      />
      {hits.length > 0 && (
        <ul className="city-hits">
          {hits.map((c) => (
            <li key={c.id}>
              <button onClick={() => pick(c)}>
                {c.name}
                <span className="muted small"> {[c.region, c.country].filter(Boolean).join(", ")}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
