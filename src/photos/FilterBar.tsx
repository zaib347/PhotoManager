import { useCallback, useEffect, useState } from "react";
import { api, isFilterEmpty, normalizeFilter, type Filter, type FilterOptions, type RootInfo } from "../api";
import { fileName, formatDay, placeLabel } from "../format";
import { Popover } from "../ui/Popover";

const NO_OPTIONS: FilterOptions = { cameras: [], places: [], countries: [], firstDay: null, lastDay: null };

const DAY_MS = 86_400_000;
const toDateInput = (day: number | undefined) =>
  day === undefined ? "" : new Date(day * DAY_MS).toISOString().slice(0, 10);
const fromDateInput = (value: string) => (value ? Math.floor(Date.parse(`${value}T00:00:00Z`) / DAY_MS) : undefined);
const cameraLabel = (model: string) => model || "Unknown camera";

type Props = {
  filter: Filter;
  roots: RootInfo[];
  onChange: (filter: Filter) => void;
};

/** Filter button with a panel, plus a chip for each active filter. */
export function FilterBar({ filter, roots, onChange }: Props) {
  // Kept here rather than in the panel so place chips can show names. Loaded when the
  // panel opens, or up front only if a chip needs a name (it's ~1 s at 1M photos).
  const [options, setOptions] = useState<FilterOptions | null>(null);
  const loadOptions = useCallback(() => {
    api.filterOptions().then(setOptions, () => setOptions(NO_OPTIONS));
  }, []);
  const needsNames = !!(filter.placeIds?.length || filter.countries?.length);
  useEffect(() => {
    if (needsNames && !options) loadOptions();
  }, [needsNames, options, loadOptions]);

  const set = (patch: Partial<Filter>) => onChange(normalizeFilter({ ...filter, ...patch }));
  const toggle = <T,>(list: T[] | undefined, item: T) =>
    list?.includes(item) ? list.filter((x) => x !== item) : [...(list ?? []), item];

  const chips: { label: string; clear: () => void }[] = [];
  if (filter.media) chips.push({ label: filter.media === "photo" ? "Photos" : "Videos", clear: () => set({ media: undefined }) });
  if (filter.minRating) chips.push({ label: `★ ${filter.minRating}+`, clear: () => set({ minRating: undefined }) });
  if (filter.fromDay !== undefined || filter.toDay !== undefined) {
    const from = filter.fromDay !== undefined ? formatDay(filter.fromDay) : "…";
    const to = filter.toDay !== undefined ? formatDay(filter.toDay) : "…";
    chips.push({ label: `${from} – ${to}`, clear: () => set({ fromDay: undefined, toDay: undefined }) });
  }
  for (const model of filter.cameras ?? []) {
    chips.push({ label: cameraLabel(model), clear: () => set({ cameras: toggle(filter.cameras, model) }) });
  }
  for (const id of filter.rootIds ?? []) {
    const root = roots.find((r) => r.id === id);
    chips.push({ label: root ? fileName(root.path) : "Folder", clear: () => set({ rootIds: toggle(filter.rootIds, id) }) });
  }
  for (const code of filter.countries ?? []) {
    const country = options?.countries.find((c) => c.code === code);
    chips.push({ label: country?.name ?? code, clear: () => set({ countries: toggle(filter.countries, code) }) });
  }
  if (filter.area) chips.push({ label: "Map area", clear: () => set({ area: undefined }) });
  for (const id of filter.placeIds ?? []) {
    const place = options?.places.find((p) => p.id === id);
    chips.push({ label: place?.name ?? "Place", clear: () => set({ placeIds: toggle(filter.placeIds, id) }) });
  }

  return (
    <div className="filter-bar">
      <Popover label={isFilterEmpty(filter) ? "Filter" : `Filter · ${chips.length}`}>
        {() => <FilterPanel filter={filter} roots={roots} options={options} reload={loadOptions} set={set} toggle={toggle} />}
      </Popover>
      {chips.map((c) => (
        <span key={c.label} className="chip filter-chip">
          {c.label}
          <button className="chip-remove" onClick={c.clear} aria-label={`Remove filter ${c.label}`}>
            ×
          </button>
        </span>
      ))}
      {chips.length > 1 && (
        <button className="link small" onClick={() => onChange({})}>
          Clear all
        </button>
      )}
    </div>
  );
}

type PanelProps = {
  filter: Filter;
  roots: RootInfo[];
  options: FilterOptions | null;
  /** Refreshes the choices (a scan may have added cameras or places). */
  reload: () => void;
  set: (patch: Partial<Filter>) => void;
  toggle: <T>(list: T[] | undefined, item: T) => T[];
};

function FilterPanel({ filter, roots, options, reload, set, toggle }: PanelProps) {
  const [placeQuery, setPlaceQuery] = useState("");
  useEffect(reload, [reload]);

  const query = placeQuery.trim().toLowerCase();
  const places = (options?.places ?? []).filter(
    (p) => !query || [p.name, p.region ?? "", p.country].some((s) => s.toLowerCase().includes(query)),
  );

  return (
    <div className="filter-panel">
      <section>
        <h4>Type</h4>
        <div className="segmented">
          {([undefined, "photo", "video"] as const).map((m) => (
            <button key={m ?? "all"} className={filter.media === m ? "on" : ""} onClick={() => set({ media: m })}>
              {m === "photo" ? "Photos" : m === "video" ? "Videos" : "All"}
            </button>
          ))}
        </div>
      </section>

      <section>
        <h4>Rating</h4>
        <div className="segmented">
          {[undefined, 1, 2, 3, 4, 5].map((r) => (
            <button key={r ?? 0} className={filter.minRating === r ? "on" : ""} onClick={() => set({ minRating: r })}>
              {r ? `${r}★+` : "Any"}
            </button>
          ))}
        </div>
      </section>

      <section>
        <h4>Date</h4>
        <div className="date-range">
          <input
            type="date"
            value={toDateInput(filter.fromDay)}
            min={toDateInput(options?.firstDay ?? undefined)}
            max={toDateInput(options?.lastDay ?? undefined)}
            onChange={(e) => set({ fromDay: fromDateInput(e.target.value) })}
            aria-label="From date"
          />
          <span className="muted">to</span>
          <input
            type="date"
            value={toDateInput(filter.toDay)}
            min={toDateInput(options?.firstDay ?? undefined)}
            max={toDateInput(options?.lastDay ?? undefined)}
            onChange={(e) => set({ toDay: fromDateInput(e.target.value) })}
            aria-label="To date"
          />
        </div>
      </section>

      {roots.length > 1 && (
        <section>
          <h4>Folders</h4>
          <ul className="check-list">
            {roots.map((r) => (
              <li key={r.id}>
                <label title={r.path}>
                  <input
                    type="checkbox"
                    checked={filter.rootIds?.includes(r.id) ?? false}
                    onChange={() => set({ rootIds: toggle(filter.rootIds, r.id) })}
                  />
                  <span className="link-label">{fileName(r.path)}</span>
                </label>
              </li>
            ))}
          </ul>
        </section>
      )}

      {options && options.countries.length > 1 && (
        <section>
          <h4>Country</h4>
          <ul className="check-list scroll">
            {options.countries.map((c) => (
              <li key={c.code}>
                <label>
                  <input
                    type="checkbox"
                    checked={filter.countries?.includes(c.code) ?? false}
                    onChange={() => set({ countries: toggle(filter.countries, c.code) })}
                  />
                  <span className="link-label">{c.name}</span>
                  <span className="muted small">{c.count.toLocaleString()}</span>
                </label>
              </li>
            ))}
          </ul>
        </section>
      )}

      {options && options.places.length > 0 && (
        <section>
          <h4>Place</h4>
          {options.places.length > 8 && (
            <input
              type="search"
              className="filter-search"
              placeholder="Find a place"
              value={placeQuery}
              onChange={(e) => setPlaceQuery(e.target.value)}
              aria-label="Find a place"
            />
          )}
          <ul className="check-list scroll">
            {places.map((p) => (
              <li key={p.id}>
                <label title={placeLabel(p)}>
                  <input
                    type="checkbox"
                    checked={filter.placeIds?.includes(p.id) ?? false}
                    onChange={() => set({ placeIds: toggle(filter.placeIds, p.id) })}
                  />
                  <span className="link-label">{p.name}</span>
                  <span className="muted small">{p.count.toLocaleString()}</span>
                </label>
              </li>
            ))}
            {places.length === 0 && <li className="muted small">No places match</li>}
          </ul>
        </section>
      )}

      <section>
        <h4>Camera</h4>
        {!options ? (
          <p className="muted small">Loading…</p>
        ) : (
          <ul className="check-list scroll">
            {options.cameras.map((c) => (
              <li key={c.model}>
                <label>
                  <input
                    type="checkbox"
                    checked={filter.cameras?.includes(c.model) ?? false}
                    onChange={() => set({ cameras: toggle(filter.cameras, c.model) })}
                  />
                  <span className="link-label">{cameraLabel(c.model)}</span>
                  <span className="muted small">{c.count.toLocaleString()}</span>
                </label>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}
