import { useEffect, useState, type MouseEvent } from "react";
import { api, faceUrl, type IndexCount, type Person } from "../api";
import { useCollections } from "../collections/context";

type Props = {
  /** Bumped when faces were found or people changed. */
  version: number;
  /** Faces looked for so far, while People is installed. */
  progress: IndexCount | null;
  onError: (e: unknown) => void;
};

/** A person's name, or a stand-in until they're named. */
export const personName = (p: Person) => p.name ?? "Add a name";

/** Everyone found in the photos, named first. Ctrl/⌘-click selects people to merge. */
export function PeoplePage({ version, progress, onError }: Props) {
  const { navigate } = useCollections();
  const [hidden, setHidden] = useState(false);
  const [list, setList] = useState<Person[] | null>(null);
  const [selected, setSelected] = useState<ReadonlySet<number>>(new Set());
  const [naming, setNaming] = useState<{ id: number; text: string } | null>(null);
  const [reload, setReload] = useState(0);

  useEffect(() => {
    api.aiPeople(hidden).then(setList, onError);
  }, [version, hidden, reload, onError]);

  useEffect(() => {
    if (selected.size === 0) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setSelected(new Set());
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selected.size]);

  if (!list) return null;

  const open = (e: MouseEvent, id: number) => {
    if (e.ctrlKey || e.metaKey || selected.size > 0) {
      setSelected((s) => {
        const next = new Set(s);
        if (!next.delete(id)) next.add(id);
        return next;
      });
    } else {
      navigate({ person: id });
    }
  };

  const run = (action: () => Promise<void>) =>
    action()
      .then(() => setReload((r) => r + 1))
      .catch(onError);

  /** Merges the selection into the named one (or the one with most photos). */
  const merge = () =>
    run(async () => {
      const chosen = list.filter((p) => selected.has(p.id));
      const into = chosen.find((p) => p.name) ?? chosen[0];
      await api.aiMergePeople(
        into.id,
        chosen.filter((p) => p !== into).map((p) => p.id),
      );
      setSelected(new Set());
    });

  const saveName = (person: Person) =>
    run(async () => {
      const text = naming?.text.trim() ?? "";
      setNaming(null);
      if (text === (person.name ?? "")) return;
      // The same name as someone else: they're one person.
      const twin = list.find((p) => p.id !== person.id && p.name?.toLowerCase() === text.toLowerCase());
      if (twin) await api.aiMergePeople(twin.id, [person.id]);
      else await api.aiRenamePerson(person.id, text);
    });

  const working = progress && progress.indexed < progress.total;
  const intro =
    list.length === 0
      ? hidden
        ? "No hidden people."
        : working
          ? "Looking for faces… People appear here once someone is in a few photos."
          : "No people found yet. People appear here once someone is in a few photos."
      : `${list.length.toLocaleString()} ${list.length === 1 ? "person" : "people"} · click a name to change it · Ctrl-click to select people to merge`;

  return (
    <div className="events-page people-page">
      {selected.size > 0 ? (
        <div className="selection-bar" role="toolbar" aria-label="People selection">
          <button className="ghost" onClick={() => setSelected(new Set())} title="Clear selection (Esc)">
            ✕
          </button>
          <strong>{selected.size.toLocaleString()} selected</strong>
          <div className="spacer" />
          <button className="primary" disabled={selected.size < 2} onClick={merge}>
            Same person: merge
          </button>
          <button
            onClick={() =>
              run(async () => {
                for (const id of selected) await api.aiHidePerson(id, !hidden);
                setSelected(new Set());
              })
            }
          >
            {hidden ? "Show" : "Hide"}
          </button>
        </div>
      ) : (
        <div className="people-bar">
          <p className="events-hint muted small">
            {intro}
            {working && list.length > 0 && ` · looked at ${progress.indexed.toLocaleString()} of ${progress.total.toLocaleString()} photos`}
          </p>
          <div className="spacer" />
          <button className="link small" onClick={() => setHidden((h) => !h)}>
            {hidden ? "‹ Back to people" : "Hidden people"}
          </button>
        </div>
      )}
      <div className="people-cards">
        {list.map((p) => (
          <div key={p.id} className={`person-card${selected.has(p.id) ? " selected" : ""}`}>
            <button
              className="person-face"
              onClick={(e) => open(e, p.id)}
              aria-pressed={selected.size > 0 ? selected.has(p.id) : undefined}
              title={p.name ?? "Open"}
            >
              {p.faceId > 0 && <img src={faceUrl(p.faceId)} alt="" loading="lazy" />}
            </button>
            {naming?.id === p.id ? (
              <input
                className="person-name-input"
                autoFocus
                value={naming.text}
                placeholder="Name"
                onChange={(e) => setNaming({ id: p.id, text: e.target.value })}
                onBlur={() => saveName(p)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") saveName(p);
                  if (e.key === "Escape") setNaming(null);
                }}
              />
            ) : (
              <button
                className={`person-name link${p.name ? "" : " muted"}`}
                onClick={() => setNaming({ id: p.id, text: p.name ?? "" })}
              >
                {personName(p)}
              </button>
            )}
            <span className="muted small">
              {p.photos.toLocaleString()} {p.photos === 1 ? "photo" : "photos"}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}
