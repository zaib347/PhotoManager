import type { Recipe } from "../api";
import { hasGeometry } from "../editor/copied";

type Props = {
  count: number;
  copied: Recipe;
  onPaste: (what: "everything" | "tone") => void;
};

/** Paste a copied edit onto the selection: all of it, or light and color only. */
export function PastePanel({ count, copied, onPaste }: Props) {
  const photos = `${count.toLocaleString()} ${count === 1 ? "photo" : "photos"}`;
  const geometry = hasGeometry(copied);
  return (
    <div className="paste-panel">
      {geometry && (
        <>
          <button className="primary" onClick={() => onPaste("tone")}>
            Light and color only
          </button>
          <p className="muted small">Each photo keeps its own crop and straightening.</p>
        </>
      )}
      <button className={geometry ? "" : "primary"} onClick={() => onPaste("everything")}>
        {geometry ? "Everything, with crop and straightening" : `Paste onto ${photos}`}
      </button>
      <p className="muted small">
        Replaces any edits the {photos} already have; each can be reverted on its own. Videos are skipped.
      </p>
    </div>
  );
}
