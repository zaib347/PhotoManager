import { confirm } from "../ui/confirm";
import { useState } from "react";
import { api, type AlbumInfo } from "../api";
import { useCollections } from "./context";

/** Title row for an album view: inline rename and delete. */
export function AlbumHeader({ album, onError }: { album: AlbumInfo; onError: (e: unknown) => void }) {
  const { reload, navigate } = useCollections();
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(album.name);

  const save = async () => {
    setEditing(false);
    if (name.trim() === album.name) return;
    try {
      await api.renameAlbum(album.id, name);
      await reload();
    } catch (e) {
      setName(album.name);
      onError(e);
    }
  };

  const remove = async () => {
    const ok = await confirm(
      `Delete the album "${album.name}"?\n\nThe photos stay in your library and folders; only the album is removed.`,
      { title: "Delete album", kind: "warning", okLabel: "Delete album" },
    );
    if (!ok) return;
    try {
      await api.deleteAlbum(album.id);
      navigate("all");
      await reload();
    } catch (e) {
      onError(e);
    }
  };

  return (
    <div className="album-header">
      {editing ? (
        <input
          className="album-title-input"
          autoFocus
          value={name}
          onChange={(e) => setName(e.target.value)}
          onBlur={save}
          onKeyDown={(e) => {
            if (e.key === "Enter") save();
            if (e.key === "Escape") {
              setName(album.name);
              setEditing(false);
            }
          }}
        />
      ) : (
        <h2 className="album-title" onClick={() => setEditing(true)} title="Click to rename">
          {album.name}
        </h2>
      )}
      <span className="muted">{album.count.toLocaleString()} items</span>
      <div className="spacer" />
      <button className="ghost" onClick={remove}>
        Delete album
      </button>
    </div>
  );
}
