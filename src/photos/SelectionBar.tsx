import type { View } from "../api";

type Props = {
  count: number;
  view: View;
  onClear: () => void;
  onFavorite: (favorite: boolean) => void;
  onRate: (rating: number | null) => void;
  onHide: (hidden: boolean) => void;
};

export function SelectionBar({ count, view, onClear, onFavorite, onRate, onHide }: Props) {
  return (
    <div className="selection-bar" role="toolbar" aria-label="Selection actions">
      <button className="ghost" onClick={onClear} title="Clear selection (Esc)">
        ✕
      </button>
      <strong>{count.toLocaleString()} selected</strong>
      <div className="spacer" />
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
