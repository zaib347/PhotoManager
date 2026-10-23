import { useEffect, useMemo, useState } from "react";
import { api, thumbUrl, type TripSummary } from "../api";
import { useCollections } from "../collections/context";
import { formatTripDates, tripTitle, tripYear } from "./format";

type Props = {
  /** Bumped by the parent when scans or detection may have changed trips. */
  version: number;
  onError: (e: unknown) => void;
};

/** Every trip as a card, newest first, grouped by year. */
export function TripsPage({ version, onError }: Props) {
  const { navigate } = useCollections();
  const [list, setList] = useState<TripSummary[] | null>(null);

  useEffect(() => {
    api.listTrips().then(setList, onError);
  }, [version, onError]);

  const years = useMemo(() => {
    const groups: { year: number; trips: TripSummary[] }[] = [];
    for (const t of list ?? []) {
      const year = tripYear(t);
      let last = groups[groups.length - 1];
      if (last?.year !== year) groups.push((last = { year, trips: [] }));
      last.trips.push(t);
    }
    return groups;
  }, [list]);

  if (!list) return null;
  if (list.length === 0) {
    return (
      <div className="empty">
        <h2>No trips yet</h2>
        <p className="muted">
          Days spent far from home become trips automatically. This needs photos with a location, such as most phone
          photos.
        </p>
      </div>
    );
  }

  return (
    <div className="events-page">
      <p className="events-hint muted small">{list.length.toLocaleString()} trips · days spent over 100 km from home</p>
      {years.map(({ year, trips }) => (
        <section
          key={year}
          className="events-year"
          // Lets the browser skip layout and paint for years that are off screen.
          style={{ containIntrinsicSize: `auto ${Math.ceil(trips.length / 5) * 250}px` }}
        >
          <h2 className="events-year-title">{year}</h2>
          <div className="event-cards">
            {trips.map((t) => (
              <button key={t.id} className="event-card" onClick={() => navigate({ trip: t.id })}>
                <div className="event-cover">
                  {t.coverId !== null && <img src={thumbUrl(t.coverId)} alt="" loading="lazy" />}
                </div>
                <span className="event-name">{tripTitle(t)}</span>
                <span className="muted small">
                  {formatTripDates(t)} · {t.count.toLocaleString()} items
                </span>
              </button>
            ))}
          </div>
        </section>
      ))}
    </div>
  );
}
