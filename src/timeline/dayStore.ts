import { api, type DayAssets, type Filter, type Layout, type View } from "../api";

const MAX_CACHED_DAYS = 4000;

/** Position of an asset in the timeline: a day and an index within that day. */
export type Position = { day: number; index: number };

/**
 * Asset ids per day for one view, shared by the grid and the viewer.
 *
 * A new layout (e.g. a scan added photos) marks cached days stale but keeps serving
 * them until refetched, so live updates don't flicker.
 */
export class DayStore {
  layout: Layout;
  version = 0;
  lastFetchMs: number | null = null;
  private cache = new Map<number, { gen: number; data: DayAssets }>();
  private pending = new Map<number, Promise<void>>();
  private dayIndex = new Map<number, number>();
  private gen = 0;
  private listeners = new Set<() => void>();

  constructor(
    readonly view: View,
    readonly filter: Filter,
    layout: Layout,
  ) {
    this.layout = layout;
    this.indexDays();
  }

  setLayout(layout: Layout) {
    if (layout === this.layout) return;
    this.layout = layout;
    this.gen++;
    this.pending.clear();
    this.indexDays();
    this.emit();
  }

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  getVersion = () => this.version;

  get(day: number): DayAssets | undefined {
    return this.cache.get(day)?.data;
  }

  /** Index of a day in the layout, or -1. */
  indexOf(day: number): number {
    return this.dayIndex.get(day) ?? -1;
  }

  /** Fetches days that are missing or stale. Resolves when all are current. */
  ensure(days: number[], keep: Set<number> = new Set(days)): Promise<void> {
    const waits: Promise<void>[] = [];
    const missing: number[] = [];
    for (const day of days) {
      if (this.cache.get(day)?.gen === this.gen) continue;
      const inflight = this.pending.get(day);
      if (inflight) waits.push(inflight);
      else missing.push(day);
    }
    if (missing.length > 0) {
      const gen = this.gen;
      const started = performance.now();
      const request = api.dayAssets(this.view, this.filter, missing).then((result) => {
        if (gen !== this.gen) return;
        result.forEach((d) => this.cache.set(d.day, { gen, data: d }));
        this.evict(keep);
        this.lastFetchMs = performance.now() - started;
        this.emit();
      });
      const settled = request.finally(() => missing.forEach((d) => this.pending.delete(d)));
      missing.forEach((d) => this.pending.set(d, settled));
      waits.push(settled);
    }
    return Promise.all(waits).then(() => undefined);
  }

  /** Neighbouring position in display order, loading the adjacent day if needed. */
  async step(pos: Position, delta: 1 | -1): Promise<Position | null> {
    const data = this.get(pos.day);
    const next = pos.index + delta;
    if (data && next >= 0 && next < data.ids.length) return { day: pos.day, index: next };
    const i = this.indexOf(pos.day) + delta;
    if (i < 0 || i >= this.layout.days.length) return null;
    const day = this.layout.days[i];
    await this.ensure([day]);
    const ids = this.get(day)?.ids ?? [];
    if (ids.length === 0) return null;
    return { day, index: delta === 1 ? 0 : ids.length - 1 };
  }

  idAt(pos: Position): number | undefined {
    return this.get(pos.day)?.ids[pos.index];
  }

  /** Optimistic local update so hearts change instantly; the server is the source of truth. */
  patchFavorites(ids: Set<number>, favorite: boolean) {
    for (const { data } of this.cache.values()) {
      data.ids.forEach((id, i) => {
        if (ids.has(id)) data.favorites[i] = favorite ? 1 : 0;
      });
    }
    this.emit();
  }

  private indexDays() {
    this.dayIndex = new Map(this.layout.days.map((d, i) => [d, i]));
  }

  private evict(keep: Set<number>) {
    if (this.cache.size <= MAX_CACHED_DAYS) return;
    // Map iterates in insertion order, so the oldest fetches go first.
    for (const day of this.cache.keys()) {
      if (this.cache.size <= MAX_CACHED_DAYS * 0.75) break;
      if (!keep.has(day)) this.cache.delete(day);
    }
  }

  private emit() {
    this.version++;
    this.listeners.forEach((l) => l());
  }
}
