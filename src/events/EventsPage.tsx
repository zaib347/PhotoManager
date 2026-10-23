import { useEffect, useMemo, useState, type MouseEvent } from "react";
import { api, thumbUrl, type EventSummary } from "../api";
import { useCollections } from "../collections/context";
import { eventTitle, eventYear, formatEventDates, hasName } from "./format";

type Props = {
  /** Bumped by the parent when scans or detection change events. */
  version: number;
  onError: (e: unknown) => void;
};

/** Every event as a card, newest first, grouped by year. Ctrl/⌘-click selects for merging. */
export function EventsPage({ version, onError }: Props) {
  const { navigate } = useCollections();
  const [list, setList] = useState<EventSummary[] | null>(null);
  const [selected, setSelected] = useState<ReadonlySet<number>>(new Set());

  useEffect(() => {
    api.listEvents().then(setList, onError);
  }, [version, onError]);

  const years = useMemo(() => {
    const groups: { year: number; events: EventSummary[] }[] = [];
    for (const e of list ?? []) {
      const year = eventYear(e);
      let last = groups[groups.length - 1];
      if (last?.year !== year) groups.push((last = { year, events: [] }));
      last.events.push(e);
    }
    return groups;
  }, [list]);

  useEffect(() => {
    if (selected.size === 0) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setSelected(new Set());
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selected.size]);

  if (!list) return null;
  if (list.length === 0) {
    return (
      <div className="empty">
        <h2>No events yet</h2>
        <p className="muted">
          Photos taken close together are grouped into events automatically as your folders are scanned.
        </p>
      </div>
    );
  }

  const open = (e: MouseEvent, id: number) => {
    if (e.ctrlKey || e.metaKey || selected.size > 0) {
      setSelected((s) => {
        const next = new Set(s);
        if (!next.delete(id)) next.add(id);
        return next;
      });
    } else {
      navigate({ event: id });
    }
  };

  const merge = async () => {
    try {
      const id = await api.mergeEvents([...selected]);
      setSelected(new Set());
      navigate({ event: id });
    } catch (e) {
      onError(e);
    }
  };

  return (
    <div className="events-page">
      {selected.size > 0 ? (
        <div className="selection-bar" role="toolbar" aria-label="Event selection">
          <button className="ghost" onClick={() => setSelected(new Set())} title="Clear selection (Esc)">
            ✕
          </button>
          <strong>{selected.size.toLocaleString()} selected</strong>
          <div className="spacer" />
          <button className="primary" disabled={selected.size < 2} onClick={merge}>
            Merge into one event
          </button>
        </div>
      ) : (
        <p className="events-hint muted small">
          {list.length.toLocaleString()} events · Ctrl-click to select several and merge them
        </p>
      )}
      {years.map(({ year, events }) => (
        <section
          key={year}
          className="events-year"
          // Lets the browser skip layout and paint for years that are off screen.
          style={{ containIntrinsicSize: `auto ${Math.ceil(events.length / 5) * 250}px` }}
        >
          <h2 className="events-year-title">{year}</h2>
          <div className="event-cards">
            {events.map((e) => (
              <button
                key={e.id}
                className={`event-card${selected.has(e.id) ? " selected" : ""}`}
                onClick={(ev) => open(ev, e.id)}
                aria-pressed={selected.size > 0 ? selected.has(e.id) : undefined}
              >
                <div className="event-cover">
                  {e.coverId !== null && <img src={thumbUrl(e.coverId)} alt="" loading="lazy" />}
                </div>
                <span className="event-name">{eventTitle(e)}</span>
                <span className="muted small">
                  {hasName(e) ? `${formatEventDates(e)} · ` : ""}
                  {e.count.toLocaleString()} items
                </span>
              </button>
            ))}
          </div>
        </section>
      ))}
    </div>
  );
}
