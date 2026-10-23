import { useState } from "react";
import type { AlbumInfo } from "../api";

type Props = {
  albums: AlbumInfo[];
  onPick: (albumId: number) => void;
  onCreate: (name: string) => void;
};

/** Filterable album list with "create new album" from the typed name. */
export function AlbumPicker({ albums, onPick, onCreate }: Props) {
  const [query, setQuery] = useState("");
  const q = query.trim().toLowerCase();
  const matches = albums.filter((a) => a.name.toLowerCase().includes(q));
  const exact = albums.some((a) => a.name.toLowerCase() === q);

  return (
    <div className="picker">
      <input
        autoFocus
        placeholder="Find or create an album"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={(e) => {
          if (e.key !== "Enter" || !q) return;
          if (matches.length === 1) onPick(matches[0].id);
          else if (!exact) onCreate(query.trim());
        }}
      />
      <ul className="picker-list">
        {q && !exact && (
          <li>
            <button className="picker-item create" onClick={() => onCreate(query.trim())}>
              + New album “{query.trim()}”
            </button>
          </li>
        )}
        {matches.map((a) => (
          <li key={a.id}>
            <button className="picker-item" onClick={() => onPick(a.id)}>
              <span>{a.name}</span>
              <span className="muted">{a.count.toLocaleString()}</span>
            </button>
          </li>
        ))}
        {albums.length === 0 && !q && <li className="muted picker-empty">Type a name to create your first album.</li>}
      </ul>
    </div>
  );
}
