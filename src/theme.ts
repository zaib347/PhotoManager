import { getCurrentWindow } from "@tauri-apps/api/window";

/**
 * The app's color theme. "system" follows Windows' light/dark setting; the others are
 * fixed palettes defined in App.css under `:root[data-theme="…"]`.
 */
export type ThemeId = "system" | "light" | "dark" | "nord" | "dracula" | "atom" | "solarized";

export type ThemeInfo = {
  id: ThemeId;
  label: string;
  dark: boolean | null;
  /** Background, surface, accent and text, for the picker's swatches. */
  swatch: [string, string, string, string];
};

export const THEMES: ThemeInfo[] = [
  { id: "system", label: "System", dark: null, swatch: ["#f5f5f7", "#1f1f21", "#2f6fde", "#6b9cff"] },
  { id: "light", label: "Light", dark: false, swatch: ["#f5f5f7", "#ffffff", "#2f6fde", "#1c1c1e"] },
  { id: "dark", label: "Dark", dark: true, swatch: ["#161617", "#1f1f21", "#6b9cff", "#ececee"] },
  { id: "nord", label: "Nord", dark: true, swatch: ["#2e3440", "#3b4252", "#88c0d0", "#eceff4"] },
  { id: "dracula", label: "Dracula", dark: true, swatch: ["#282a36", "#2e3140", "#bd93f9", "#f8f8f2"] },
  { id: "atom", label: "Atom One Dark", dark: true, swatch: ["#21252b", "#282c34", "#61afef", "#d7dae0"] },
  { id: "solarized", label: "Solarized Light", dark: false, swatch: ["#eee8d5", "#fdf6e3", "#268bd2", "#073642"] },
];

const KEY = "theme";
const listeners = new Set<() => void>();
// Bumped on every change, including Windows switching light/dark under "system".
let revision = 0;
const changed = () => {
  revision++;
  listeners.forEach((l) => l());
};

function load(): ThemeId {
  try {
    const saved = localStorage.getItem(KEY);
    if (THEMES.some((t) => t.id === saved)) return saved as ThemeId;
  } catch {
    // Storage may be unavailable; fall back to the system theme.
  }
  return "system";
}

let current: ThemeId = load();

function apply(id: ThemeId) {
  const root = document.documentElement;
  if (id === "system") delete root.dataset.theme;
  else root.dataset.theme = id;
  // The title bar follows: dark themes get a dark one, "system" lets Windows decide.
  const dark = THEMES.find((t) => t.id === id)?.dark ?? null;
  try {
    getCurrentWindow()
      .setTheme(dark === null ? null : dark ? "dark" : "light")
      .catch(() => {});
  } catch {
    // Not inside the app window (a plain browser): the page colors still apply.
  }
}

apply(current);

// Pages that paint with theme colors (the map) redraw when Windows switches light/dark.
window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
  if (current === "system") changed();
});

export const theme = {
  get: () => current,
  set(id: ThemeId) {
    current = id;
    apply(id);
    try {
      localStorage.setItem(KEY, id);
    } catch {
      // Remembering is a convenience only.
    }
    changed();
  },
  /** Changes whenever the colors on screen may have changed. */
  revision: () => revision,
  subscribe(listener: () => void) {
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  },
};
