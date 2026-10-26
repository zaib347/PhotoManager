import { useSyncExternalStore } from "react";
import type { ExportOptions, TimelineView } from "../api";
import { AlbumPicker } from "../collections/AlbumPicker";
import { useCollections } from "../collections/context";
import { TagInput } from "../collections/TagInput";
import { copiedEdit } from "../editor/copied";
import { Icon } from "../ui/Icon";
import { Popover } from "../ui/Popover";
import { DateFixer } from "./DateFixer";
import { EventMaker } from "./EventMaker";
import { ExportPanel } from "./ExportPanel";
import { LocationPanel } from "./LocationPanel";
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
  /** Places the items at a spot (set by hand, kept over their files' GPS). */
  onSetLocation: (lat: number, lon: number) => void;
  /** Opens the map to place the items with a click. */
  onPlaceOnMap: () => void;
  onClearLocation: () => void;
  /** Opens the collage editor (2–9 photos). */
  onCollage: () => void;
  /** Plays the selection as a slideshow. */
  onSlideshow: () => void;
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
  /** Present only in a person's photos. */
  onNotThisPerson?: () => void;
  onPersonCover?: () => void;
};

/**
 * Actions for the selected photos, in two rows: the selection and quick marks
 * (favorite, rating, hide, remove) on top, grouped tools below.
 */
export function SelectionBar(props: Props) {
  const { count, view, onClear, onFavorite, onRate, onHide } = props;
  const { albums, tags } = useCollections();
  const copied = useSyncExternalStore(copiedEdit.subscribe, copiedEdit.get);
  const hidden = view === "hidden";
  const contextual =
    !!props.onRemoveFromAlbum ||
    !!props.onNotThisPerson ||
    (count === 1 && (!!props.onSetCover || !!props.onPersonCover));
  return (
    <div className="selection-bar" role="toolbar" aria-label="Selection actions">
      <div className="selection-row">
        <button className="icon-button" onClick={onClear} title="Clear selection (Esc)">
          <Icon name="close" />
        </button>
        <strong className="selection-count">{count.toLocaleString()} selected</strong>
        <button className="link small" onClick={props.onSelectAll} title="Select all (Ctrl+A)">
          Select all
        </button>
        <div className="spacer" />
        <button className="icon-button" onClick={() => onFavorite(true)} title="Favorite">
          <Icon name="heart" filled className="fav-icon" />
        </button>
        <button className="icon-button" onClick={() => onFavorite(false)} title="Unfavorite">
          <Icon name="heart" />
        </button>
        <div className="stars" role="group" aria-label="Rate selection">
          {[1, 2, 3, 4, 5].map((n) => (
            <button key={n} className="icon-button star" onClick={() => onRate(n)} title={`Rate ${n}`}>
              <Icon name="star" filled />
            </button>
          ))}
          <button className="icon-button" onClick={() => onRate(null)} title="Clear rating">
            <Icon name="close" size={14} />
          </button>
        </div>
        <span className="divider" />
        <button className="tool" onClick={() => onHide(!hidden)}>
          <Icon name={hidden ? "eye" : "eyeOff"} />
          {hidden ? "Unhide" : "Hide"}
        </button>
        <button className="tool danger" onClick={props.onRemove} title="Move to the trash folder (Delete)">
          <Icon name="trash" />
          Remove
        </button>
      </div>

      <div className="selection-row tools">
        <div className="tool-group" role="group" aria-label="Edit">
          <button className="icon-button" onClick={() => props.onRotate(-1)} title="Rotate left">
            <Icon name="rotateLeft" />
          </button>
          <button className="icon-button" onClick={() => props.onRotate(1)} title="Rotate right">
            <Icon name="rotateRight" />
          </button>
          {copied && (
            <Popover label={<><Icon name="paste" />Paste edit</>} buttonClass="tool">
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
          <Popover label={<><Icon name="clock" />Fix date</>} buttonClass="tool">
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
          <Popover label={<><Icon name="pin" />Location</>} buttonClass="tool">
            {(close) => (
              <LocationPanel
                count={count}
                onSet={(lat, lon) => {
                  props.onSetLocation(lat, lon);
                  close();
                }}
                onPickOnMap={() => {
                  props.onPlaceOnMap();
                  close();
                }}
                onClear={() => {
                  props.onClearLocation();
                  close();
                }}
              />
            )}
          </Popover>
        </div>

        <div className="tool-group" role="group" aria-label="Organize">
          <Popover label={<><Icon name="album" />Add to album</>} buttonClass="tool">
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
          <Popover label={<><Icon name="calendar" />Event</>} buttonClass="tool">
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
          <Popover label={<><Icon name="tag" />Tag</>} buttonClass="tool">
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
          <Popover label={<><Icon name="rename" />Rename</>} buttonClass="tool">
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
        </div>

        <div className="tool-group" role="group" aria-label="Create and share">
          {count >= 2 && (
            <button className="tool" onClick={props.onSlideshow} title="Play as a slideshow">
              <Icon name="play" />
              Slideshow
            </button>
          )}
          {count >= 2 && count <= 9 && (
            <button className="tool" onClick={props.onCollage} title="Arrange these photos on one page">
              <Icon name="collage" />
              Collage
            </button>
          )}
          <Popover label={<><Icon name="export" />Export</>} buttonClass="tool" align="right">
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
        </div>

        {contextual && (
          <div className="tool-group" role="group" aria-label="This collection">
            {props.onRemoveFromAlbum && (
              <button className="tool" onClick={props.onRemoveFromAlbum}>
                Remove from album
              </button>
            )}
            {props.onSetCover && count === 1 && (
              <button className="tool" onClick={props.onSetCover}>
                Set as cover
              </button>
            )}
            {props.onNotThisPerson && (
              <button className="tool" onClick={props.onNotThisPerson}>
                Not this person
              </button>
            )}
            {props.onPersonCover && count === 1 && (
              <button className="tool" onClick={props.onPersonCover}>
                Use as their picture
              </button>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
