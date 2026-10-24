import { useEffect, useState } from "react";
import { api, faceUrl, type Person } from "../api";
import { useCollections } from "../collections/context";
import { personName } from "./PeoplePage";

type Props = {
  id: number;
  /** Bumped by the parent after edits that change the person (photos taken out). */
  version: number;
  onError: (e: unknown) => void;
  /** Plays their photos as a slideshow. */
  onPlay: () => void;
};

/** Title row for a person's photos: their face, inline naming, back to People, hide. */
export function PersonHeader({ id, version, onError, onPlay }: Props) {
  const { navigate } = useCollections();
  const [person, setPerson] = useState<Person | null>(null);
  const [editing, setEditing] = useState<string | null>(null);

  useEffect(() => {
    api.aiPerson(id).then(setPerson, onError);
  }, [id, version, onError]);

  if (!person) return null;

  const save = async () => {
    const name = editing?.trim() ?? "";
    setEditing(null);
    if (name === (person.name ?? "")) return;
    try {
      // The same name as someone else: they're one person.
      const twin = (await api.aiPeople(false)).find(
        (p) => p.id !== id && p.name?.toLowerCase() === name.toLowerCase(),
      );
      if (twin) {
        await api.aiMergePeople(twin.id, [id]);
        navigate({ person: twin.id });
        return;
      }
      await api.aiRenamePerson(id, name);
      setPerson(await api.aiPerson(id));
    } catch (e) {
      onError(e);
    }
  };

  const hide = async () => {
    try {
      await api.aiHidePerson(id, !person.hidden);
      if (person.hidden) setPerson(await api.aiPerson(id));
      else navigate("people");
    } catch (e) {
      onError(e);
    }
  };

  return (
    <div className="album-header">
      <button className="ghost" onClick={() => navigate("people")} title="Everyone">
        ‹ People
      </button>
      {person.faceId > 0 && <img className="person-header-face" src={faceUrl(person.faceId)} alt="" />}
      {editing !== null ? (
        <input
          className="album-title-input"
          autoFocus
          placeholder="Name"
          value={editing}
          onChange={(e) => setEditing(e.target.value)}
          onBlur={save}
          onKeyDown={(e) => {
            if (e.key === "Enter") save();
            if (e.key === "Escape") setEditing(null);
          }}
        />
      ) : (
        <h2
          className={`album-title${person.name ? "" : " muted"}`}
          onClick={() => setEditing(person.name ?? "")}
          title="Click to name this person"
        >
          {personName(person)}
        </h2>
      )}
      <span className="muted">{person.photos.toLocaleString()} photos</span>
      <div className="spacer" />
      <span className="muted small">Select photos that aren't them and choose Not this person</span>
      <button className="ghost" onClick={onPlay} title="Slideshow">
        ▶ Play
      </button>
      <button className="ghost" onClick={hide}>
        {person.hidden ? "Show in People" : "Hide"}
      </button>
    </div>
  );
}
