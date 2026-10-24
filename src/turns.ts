/**
 * Photos the user turned, and by how much (quarter turns clockwise). Thumbnails and
 * viewer images of a turned photo get a different URL per turn, so the webview never
 * shows a cached image the wrong way round. Loaded at startup, updated after rotating.
 */
const turns = new Map<number, number>();
const listeners = new Set<() => void>();
let version = 0;

export const photoTurns = {
  get: (id: number) => turns.get(id) ?? 0,
  /** Records new turns, as the backend returned them. */
  set(changes: [number, number][]) {
    for (const [id, t] of changes) {
      if (t === 0) turns.delete(id);
      else turns.set(id, t);
    }
    version++;
    listeners.forEach((l) => l());
  },
  subscribe(listener: () => void) {
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  },
  getVersion: () => version,
};

/** `?t=<turns>` for a turned photo, nothing otherwise. */
export const turnsQuery = (id: number) => {
  const t = photoTurns.get(id);
  return t ? `?t=${t}` : "";
};
