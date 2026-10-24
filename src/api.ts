import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { turnsQuery } from "./turns";

/** Views that are pages rather than timelines. */
const PAGES = ["events", "trips", "duplicates", "similar", "removed", "organize"] as const;
export type PageView = (typeof PAGES)[number];
export type BuiltinView = "all" | "favorites" | "videos" | "hidden" | PageView;
export type View = BuiltinView | { album: number } | { tag: number } | { event: number } | { trip: number };
export const isPage = (view: View): view is PageView => (PAGES as readonly View[]).includes(view);
/** Views the backend can lay out as a timeline. */
export type TimelineView = Exclude<View, PageView>;

/** Stable string identity for a view (React keys, comparisons). */
export const viewKey = (view: View) =>
  typeof view === "string"
    ? view
    : "album" in view
      ? `album:${view.album}`
      : "tag" in view
        ? `tag:${view.tag}`
        : "trip" in view
          ? `trip:${view.trip}`
          : `event:${view.event}`;

export type Named = { id: number; name: string };

/** Narrows any view. Omitted or empty fields don't filter. */
export type Filter = {
  media?: "photo" | "video";
  /** Camera models; "" means no camera recorded. */
  cameras?: string[];
  rootIds?: number[];
  /** Inclusive local-day range. */
  fromDay?: number;
  toDay?: number;
  minRating?: number;
  /** Towns (GeoNames ids) and countries (ISO codes); matching either is enough. */
  placeIds?: number[];
  countries?: string[];
};

/** A town from the bundled GeoNames list. */
export type Place = {
  id: number;
  name: string;
  region: string | null;
  country: string;
  countryCode: string;
};

export type FilterOptions = {
  cameras: { model: string; count: number }[];
  /** The most common places and every country, most photos first. */
  places: (Place & { count: number })[];
  countries: { code: string; name: string; count: number }[];
  firstDay: number | null;
  lastDay: number | null;
};

/** Drops empty fields so equal filters always produce the same key. */
export function normalizeFilter(f: Filter): Filter {
  const out: Filter = {};
  if (f.media) out.media = f.media;
  if (f.cameras?.length) out.cameras = [...f.cameras].sort();
  if (f.rootIds?.length) out.rootIds = [...f.rootIds].sort((a, b) => a - b);
  if (f.fromDay !== undefined) out.fromDay = f.fromDay;
  if (f.toDay !== undefined) out.toDay = f.toDay;
  if (f.minRating) out.minRating = f.minRating;
  if (f.placeIds?.length) out.placeIds = [...f.placeIds].sort((a, b) => a - b);
  if (f.countries?.length) out.countries = [...f.countries].sort();
  return out;
}

export const filterKey = (f: Filter) => JSON.stringify(normalizeFilter(f));
export const isFilterEmpty = (f: Filter) => filterKey(f) === "{}";

export type AlbumInfo = {
  id: number;
  name: string;
  count: number;
  coverAssetId: number | null;
  createdAt: number;
  updatedAt: number;
};

export type TagInfo = { id: number; name: string; count: number };

export type EventSummary = {
  id: number;
  /** null until named; show the dates instead. */
  title: string | null;
  /** Wall-clock ms of the first and last item. */
  start: number;
  end: number;
  count: number;
  coverId: number | null;
  /** Where most of its photos were taken; the default name when untitled. */
  place: Place | null;
  /** Renamed, merged, split or made by hand; detection leaves it alone. */
  edited: boolean;
};

export type TripSummary = {
  id: number;
  /** null until named; show its places instead. */
  title: string | null;
  /** Local days, inclusive. */
  firstDay: number;
  lastDay: number;
  count: number;
  coverId: number | null;
  /** The most photographed towns, in visit order. */
  places: Place[];
  /** Every town visited, and their countries in visit order. */
  placeCount: number;
  countries: string[];
  /** Renamed; detection leaves its days alone. */
  edited: boolean;
};

export type DuplicateOverview = { groups: number; extraFiles: number; extraBytes: number };

/** A photo stored more than once; the suggested copy to keep is first. */
export type DuplicateGroup = {
  assetId: number;
  mediaKind: number;
  takenAt: number;
  tzOffsetMin: number;
  size: number;
  copies: FileInfo[];
};

/** One removal, under "Recently removed". */
export type FileOp = {
  id: number;
  reason: string;
  createdAt: number;
  status: "running" | "done" | "undoing" | "undone" | "emptied";
  /** Files still in the trash folder (undoable). */
  inTrash: number;
  inTrashBytes: number;
  failed: number;
};

export type SimilarOverview = { groups: number; photos: number; pending: number };

export type SimilarMember = {
  assetId: number;
  mediaKind: number;
  takenAt: number;
  tzOffsetMin: number;
  width: number | null;
  height: number | null;
  bytes: number;
  favorite: boolean;
  rating: number | null;
  /** From the thumbnail. */
  sharpness: number | null;
  /** From the original, once assessed: focus of the sharpest part, and exposure
   * (0 good, towards -1 too dark, towards +1 too bright). */
  focus: number | null;
  exposure: number | null;
  /** The one suggested to keep. */
  best: boolean;
  why: "favorite" | "rated" | "sharpest" | "exposure" | "biggest" | "similar" | null;
  /** Weak points next to the others in the group. */
  notes: ("blurry" | "dark" | "bright")[];
};

export type SimilarGroup = {
  id: number;
  /** "similar": taken close together; "copies": the same picture saved again. */
  kind: "similar" | "copies";
  members: SimilarMember[];
};

export type ExportFormat = "original" | "jpeg" | "webp" | "png";

export type ExportOptions = {
  /** "original" keeps JPEG/PNG/WebP and turns everything else (HEIC…) into JPEG. */
  format: ExportFormat;
  /** 1–100, for JPEG and WebP. */
  quality: number;
  /** Longer edge at most this many pixels; null keeps the full size. */
  maxEdge: number | null;
  removeLocation: boolean;
  /** Drawn on every copy when set. */
  watermark: Watermark | null;
  folder: string;
};

export type WatermarkPosition = "topLeft" | "topRight" | "bottomLeft" | "bottomRight" | "center" | "tiled";

export type Watermark = {
  /** Drawn when there is no logo. */
  text: string;
  /** Path of a logo image, used instead of the text. */
  logo: string | null;
  position: WatermarkPosition;
  /** Height as a percentage of the photo's shorter edge. */
  size: number;
  /** 5–100. */
  opacity: number;
};

export type ExportReport = {
  exported: number;
  failed: number;
  /** Videos, items without files, cancelled ones. */
  skipped: number;
  bytesIn: number;
  bytesOut: number;
  folder: string;
  errors: string[];
};

export type FileOpKind = "trash" | "move";

/** 2023/2023-06, 2023/06 or 2023/2023-06-14. */
export type OrganizeLayout = "yearMonth" | "yearMonthNumber" | "yearDay";

export type OrganizePreview = {
  moves: number;
  inPlace: number;
  /** Left alone: their only date is the file's modified time. */
  undated: number;
  /** Need " (2)" because the name is taken there. */
  renamed: number;
  sample: { fileId: number; from: string; to: string }[];
};

export type FileOpReport = { opId: number; done: number; bytes: number; failed: number; errors: string[] };

export type DayMemory = { day: number; yearsAgo: number; count: number; coverIds: number[] };

/** Wall-clock ms (the time as shown, in each photo's own zone). */
export type WallRange = { first: number; last: number };

/** Today's date on this computer, as a local day number. */
export const todayLocalDay = () =>
  Math.floor((Date.now() - new Date().getTimezoneOffset() * 60_000) / 86_400_000);

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
  /** The town nearest the GPS position, once looked up. */
  place: Place | null;
  favorite: boolean;
  rating: number | null;
  hidden: boolean;
  files: FileInfo[];
  albums: Named[];
  tags: Named[];
  /** The clip of a Live Photo. */
  liveClipId: number | null;
  event: EventSummary | null;
  /** The trip this item's day belongs to. */
  trip: TripSummary | null;
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
  layout: (view: TimelineView, filter: Filter) => invoke<Layout>("get_layout", { view, filter }),
  dayAssets: (view: TimelineView, filter: Filter, days: number[]) =>
    invoke<DayAssets[]>("get_day_assets", { view, filter, days }),
  filterOptions: () => invoke<FilterOptions>("filter_options"),
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
  onThisDay: (today: number) => invoke<DayMemory[]>("on_this_day", { today }),
  dateRange: (ids: number[]) => invoke<WallRange | null>("date_range", { ids }),
  shiftDates: (ids: number[], deltaMs: number) => invoke<number>("shift_dates", { ids, deltaMs }),
  restoreDates: (ids: number[]) => invoke<number>("restore_dates", { ids }),
  assetIds: (view: TimelineView, filter: Filter) => invoke<number[]>("asset_ids", { view, filter }),
  listEvents: () => invoke<EventSummary[]>("list_events"),
  event: (id: number) => invoke<EventSummary | null>("get_event", { id }),
  renameEvent: (id: number, title: string) => invoke<void>("rename_event", { id, title }),
  mergeEvents: (ids: number[]) => invoke<number>("merge_events", { ids }),
  splitEvent: (id: number, atAsset: number) => invoke<number>("split_event", { id, atAsset }),
  createEvent: (ids: number[], title: string) => invoke<number>("create_event", { ids, title }),
  removeEvent: (id: number) => invoke<void>("remove_event", { id }),
  listTrips: () => invoke<TripSummary[]>("list_trips"),
  trip: (id: number) => invoke<TripSummary | null>("get_trip", { id }),
  renameTrip: (id: number, title: string) => invoke<void>("rename_trip", { id, title }),
  removeTrip: (id: number) => invoke<void>("remove_trip", { id }),
  duplicateOverview: () => invoke<DuplicateOverview>("duplicate_overview"),
  duplicateGroups: (offset: number, limit: number) =>
    invoke<DuplicateGroup[]>("duplicate_groups", { offset, limit }),
  /** `keep`: asset id → file id to keep instead of the suggestion; `only`: just these photos. */
  removeDuplicates: (keep: Record<number, number>, only?: number[]) =>
    invoke<FileOpReport>("remove_duplicates", { keep, only: only ?? null }),
  similarOverview: () => invoke<SimilarOverview>("similar_overview"),
  similarGroups: (offset: number, limit: number) => invoke<SimilarGroup[]>("similar_groups", { offset, limit }),
  /** Moves every file of these photos to the trash folder (undoable). */
  removeSimilar: (ids: number[]) => invoke<FileOpReport>("remove_similar", { ids }),
  keepSimilar: (ids: number[]) => invoke<void>("keep_similar", { ids }),
  /** Moves every file of these photos to the trash folder next to it (undoable). */
  removePhotos: (ids: number[]) => invoke<FileOpReport>("remove_photos", { ids }),
  /** Starts a background export; watch `events.exportProgress` / `exportFinished`. */
  exportPhotos: (ids: number[], options: ExportOptions) => invoke<void>("export_photos", { ids, options }),
  cancelExport: () => invoke<void>("cancel_export"),
  /** Turns photos without touching their files: 1 clockwise, -1 counter-clockwise. */
  rotatePhotos: (ids: number[], quarters: number) => invoke<[number, number][]>("rotate_photos", { ids, quarters }),
  /** Every turned photo with its quarter turns. */
  turnedPhotos: () => invoke<[number, number][]>("turned_photos"),
  /** Photo `id` with the watermark drawn on it, as a JPEG. */
  watermarkPreview: async (id: number, watermark: Watermark) =>
    new Blob([await invoke<ArrayBuffer>("watermark_preview", { id, watermark })], { type: "image/jpeg" }),
  /** Recent removals ("trash") or organizer moves ("move"). */
  listFileOps: (kind: FileOpKind = "trash") => invoke<FileOp[]>("list_file_ops", { kind }),
  organizePreview: (rootId: number, layout: OrganizeLayout, includeUndated: boolean) =>
    invoke<OrganizePreview>("organize_preview", { rootId, layout, includeUndated }),
  organizeRun: (rootId: number, layout: OrganizeLayout, includeUndated: boolean) =>
    invoke<FileOpReport>("organize_run", { rootId, layout, includeUndated }),
  undoFileOp: (id: number) => invoke<FileOpReport>("undo_file_op", { id }),
  emptyFileOp: (id: number) => invoke<FileOpReport>("empty_file_op", { id }),
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
  /** Events were (re)detected in the background. */
  eventsChanged: (cb: () => void): Promise<UnlistenFn> => listen("events-changed", () => cb()),
  exportProgress: (cb: (p: { done: number; total: number }) => void): Promise<UnlistenFn> =>
    listen<{ done: number; total: number }>("export-progress", (e) => cb(e.payload)),
  exportFinished: (cb: (f: { report: ExportReport | null; error: string | null }) => void): Promise<UnlistenFn> =>
    listen<{ report: ExportReport | null; error: string | null }>("export-finished", (e) => cb(e.payload)),
  /** Background hashing finished a pass; similar groups may have changed. */
  similarChanged: (cb: () => void): Promise<UnlistenFn> => listen("similar-changed", () => cb()),
};

export const thumbUrl = (assetId: number) => convertFileSrc(String(assetId), "thumb") + turnsQuery(assetId);
export const mediaUrl = (assetId: number) => convertFileSrc(String(assetId), "media") + turnsQuery(assetId);
/** The motion clip of a Live Photo (never turned). */
export const liveUrl = (assetId: number) => `${convertFileSrc(String(assetId), "media")}?live`;
