import { useEffect, useState } from "react";
import { api, thumbUrl, type Country } from "../api";
import { useCollections } from "../collections/context";

/** Towns shown per country before "Show all". */
const FIRST_TOWNS = 10;

type Props = {
  /** Bumped by the parent when scans may have changed places. */
  version: number;
  onError: (e: unknown) => void;
};

/** Every country with photos, most first, with its towns as cards. */
export function PlacesPage({ version, onError }: Props) {
  const { showPlace } = useCollections();
  const [list, setList] = useState<Country[] | null>(null);
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());

  useEffect(() => {
    api.browsePlaces().then(setList, onError);
  }, [version, onError]);

  if (!list) return null;
  if (list.length === 0) {
    return (
      <div className="empty">
        <h2>No places yet</h2>
        <p className="muted">
          Photos with a location, such as most phone photos, are sorted by the town they were taken in. Place names
          work offline.
        </p>
      </div>
    );
  }

  const towns = list.reduce((n, c) => n + c.places.length, 0);
  return (
    <div className="events-page">
      <p className="events-hint muted small">
        {list.length.toLocaleString()} {list.length === 1 ? "country" : "countries"} · {towns.toLocaleString()}{" "}
        {towns === 1 ? "town" : "towns"}
      </p>
      {list.map((country) => {
        const open = expanded.has(country.code);
        const shown = open ? country.places : country.places.slice(0, FIRST_TOWNS);
        return (
          <section
            key={country.code}
            className="events-year"
            // Lets the browser skip layout and paint for countries that are off screen.
            style={{ containIntrinsicSize: `auto ${Math.ceil(shown.length / 5) * 250 + 60}px` }}
          >
            <h2 className="events-year-title">
              <button className="link place-country" onClick={() => showPlace({ countries: [country.code] })}>
                {country.name}
              </button>{" "}
              <span className="muted small">{country.count.toLocaleString()} items</span>
            </h2>
            <div className="event-cards">
              {shown.map(({ place, count, coverId }) => (
                <button key={place.id} className="event-card" onClick={() => showPlace({ placeIds: [place.id] })}>
                  <div className="event-cover">
                    <img src={thumbUrl(coverId)} alt="" loading="lazy" />
                  </div>
                  <span className="event-name">{place.name}</span>
                  <span className="muted small">
                    {place.region ? `${place.region} · ` : ""}
                    {count.toLocaleString()} items
                  </span>
                </button>
              ))}
            </div>
            {country.places.length > FIRST_TOWNS && (
              <button
                className="link small"
                onClick={() =>
                  setExpanded((s) => {
                    const next = new Set(s);
                    if (!next.delete(country.code)) next.add(country.code);
                    return next;
                  })
                }
              >
                {open ? "Show fewer" : `Show all ${country.places.length.toLocaleString()} towns`}
              </button>
            )}
          </section>
        );
      })}
    </div>
  );
}
