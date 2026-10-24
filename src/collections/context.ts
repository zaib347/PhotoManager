import { createContext, useContext } from "react";
import type { AlbumInfo, Filter, TagInfo, View } from "../api";

export type Collections = {
  albums: AlbumInfo[];
  tags: TagInfo[];
  /** Refetch album and tag lists (names, counts, covers). */
  reload: () => Promise<void>;
  navigate: (view: View) => void;
  /** Show every photo from one day. */
  showDay: (day: number) => void;
  /** Show every photo from some towns or countries. */
  showPlace: (where: Pick<Filter, "placeIds" | "countries">) => void;
};

export const CollectionsContext = createContext<Collections>({
  albums: [],
  tags: [],
  reload: async () => {},
  navigate: () => {},
  showDay: () => {},
  showPlace: () => {},
});

export const useCollections = () => useContext(CollectionsContext);
