import { createContext, useContext } from "react";
import type { AlbumInfo, TagInfo, View } from "../api";

export type Collections = {
  albums: AlbumInfo[];
  tags: TagInfo[];
  /** Refetch album and tag lists (names, counts, covers). */
  reload: () => Promise<void>;
  navigate: (view: View) => void;
  /** Show every photo from one day. */
  showDay: (day: number) => void;
};

export const CollectionsContext = createContext<Collections>({
  albums: [],
  tags: [],
  reload: async () => {},
  navigate: () => {},
  showDay: () => {},
});

export const useCollections = () => useContext(CollectionsContext);
