/**
 * Photos that look different from their files: turned (quarter turns clockwise) or
 * edited (the edit version counts saves). Their thumbnails and viewer images get a
 * different URL for each look, so the webview never shows a cached old look. Loaded
 * at startup, updated after rotating or saving an edit.
 */
type Look = { turns: number; version: number };

const looks = new Map<number, Look>();
const listeners = new Set<() => void>();
let changes = 0;

function update(id: number, patch: Partial<Look>) {
  const next = { turns: 0, version: 0, ...looks.get(id), ...patch };
  if (next.turns === 0 && next.version === 0) looks.delete(id);
  else looks.set(id, next);
}

function notify() {
  changes++;
  listeners.forEach((l) => l());
}

export const photoLooks = {
  turns: (id: number) => looks.get(id)?.turns ?? 0,
  /** Everything at startup: (id, turns, edit version). */
  load(list: [number, number, number][]) {
    for (const [id, turns, version] of list) update(id, { turns, version });
    notify();
  },
  /** New turns, as the backend returned them after rotating. */
  setTurns(list: [number, number][]) {
    for (const [id, turns] of list) update(id, { turns });
    notify();
  },
  setVersion(id: number, version: number) {
    update(id, { version });
    notify();
  },
  subscribe(listener: () => void) {
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  },
  getVersion: () => changes,
};

/** A stable key for a photo's look, for React keys. */
export const lookKey = (id: number) => {
  const look = looks.get(id);
  return look ? `${id}:${look.turns}:${look.version}` : String(id);
};

/** `?t=<turns>&e=<edit version>` for a photo that has a look, nothing otherwise. */
export const lookQuery = (id: number) => {
  const look = looks.get(id);
  if (!look) return "";
  const parts = [];
  if (look.turns) parts.push(`t=${look.turns}`);
  if (look.version) parts.push(`e=${look.version}`);
  return `?${parts.join("&")}`;
};
