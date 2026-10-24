import { useSyncExternalStore } from "react";
import type { ExportOptions, TimelineView } from "../api";
import { AlbumPicker } from "../collections/AlbumPicker";
import { useCollections } from "../collections/context";
import { TagInput } from "../collections/TagInput";
import { copiedEdit } from "../editor/copied";
import { Popover } from "../ui/Popover";
import { DateFixer } from "./DateFixer";
import { EventMaker } from "./EventMaker";
import { ExportPanel } from "./ExportPanel";
import { PastePanel } from "./PastePanel";
import { RenamePanel } from "./RenamePanel";

type Props = {
  count: number;
  ids: number[];
  onSelectAll: () => void;
  onShiftDates: (deltaMs: number) => void;
  onRestoreDates: () => void;
  view: TimelineView;
  onClear: () => void;
  onFavorite: (favorite: boolean) => void;
  onRate: (rating: number | null) => void;
  onHide: (hidden: boolean) => void;
  /** Moves the photos' files to the trash folder (asks first). */
  onRemove: () => void;
  /** Turns the photos: 1 clockwise, -1 counter-clockwise. */
  onRotate: (quarters: number) => void;
  /** Pastes the copied edit: all of it, or light and color only. */
  onPasteEdit: (what: "everything" | "tone") => void;
  /** Renames the photos' files from a pattern (undoable). */
  onRename: (pattern: string) => void;
  /** Writes copies, resized/converted, to a folder (in the background). */
  onExport: (options: ExportOptions) => void;
  onAddToAlbum: (albumId: number) => void;
  onCreateAlbum: (name: string) => void;
  onTag: (name: string) => void;
  onCreateEvent: (name: string) => void;
  /** Present only inside an event. */
  onSplitEvent?: () => void;
  /** Present only inside an album. */
  onRemoveFromAlbum?: () => void;
  onSetCover?: () => void;
};

export function SelectionBar(props: Props) {
  const { count, view, onClear, onFavorite, onRate, onHide } = props;
  const { albums, tags } = useCollections();
  const copied = useSyncExternalStore(copiedEdit.subscribe, copiedEdit.get);
  return (
    <div className="selection-bar" role="toolbar" aria-label="Selection actions">
      <button className="ghost" onClick={onClear} title="Clear selection (Esc)">
        ✕
      </button>
      <strong>{count.toLocaleString()} selected</strong>
      <button className="link small" onClick={props.onSelectAll} title="Select all (Ctrl+A)">
        Select all
      </button>
      <div className="spacer" />
      <button className="ghost" onClick={() => props.onRotate(-1)} title="Rotate left">
        ⟲
      </button>
      <button className="ghost" onClick={() => props.onRotate(1)} title="Rotate right">
        ⟳
      </button>
      {copied && (
        <Popover label="Paste edit" align="right">
          {(close) => (
            <PastePanel
              count={count}
              copied={copied}
              onPaste={(what) => {
                props.onPasteEdit(what);
                close();
              }}
            />
          )}
        </Popover>
      )}
      <Popover label="Fix date" align="right">
        {(close) => (
          <DateFixer
            ids={props.ids}
            onApply={(delta) => {
              props.onShiftDates(delta);
              close();
            }}
            onRestore={() => {
              props.onRestoreDates();
              close();
            }}
          />
        )}
      </Popover>
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
      <Popover label="Event">
        {(close) => (
          <EventMaker
            count={count}
            onCreate={(name) => {
              props.onCreateEvent(name);
              close();
            }}
            onSplit={
              props.onSplitEvent && count === 1
                ? () => {
                    props.onSplitEvent!();
                    close();
                  }
                : undefined
            }
          />
        )}
      </Popover>
      <Popover label="Rename" align="right">
        {(close) => (
          <RenamePanel
            ids={props.ids}
            onRename={(pattern) => {
              props.onRename(pattern);
              close();
            }}
          />
        )}
      </Popover>
      <Popover label="Export" align="right">
        {(close) => (
          <ExportPanel
            count={count}
            previewId={props.ids[0]}
            onExport={(options) => {
              props.onExport(options);
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
      <button onClick={props.onRemove} title="Move to the trash folder (Delete)">
        Remove
      </button>
    </div>
  );
}
