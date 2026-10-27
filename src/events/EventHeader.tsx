import { confirm } from "../ui/confirm";
import { useEffect, useState } from "react";
import { api, type EventSummary } from "../api";
import { useCollections } from "../collections/context";
import { defaultEventTitle, eventTitle, formatEventDates, hasName } from "./format";

type Props = {
  id: number;
  /** Bumped by the parent after edits that change the event (split, dates). */
  version: number;
  onError: (e: unknown) => void;
  /** Plays the event as a slideshow. */
  onPlay: () => void;
};

/** Title row for an event view: inline rename, back to Events, remove. */
export function EventHeader({ id, version, onError, onPlay }: Props) {
  const { navigate } = useCollections();
  const [event, setEvent] = useState<EventSummary | null>(null);
  const [editing, setEditing] = useState<string | null>(null);

  useEffect(() => {
    api.event(id).then(setEvent, onError);
  }, [id, version, onError]);

  if (!event) return null;

  const save = async () => {
    const title = editing?.trim() ?? "";
    setEditing(null);
    if (title === (event.title ?? "")) return;
    try {
      await api.renameEvent(id, title);
      setEvent(await api.event(id));
    } catch (e) {
      onError(e);
    }
  };

  const remove = async () => {
    const ok = await confirm(
      `Stop showing "${eventTitle(event)}" as an event?\n\nThe photos stay in your library; they just won't be grouped like this again.`,
      { title: "Remove event", kind: "warning", okLabel: "Remove event" },
    );
    if (!ok) return;
    try {
      await api.removeEvent(id);
      navigate("events");
    } catch (e) {
      onError(e);
    }
  };

  return (
    <div className="album-header">
      <button className="ghost" onClick={() => navigate("events")} title="All events">
        ‹ Events
      </button>
      {editing !== null ? (
        <input
          className="album-title-input"
          autoFocus
          placeholder={defaultEventTitle(event)}
          value={editing}
          onChange={(e) => setEditing(e.target.value)}
          onBlur={save}
          onKeyDown={(e) => {
            if (e.key === "Enter") save();
            if (e.key === "Escape") setEditing(null);
          }}
        />
      ) : (
        <h2 className="album-title" onClick={() => setEditing(event.title ?? "")} title="Click to name this event">
          {eventTitle(event)}
        </h2>
      )}
      <span className="muted">
        {hasName(event) ? `${formatEventDates(event)} · ` : ""}
        {event.count.toLocaleString()} items
      </span>
      <div className="spacer" />
      <span className="muted small">Select a photo to split the event there</span>
      <button className="ghost" onClick={onPlay} title="Slideshow">
        ▶ Play
      </button>
      <button className="ghost" onClick={remove}>
        Remove event
      </button>
    </div>
  );
}
