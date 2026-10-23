import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type BuiltinView = "all" | "favorites" | "videos" | "hidden";
export type View = BuiltinView | { album: number } | { tag: number };

/** Stable string identity for a view (React keys, comparisons). */
export const viewKey = (view: View) =>
  typeof view === "string" ? view : "album" in view ? `album:${view.album}` : `tag:${view.tag}`;

export type Named = { id: number; name: string };

export type AlbumInfo = {
  id: number;
  name: string;
  count: number;
  coverAssetId: number | null;
  createdAt: number;
  updatedAt: number;
};

export type TagInfo = { id: number; name: string; count: number };

/** Every day that has assets, newest first (parallel arrays). */
export type Layout = { days: number[]; counts: number[] };

/** Assets of one day in display order (parallel arrays). */
export type DayAssets = {
  day: number;
  ids: number[];
  kinds: number[];
  favorites: number[];
  durations: (number | null)[];
};

export type Stats = { assets: number; days: number };

export type RootInfo = {
  id: number;
  path: string;
  volumeLabel: string | null;
  online: boolean;
  lastScanAt: number | null;
  files: number;
};

export type FileInfo = {
  id: number;
  path: string;
  size: number;
  modifiedAt: number;
  online: boolean;
  missing: boolean;
};

export type AssetDetails = {
  id: number;
  mediaKind: number;
  takenAt: number;
  /** 0 EXIF, 1 video metadata, 2 filename, 3 file modified time, 4 user */
  takenSrc: number;
  tzOffsetMin: number;
  localDay: number;
  width: number | null;
  height: number | null;
  orientation: number | null;
  durationMs: number | null;
  cameraMake: string | null;
  cameraModel: string | null;
  lat: number | null;
  lon: number | null;
  favorite: boolean;
  rating: number | null;
  hidden: boolean;
  files: FileInfo[];
  albums: Named[];
  tags: Named[];
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
  layout: (view: View) => invoke<Layout>("get_layout", { view }),
  dayAssets: (view: View, days: number[]) => invoke<DayAssets[]>("get_day_assets", { view, days }),
  asset: (id: number) => invoke<AssetDetails | null>("get_asset", { id }),
  setFavorite: (ids: number[], favorite: boolean) => invoke<void>("set_favorite", { ids, favorite }),
  setRating: (ids: number[], rating: number | null) => invoke<void>("set_rating", { ids, rating }),
  setHidden: (ids: number[], hidden: boolean) => invoke<void>("set_hidden", { ids, hidden }),
  listAlbums: () => invoke<AlbumInfo[]>("list_albums"),
  createAlbum: (name: string, ids: number[]) => invoke<number>("create_album", { name, ids }),
  renameAlbum: (id: number, name: string) => invoke<void>("rename_album", { id, name }),
  deleteAlbum: (id: number) => invoke<void>("delete_album", { id }),
  addToAlbum: (albumId: number, ids: number[]) => invoke<number>("add_to_album", { albumId, ids }),
  removeFromAlbum: (albumId: number, ids: number[]) => invoke<void>("remove_from_album", { albumId, ids }),
  setAlbumCover: (albumId: number, assetId: number) => invoke<void>("set_album_cover", { albumId, assetId }),
  listTags: () => invoke<TagInfo[]>("list_tags"),
  addTag: (ids: number[], name: string) => invoke<number>("add_tag", { ids, name }),
  removeTag: (ids: number[], tagId: number) => invoke<void>("remove_tag", { ids, tagId }),
  renameTag: (id: number, name: string) => invoke<void>("rename_tag", { id, name }),
  deleteTag: (id: number) => invoke<void>("delete_tag", { id }),
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
export const mediaUrl = (assetId: number) => convertFileSrc(String(assetId), "media");
