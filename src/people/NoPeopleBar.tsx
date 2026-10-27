import { useState } from "react";
import { api, type Filter, type IndexCount } from "../api";

/** Ready-made searches that are about people, so never match a photo without any. */
const ABOUT_PEOPLE = new Set(["Babies", "Parties"]);

type Props = {
  /** Ready-made searches (name, description), once search by description is ready. */
  things: [string, string][];
  filter: Filter;
  onFilter: (filter: Filter) => void;
  /** How far People has looked for faces. */
  progress: IndexCount | null;
  onError: (e: unknown) => void;
};

/**
 * Above the "No people" timeline: how many photos have been looked at so far, and
 * (with search by description) chips that narrow it to beaches, food, documents…
 */
export function NoPeopleBar({ things, filter, onFilter, progress, onError }: Props) {
  const [searching, setSearching] = useState<string | null>(null);
  const chips = things.filter(([name]) => !ABOUT_PEOPLE.has(name));
  const working = !!progress && progress.indexed < progress.total;
  if (chips.length === 0 && !working) return null;

  const pick = async (name: string, description: string) => {
    setSearching(name);
    try {
      const ids = await api.aiSearch(description, false);
      onFilter({ ...filter, ids, searchText: name });
    } catch (e) {
      onError(e);
    } finally {
      setSearching(null);
    }
  };

  return (
    <div className="no-people-bar">
      {chips.length > 0 && (
        <div className="chips">
          <button className={`chip${filter.ids ? "" : " on"}`} onClick={() => onFilter({ ...filter, ids: undefined, searchText: undefined })}>
            All
          </button>
          {chips.map(([name, description]) => (
            <button
              key={name}
              className={`chip${filter.ids && filter.searchText === name ? " on" : ""}`}
              disabled={searching !== null}
              onClick={() => pick(name, description)}
            >
              {searching === name ? `${name}…` : name}
            </button>
          ))}
        </div>
      )}
      {working && (
        <p className="muted small">
          Looked at {progress.indexed.toLocaleString()} of {progress.total.toLocaleString()} photos for faces so far;
          more appear here as the rest are looked at.
        </p>
      )}
    </div>
  );
}
