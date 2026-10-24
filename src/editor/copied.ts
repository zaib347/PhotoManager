import type { Recipe } from "../api";

/**
 * The edit copied with "Copy edit" in the editor or viewer, ready to paste onto a
 * selection. Kept in memory only, until the app closes or another edit is copied.
 */
let copied: Recipe | null = null;
const listeners = new Set<() => void>();

export const copiedEdit = {
  get: () => copied,
  set(recipe: Recipe) {
    copied = recipe;
    listeners.forEach((l) => l());
  },
  subscribe(listener: () => void) {
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  },
};

/** True when the edit crops or straightens, so pasting "light and color only" differs. */
export const hasGeometry = (r: Recipe) => r.crop !== null || Math.abs(r.straighten) > 0.01;
