import { useCallback, useEffect, useMemo, useRef, useState, type MouseEvent } from "react";
import { api, type Layout, type View } from "../api";
import { DayStore, type Position } from "../timeline/dayStore";
import { Timeline } from "../timeline/Timeline";
import { Viewer } from "../viewer/Viewer";
import { SelectionBar } from "./SelectionBar";

type Props = {
  view: View;
  layout: Layout;
  /** Reload layout and counts after changes that move items between views. */
  onLibraryChanged: () => Promise<void>;
  onError: (e: unknown) => void;
};

/** One timeline view (All, Favorites, …) with selection and the full-screen viewer. */
export function PhotosView({ view, layout, onLibraryChanged, onError }: Props) {
  // `layout` only seeds a new store; later layouts arrive through setLayout below.
  const store = useMemo(() => new DayStore(view, layout), [view]);
  const [selection, setSelection] = useState<ReadonlySet<number>>(new Set());
  const anchor = useRef<Position | null>(null);
  const [viewing, setViewing] = useState<Position | null>(null);
  const [reveal, setReveal] = useState<Position | null>(null);

  useEffect(() => store.setLayout(layout), [store, layout]);

  const clearSelection = useCallback(() => {
    setSelection(new Set());
    anchor.current = null;
  }, []);

  useEffect(() => {
    if (viewing) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && selection.size > 0) clearSelection();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [viewing, selection.size, clearSelection]);

  const toggle = (pos: Position, id: number) => {
    setSelection((s) => {
      const next = new Set(s);
      if (!next.delete(id)) next.add(id);
      return next;
    });
    anchor.current = pos;
  };

  /** Adds everything between the anchor and `pos` (inclusive), loading days as needed. */
  const selectRange = async (pos: Position) => {
    const from = anchor.current;
    if (!from) return;
    const a = store.indexOf(from.day);
    const b = store.indexOf(pos.day);
    if (a < 0 || b < 0) return;
    const [start, end] = a < b || (a === b && from.index <= pos.index) ? [from, pos] : [pos, from];
    const days = layout.days.slice(Math.min(a, b), Math.max(a, b) + 1);
    await store.ensure(days);
    const ids: number[] = [];
    for (const day of days) {
      const dayIds = store.get(day)?.ids ?? [];
      const lo = day === start.day ? start.index : 0;
      const hi = day === end.day ? end.index : dayIds.length - 1;
      ids.push(...dayIds.slice(lo, hi + 1));
    }
    setSelection((s) => new Set([...s, ...ids]));
  };

  const onActivate = (e: MouseEvent, pos: Position, id: number) => {
    if (e.shiftKey && anchor.current) selectRange(pos).catch(onError);
    else if (e.ctrlKey || e.metaKey || selection.size > 0) toggle(pos, id);
    else setViewing(pos);
  };

  const toggleDay = (day: number) => {
    const ids = store.get(day)?.ids ?? [];
    setSelection((s) => {
      const next = new Set(s);
      const all = ids.length > 0 && ids.every((id) => next.has(id));
      ids.forEach((id) => (all ? next.delete(id) : next.add(id)));
      return next;
    });
  };

  const setFavorite = useCallback(
    async (ids: number[], favorite: boolean) => {
      store.patchFavorites(new Set(ids), favorite);
      await api.setFavorite(ids, favorite);
      if (view === "favorites") await onLibraryChanged();
    },
    [store, view, onLibraryChanged],
  );

  const setHidden = useCallback(
    async (ids: number[], hidden: boolean) => {
      await api.setHidden(ids, hidden);
      await onLibraryChanged();
    },
    [onLibraryChanged],
  );

  const run = (action: () => Promise<void>) => action().catch(onError);

  return (
    <div className="photos-view">
      {selection.size > 0 && (
        <SelectionBar
          count={selection.size}
          view={view}
          onClear={clearSelection}
          onFavorite={(favorite) => run(() => setFavorite([...selection], favorite))}
          onRate={(rating) => run(() => api.setRating([...selection], rating))}
          onHide={(hidden) =>
            run(async () => {
              await setHidden([...selection], hidden);
              clearSelection();
            })
          }
        />
      )}
      <Timeline
        store={store}
        selection={selection}
        keyboard={!viewing}
        onActivate={onActivate}
        onToggle={toggle}
        onToggleDay={toggleDay}
        reveal={reveal}
      />
      {viewing && (
        <Viewer
          store={store}
          start={viewing}
          onClose={(last) => {
            setViewing(null);
            setReveal(last);
          }}
          onFavorite={setFavorite}
          onHidden={setHidden}
        />
      )}
    </div>
  );
}
