import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** "events" and "trips" are overview pages; the others are timelines. */
export type BuiltinView = "all" | "favorites" | "videos" | "hidden" | "events" | "trips";
export type View = BuiltinView | { album: number } | { tag: number } | { event: number } | { trip: number };
/** Views that are pages of cards rather than timelines. */
export type PageView = "events" | "trips";
export const isPage = (view: View): view is PageView => view === "events" || view === "trips";
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

export type DayMemory ={ day: number; yearsAgo: number; count: number; coverIds: number[] };

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
};

export const thumbUrl = (assetId: number) => convertFileSrc(String(assetId), "thumb");
export const mediaUrl = (assetId: number) => convertFileSrc(String(assetId), "media");
/** The motion clip of a Live Photo. */
export const liveUrl = (assetId: number) => `${mediaUrl(assetId)}?live`;
