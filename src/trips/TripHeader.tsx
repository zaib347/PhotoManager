import { confirm } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { api, type TripSummary } from "../api";
import { useCollections } from "../collections/context";
import { formatTripDates, tripLength, tripPlaces, tripTitle } from "./format";

type Props = {
  id: number;
  /** Bumped by the parent after edits that may change the trip (dates). */
  version: number;
  onError: (e: unknown) => void;
};

/** Title row for a trip view: inline rename, back to Trips, remove. */
export function TripHeader({ id, version, onError }: Props) {
  const { navigate } = useCollections();
  const [trip, setTrip] = useState<TripSummary | null>(null);
  const [editing, setEditing] = useState<string | null>(null);

  useEffect(() => {
    api.trip(id).then(setTrip, onError);
  }, [id, version, onError]);

  if (!trip) return null;

  const save = async () => {
    const title = editing?.trim() ?? "";
    setEditing(null);
    if (title === (trip.title ?? "")) return;
    try {
      await api.renameTrip(id, title);
      setTrip(await api.trip(id));
    } catch (e) {
      onError(e);
    }
  };

  const remove = async () => {
    const ok = await confirm(
      `Stop showing "${tripTitle(trip)}" as a trip?\n\nThe photos stay in your library; these days just won't be suggested as a trip again.`,
      { title: "Remove trip", kind: "warning", okLabel: "Remove trip" },
    );
    if (!ok) return;
    try {
      await api.removeTrip(id);
      navigate("trips");
    } catch (e) {
      onError(e);
    }
  };

  return (
    <div className="album-header">
      <button className="ghost" onClick={() => navigate("trips")} title="All trips">
        ‹ Trips
      </button>
      {editing !== null ? (
        <input
          className="album-title-input"
          autoFocus
          placeholder={tripPlaces(trip)}
          value={editing}
          onChange={(e) => setEditing(e.target.value)}
          onBlur={save}
          onKeyDown={(e) => {
            if (e.key === "Enter") save();
            if (e.key === "Escape") setEditing(null);
          }}
        />
      ) : (
        <h2 className="album-title" onClick={() => setEditing(trip.title ?? "")} title="Click to name this trip">
          {tripTitle(trip)}
        </h2>
      )}
      <span className="muted">
        {formatTripDates(trip)} · {tripLength(trip)} · {trip.count.toLocaleString()} items
      </span>
      <div className="spacer" />
      <button className="ghost" onClick={remove}>
        Remove trip
      </button>
    </div>
  );
}
