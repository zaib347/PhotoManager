/**
 * The bundled offline world map (Natural Earth, packed by `examples/build_map.rs`):
 * parsing, lazy tile decoding into cached `Path2D`s, and drawing at any zoom.
 *
 * Coordinates: Web Mercator x and y from 0 to 1 (y grows southwards). Paths are kept
 * in "world units" (256 × Mercator, the size of the world at zoom 0) and moved onto
 * the screen with `addPath` and a matrix, so line widths stay in screen pixels.
 */

const SCALE = 1e4;
export const MAX_LAT = 85.05112878;
const WORLD = 256;

export type View = { cx: number; cy: number; zoom: number; width: number; height: number };

type Tile = { bytes: Uint8Array; path?: Path2D };
type Layer = { area: boolean; deg: number; tiles: Map<string, Tile> };
export type Label = { name: string; x: number; y: number; minZoom: number };
export type City = { name: string; x: number; y: number; population: number };

export const projectX = (lon: number) => (lon + 180) / 360;
export function projectY(lat: number) {
  const r = (Math.max(-MAX_LAT, Math.min(MAX_LAT, lat)) * Math.PI) / 180;
  return (1 - Math.log(Math.tan(r) + 1 / Math.cos(r)) / Math.PI) / 2;
}
export const unprojectLon = (x: number) => x * 360 - 180;
export const unprojectLat = (y: number) => (Math.atan(Math.sinh(Math.PI * (1 - 2 * y))) * 180) / Math.PI;

/** Screen position of Mercator (x, y). */
export function toScreen(v: View, x: number, y: number) {
  const k = WORLD * 2 ** v.zoom;
  return { sx: (x - v.cx) * k + v.width / 2, sy: (y - v.cy) * k + v.height / 2 };
}

/** Mercator position under a screen point. */
export function fromScreen(v: View, sx: number, sy: number) {
  const k = WORLD * 2 ** v.zoom;
  return { x: v.cx + (sx - v.width / 2) / k, y: v.cy + (sy - v.height / 2) / k };
}

/** Which layer (by detail) draws at each zoom; null hides it. */
function layerFor(kind: string, z: number): string | null {
  switch (kind) {
    case "land":
      return z < 2 ? "land0" : z < 4.5 ? "land1" : "land2";
    case "lakes":
      return z < 2 ? null : z < 4.5 ? "lakes1" : "lakes2";
    case "urban":
      return z < 6 ? null : "urban2";
    case "border":
      return z < 2 ? "border0" : z < 4.5 ? "border1" : "border2";
    case "state":
      return z < 3.5 ? null : z < 5 ? "state1" : "state2";
    case "river":
      return z < 3 ? null : z < 5 ? "river1" : "river2";
    case "road":
      return z < 5.5 ? null : "road2";
  }
  return null;
}

/** Towns this big or bigger get a label at zoom `z`. */
function minPopulation(z: number) {
  if (z < 3) return 5_000_000;
  if (z < 4) return 2_000_000;
  if (z < 5) return 700_000;
  if (z < 6) return 250_000;
  if (z < 7) return 100_000;
  if (z < 8) return 40_000;
  return 0;
}

export type Colors = {
  water: string;
  land: string;
  urban: string;
  border: string;
  state: string;
  road: string;
  text: string;
  halo: string;
};

export class WorldMap {
  private layers = new Map<string, Layer>();
  labels: Label[] = [];
  cities: City[] = [];

  constructor(buffer: ArrayBuffer) {
    const bytes = new Uint8Array(buffer);
    const view = new DataView(buffer);
    const text = new TextDecoder();
    if (text.decode(bytes.subarray(0, 4)) !== "PMMP") throw new Error("Not a map file");
    let at = 8;
    const layers = view.getUint32(at, true);
    at += 4;
    for (let i = 0; i < layers; i++) {
      const nameLen = bytes[at];
      const name = text.decode(bytes.subarray(at + 1, at + 1 + nameLen));
      at += 1 + nameLen;
      const area = bytes[at] === 0;
      const deg = view.getUint16(at + 1, true);
      const count = view.getUint32(at + 3, true);
      at += 7;
      const tiles = new Map<string, Tile>();
      for (let t = 0; t < count; t++) {
        const x = view.getUint16(at, true);
        const y = view.getUint16(at + 2, true);
        const len = view.getUint32(at + 4, true);
        tiles.set(`${x},${y}`, { bytes: bytes.subarray(at + 8, at + 8 + len) });
        at += 8 + len;
      }
      this.layers.set(name, { area, deg, tiles });
    }
    const labels = view.getUint32(at, true);
    at += 4;
    for (let i = 0; i < labels; i++) {
      const len = view.getUint16(at, true);
      const name = text.decode(bytes.subarray(at + 2, at + 2 + len));
      at += 2 + len;
      const lon = view.getInt32(at, true) / SCALE;
      const lat = view.getInt32(at + 4, true) / SCALE;
      this.labels.push({ name, x: projectX(lon), y: projectY(lat), minZoom: bytes[at + 8] });
      at += 9;
    }
  }

  setCities(list: [string, number, number, number][]) {
    this.cities = list.map(([name, lat, lon, population]) => ({ name, x: projectX(lon), y: projectY(lat), population }));
  }

  /** A tile's features as one path in world units (decoded once). */
  private path(tile: Tile, area: boolean): Path2D {
    if (tile.path) return tile.path;
    const b = tile.bytes;
    let i = 0;
    const read = () => {
      let result = 0;
      let mul = 1;
      for (;;) {
        const byte = b[i++];
        result += (byte & 0x7f) * mul;
        if (byte < 0x80) return result;
        mul *= 128;
      }
    };
    const signed = () => {
      const v = read();
      return v % 2 === 0 ? v / 2 : -(v + 1) / 2;
    };
    const path = new Path2D();
    const features = read();
    for (let f = 0; f < features; f++) {
      const rings = read();
      for (let r = 0; r < rings; r++) {
        const n = read();
        let lon = 0;
        let lat = 0;
        for (let p = 0; p < n; p++) {
          lon += signed();
          lat += signed();
          const x = projectX(lon / SCALE) * WORLD;
          const y = projectY(lat / SCALE) * WORLD;
          if (p === 0) path.moveTo(x, y);
          else path.lineTo(x, y);
        }
        if (area) path.closePath();
      }
    }
    tile.path = path;
    return path;
  }

  /** Every visible tile of a layer combined, placed on screen. */
  private visible(name: string, v: View, matrix: DOMMatrix): Path2D | null {
    const layer = this.layers.get(name);
    if (!layer) return null;
    const tl = fromScreen(v, 0, 0);
    const br = fromScreen(v, v.width, v.height);
    const lon0 = unprojectLon(Math.max(0, tl.x));
    const lon1 = unprojectLon(Math.min(1, br.x));
    const lat1 = unprojectLat(Math.max(0, tl.y));
    const lat0 = unprojectLat(Math.min(1, br.y));
    const x0 = Math.floor((lon0 + 180) / layer.deg);
    const x1 = Math.floor((lon1 + 180) / layer.deg);
    const y0 = Math.floor((lat0 + 90) / layer.deg);
    const y1 = Math.floor((lat1 + 90) / layer.deg);
    const out = new Path2D();
    let any = false;
    for (let x = x0; x <= x1; x++) {
      for (let y = y0; y <= y1; y++) {
        const tile = layer.tiles.get(`${x},${y}`);
        if (!tile) continue;
        out.addPath(this.path(tile, layer.area), matrix);
        any = true;
      }
    }
    return any ? out : null;
  }

  draw(ctx: CanvasRenderingContext2D, v: View, c: Colors) {
    const k = 2 ** v.zoom;
    const matrix = new DOMMatrix([k, 0, 0, k, v.width / 2 - v.cx * WORLD * k, v.height / 2 - v.cy * WORLD * k]);
    ctx.fillStyle = c.water;
    ctx.fillRect(0, 0, v.width, v.height);
    const fill = (kind: string, color: string) => {
      const name = layerFor(kind, v.zoom);
      const path = name && this.visible(name, v, matrix);
      if (!path) return;
      ctx.fillStyle = color;
      ctx.fill(path, "evenodd");
    };
    const stroke = (kind: string, color: string, width: number, dash: number[] = []) => {
      const name = layerFor(kind, v.zoom);
      const path = name && this.visible(name, v, matrix);
      if (!path) return;
      ctx.strokeStyle = color;
      ctx.lineWidth = width;
      ctx.setLineDash(dash);
      ctx.stroke(path);
    };
    ctx.lineJoin = "round";
    ctx.lineCap = "round";
    fill("land", c.land);
    fill("urban", c.urban);
    fill("lakes", c.water);
    stroke("river", c.water, v.zoom < 6 ? 0.8 : 1.4);
    stroke("state", c.state, 0.8, [4, 3]);
    stroke("road", c.road, v.zoom < 7 ? 0.8 : 1.5);
    stroke("border", c.border, 1.1);
    ctx.setLineDash([]);
    this.drawLabels(ctx, v, c);
  }

  private drawLabels(ctx: CanvasRenderingContext2D, v: View, c: Colors) {
    const placed: [number, number, number, number][] = [];
    const free = (x0: number, y0: number, x1: number, y1: number) =>
      placed.every(([a0, b0, a1, b1]) => x1 < a0 || x0 > a1 || y1 < b0 || y0 > b1);
    const put = (text: string, sx: number, sy: number, font: string, dot: boolean) => {
      ctx.font = font;
      const w = ctx.measureText(text).width;
      const tx = dot ? sx + 5 : sx - w / 2;
      const box: [number, number, number, number] = [tx - 3, sy - 9, tx + w + 3, sy + 7];
      if (!free(...box)) return;
      placed.push(box);
      if (dot) {
        ctx.fillStyle = c.text;
        ctx.beginPath();
        ctx.arc(sx, sy, 2.2, 0, Math.PI * 2);
        ctx.fill();
      }
      ctx.lineWidth = 3;
      ctx.strokeStyle = c.halo;
      ctx.strokeText(text, tx, sy + 4);
      ctx.fillStyle = c.text;
      ctx.fillText(text, tx, sy + 4);
    };
    const inView = (sx: number, sy: number) => sx > -50 && sx < v.width + 50 && sy > -20 && sy < v.height + 20;

    ctx.textBaseline = "alphabetic";
    if (v.zoom < 7) {
      for (const l of this.labels) {
        if (l.minZoom > v.zoom + 1.5) continue;
        const { sx, sy } = toScreen(v, l.x, l.y);
        if (inView(sx, sy)) put(l.name, sx, sy, "600 12px system-ui, sans-serif", false);
      }
    }
    const min = minPopulation(v.zoom);
    let shown = 0;
    for (const city of this.cities) {
      if (city.population < min || shown > 400) break;
      const { sx, sy } = toScreen(v, city.x, city.y);
      if (!inView(sx, sy)) continue;
      put(city.name, sx, sy, "12px system-ui, sans-serif", true);
      shown++;
    }
  }
}
