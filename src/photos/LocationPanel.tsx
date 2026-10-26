import { CitySearch } from "../map/MapPage";

type Props = {
  count: number;
  onSet: (lat: number, lon: number) => void;
  onPickOnMap: () => void;
  onClear: () => void;
};

/** Set where the selection was taken: a town from the list, or a spot on the map. */
export function LocationPanel({ count, onSet, onPickOnMap, onClear }: Props) {
  return (
    <div className="paste-panel">
      <CitySearch placeholder="Town" onPick={(c) => onSet(c.lat, c.lon)} />
      <button onClick={onPickOnMap}>Pick on the map…</button>
      <button onClick={onClear}>Remove location</button>
      <p className="muted small">
        Sets the location of {count === 1 ? "this item" : `these ${count.toLocaleString()} items`} here, over what
        their files say. The files aren't changed.
      </p>
    </div>
  );
}
