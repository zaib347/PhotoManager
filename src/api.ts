import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** Every day that has assets, newest first (parallel arrays). */
export type Layout = { days: number[]; counts: number[] };
export type DayAssets = { day: number; ids: number[]; kinds: number[] };
export type Stats = { assets: number; days: number };

export type RootInfo = {
  id: number;
  path: string;
  volumeLabel: string | null;
  online: boolean;
  lastScanAt: number | null;
  files: number;
};

export type ScanProgress = {
  rootId: number;
  phase: "discovering" | "processing" | "finalizing";
  discovered: number;
  toProcess: number;
  processed: number;
  errors: number;
};

export type ScanSummary = {
  discovered: number;
  added: number;
  updated: number;
  unchanged: number;
  missing: number;
  errors: number;
  completed: boolean;
  missingCheckSkipped: boolean;
};

export type ScanFinished = { rootId: number; summary: ScanSummary | null; error: string | null };

export const MediaKind = { Photo: 0, Video: 1, LivePhoto: 2 } as const;

export const api = {
  stats: () => invoke<Stats>("library_stats"),
  layout: () => invoke<Layout>("get_layout"),
  dayAssets: (days: number[]) => invoke<DayAssets[]>("get_day_assets", { days }),
  listRoots: () => invoke<RootInfo[]>("list_roots"),
  scanningRoots: () => invoke<number[]>("scanning_roots"),
  addRoot: (path: string) => invoke<number>("add_root", { path }),
  removeRoot: (rootId: number) => invoke<void>("remove_root", { rootId }),
  scanRoot: (rootId: number) => invoke<boolean>("scan_root", { rootId }),
  cancelScan: (rootId: number) => invoke<void>("cancel_scan", { rootId }),
  generateSynthetic: (count: number) => invoke<number>("generate_synthetic", { count }),
  clear: () => invoke<void>("clear_library"),
};

export const events = {
  scanProgress: (cb: (p: ScanProgress) => void): Promise<UnlistenFn> =>
    listen<ScanProgress>("scan-progress", (e) => cb(e.payload)),
  scanFinished: (cb: (f: ScanFinished) => void): Promise<UnlistenFn> =>
    listen<ScanFinished>("scan-finished", (e) => cb(e.payload)),
  volumesChanged: (cb: () => void): Promise<UnlistenFn> => listen("volumes-changed", () => cb()),
};

export const thumbUrl = (assetId: number) => convertFileSrc(String(assetId), "thumb");
