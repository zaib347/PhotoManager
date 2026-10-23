import type { View } from "../api";
import { AlbumPicker } from "../collections/AlbumPicker";
import { useCollections } from "../collections/context";
import { TagInput } from "../collections/TagInput";
import { Popover } from "../ui/Popover";

type Props = {
  count: number;
  view: View;
  onClear: () => void;
  onFavorite: (favorite: boolean) => void;
  onRate: (rating: number | null) => void;
  onHide: (hidden: boolean) => void;
  onAddToAlbum: (albumId: number) => void;
  onCreateAlbum: (name: string) => void;
  onTag: (name: string) => void;
  /** Present only inside an album. */
  onRemoveFromAlbum?: () => void;
  onSetCover?: () => void;
};

export function SelectionBar(props: Props) {
  const { count, view, onClear, onFavorite, onRate, onHide } = props;
  const { albums, tags } = useCollections();
  return (
    <div className="selection-bar" role="toolbar" aria-label="Selection actions">
      <button className="ghost" onClick={onClear} title="Clear selection (Esc)">
        ✕
      </button>
      <strong>{count.toLocaleString()} selected</strong>
      <div className="spacer" />
      <Popover label="Add to album">
        {(close) => (
          <AlbumPicker
            albums={albums}
            onPick={(id) => {
              props.onAddToAlbum(id);
              close();
            }}
            onCreate={(name) => {
              props.onCreateAlbum(name);
              close();
            }}
          />
        )}
      </Popover>
      <Popover label="Tag">
        {(close) => (
          <TagInput
            autoFocus
            tags={tags}
            onAdd={(name) => {
              props.onTag(name);
              close();
            }}
          />
        )}
      </Popover>
      {props.onRemoveFromAlbum && <button onClick={props.onRemoveFromAlbum}>Remove from album</button>}
      {props.onSetCover && count === 1 && <button onClick={props.onSetCover}>Set as cover</button>}
      <button onClick={() => onFavorite(true)}>♥ Favorite</button>
      <button onClick={() => onFavorite(false)}>♡ Unfavorite</button>
      <div className="stars" role="group" aria-label="Rate selection">
        {[1, 2, 3, 4, 5].map((n) => (
          <button key={n} className="ghost star" onClick={() => onRate(n)} title={`Rate ${n}`}>
            ★
          </button>
        ))}
        <button className="ghost" onClick={() => onRate(null)} title="Clear rating">
          ⌫
        </button>
      </div>
      <button onClick={() => onHide(view !== "hidden")}>{view === "hidden" ? "Unhide" : "Hide"}</button>
    </div>
  );
}
