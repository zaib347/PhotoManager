import { confirm } from "../ui/confirm";
import { useCallback, useEffect, useMemo, useRef, useState, type MouseEvent } from "react";
import { api, isFilterEmpty, type Filter, type Layout, type TimelineView } from "../api";
import { describeReport } from "../cleanup/RecentlyRemoved";
import { AlbumHeader } from "../collections/AlbumHeader";
import { useCollections } from "../collections/context";
import { EventHeader } from "../events/EventHeader";
import { TripHeader } from "../trips/TripHeader";
import { PersonHeader } from "../people/PersonHeader";
import { copiedEdit } from "../editor/copied";
import { photoLooks } from "../looks";
import { DayStore, type Position } from "../timeline/dayStore";
import { Timeline } from "../timeline/Timeline";
import { Viewer } from "../viewer/Viewer";
import { OnThisDay } from "./OnThisDay";
import { CollageEditor } from "../collage/CollageEditor";
import { Slideshow } from "../slideshow/Slideshow";
import { SelectionBar } from "./SelectionBar";

type Props = {
  view: TimelineView;
  filter: Filter;
  layout: Layout;
  /** Reload layout and counts after changes that move items between views. */
  onLibraryChanged: () => Promise<void>;
  onNotice: (message: string) => void;
  onError: (e: unknown) => void;
  /** Opens the map to place these items with a click. */
  onPlaceOnMap: (ids: number[]) => void;
};

/** One timeline view (All, Favorites, …) with selection and the full-screen viewer. */
export function PhotosView({ view, filter, layout, onLibraryChanged, onNotice, onError, onPlaceOnMap }: Props) {
  // `layout` only seeds a new store; later layouts arrive through setLayout below.
  // The parent remounts this component when the view or filter changes.
  const store = useMemo(() => new DayStore(view, filter, layout), [view, filter]);
  const collections = useCollections();
  const albumId = typeof view === "object" && "album" in view ? view.album : null;
  const album = albumId === null ? undefined : collections.albums.find((a) => a.id === albumId);
  const eventId = typeof view === "object" && "event" in view ? view.event : null;
  const tripId = typeof view === "object" && "trip" in view ? view.trip : null;
  const personId = typeof view === "object" && "person" in view ? view.person : null;
  // Bumped after edits that change this event's dates or members, to refresh its header.
  const [eventVersion, setEventVersion] = useState(0);
  const [selection, setSelection] = useState<ReadonlySet<number>>(new Set());
  // Stable array identity, so panels keyed on the selection don't reset while open.
  const selectedIds = useMemo(() => [...selection], [selection]);
  const anchor = useRef<Position | null>(null);
  const [viewing, setViewing] = useState<Position | null>(null);
  const [reveal, setReveal] = useState<Position | null>(null);
  const [collage, setCollage] = useState<number[] | null>(null);
  const [slideshow, setSlideshow] = useState<{ ids: number[]; title: string } | null>(null);

  /** Plays `ids` (near-duplicates left out, best shots if there are many). */
  const play = useCallback(
    (ids: number[], title: string) => {
      api
        .slideshowPick(ids)
        .then((picked) => {
          if (picked.length === 0) onNotice("There are no photos to play here.");
          else setSlideshow({ ids: picked, title });
        })
        .catch(onError);
    },
    [onNotice, onError],
  );

  useEffect(() => store.setLayout(layout), [store, layout]);

  const clearSelection = useCallback(() => {
    setSelection(new Set());
    anchor.current = null;
  }, []);

  const selectAll = useCallback(async () => {
    setSelection(new Set(await api.assetIds(view, filter)));
  }, [view, filter]);

  /** Asks, then moves the photos' files to the trash folder. True if they were removed. */
  const remove = useCallback(
    async (ids: number[]) => {
      const n = ids.length;
      const ok = await confirm(
        `Remove ${n === 1 ? "this photo" : `${n.toLocaleString()} photos`}?\n\n` +
          "Every copy moves to a hidden .photomanager-trash folder next to it. You can put them back from " +
          "Recently removed.",
        { title: "Remove photos", kind: "warning", okLabel: "Remove" },
      );
      if (!ok) return false;
      const report = await api.removePhotos(ids);
      onNotice(`${describeReport(report, "Moved")}. Undo from Recently removed.`);
      await onLibraryChanged();
      return report.done > 0;
    },
    [onLibraryChanged, onNotice],
  );

  useEffect(() => {
    if (viewing) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLInputElement) return;
      if (e.key === "Escape" && selection.size > 0) clearSelection();
      if (e.key === "a" && (e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        selectAll().catch(onError);
      }
      if (e.key === "Delete" && selection.size > 0) {
        e.preventDefault();
        remove([...selection])
          .then((removed) => removed && clearSelection())
          .catch(onError);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [viewing, selection, clearSelection, selectAll, remove, onError]);

  const toggle = (pos: Position, id: number) => {
    setSelection((s) => {
      const next = new Set(s);
      if (!next.delete(id)) next.add(id);
      return next;
    });
    anchor.current = pos;
  };

  /** Adds everything between the anchor and `pos` (inclusive), loading days as needed. */
  const selectRange = async (pos: Position) => {
    const from = anchor.current;
    if (!from) return;
    const a = store.indexOf(from.day);
    const b = store.indexOf(pos.day);
    if (a < 0 || b < 0) return;
    const [start, end] = a < b || (a === b && from.index <= pos.index) ? [from, pos] : [pos, from];
    const days = layout.days.slice(Math.min(a, b), Math.max(a, b) + 1);
    await store.ensure(days);
    const ids: number[] = [];
    for (const day of days) {
      const dayIds = store.get(day)?.ids ?? [];
      const lo = day === start.day ? start.index : 0;
      const hi = day === end.day ? end.index : dayIds.length - 1;
      ids.push(...dayIds.slice(lo, hi + 1));
    }
    setSelection((s) => new Set([...s, ...ids]));
  };

  const onActivate = (e: MouseEvent, pos: Position, id: number) => {
    if (e.shiftKey && anchor.current) selectRange(pos).catch(onError);
    else if (e.ctrlKey || e.metaKey || selection.size > 0) toggle(pos, id);
    else setViewing(pos);
  };

  const toggleDay = (day: number) => {
    const ids = store.get(day)?.ids ?? [];
    setSelection((s) => {
      const next = new Set(s);
      const all = ids.length > 0 && ids.every((id) => next.has(id));
      ids.forEach((id) => (all ? next.delete(id) : next.add(id)));
      return next;
    });
  };

  const setFavorite = useCallback(
    async (ids: number[], favorite: boolean) => {
      store.patchFavorites(new Set(ids), favorite);
      await api.setFavorite(ids, favorite);
      if (view === "favorites") await onLibraryChanged();
    },
    [store, view, onLibraryChanged],
  );

  const setHidden = useCallback(
    async (ids: number[], hidden: boolean) => {
      await api.setHidden(ids, hidden);
      await onLibraryChanged();
    },
    [onLibraryChanged],
  );

  const rotate = useCallback(async (ids: number[], quarters: number) => {
    photoLooks.setTurns(await api.rotatePhotos(ids, quarters));
  }, []);

  const run = (action: () => Promise<void>) => action().catch(onError);

  /** After edits that change which items are in this view (and its event header). */
  const afterTimelineChange = async () => {
    setEventVersion((v) => v + 1);
    await onLibraryChanged();
  };

  /** After album/tag edits: refresh sidebar counts, and this grid if membership changed. */
  const afterCollectionChange = async (affectsThisView: boolean) => {
    await collections.reload();
    if (affectsThisView) await onLibraryChanged();
  };

  const addTag = useCallback(
    async (ids: number[], name: string) => {
      await api.addTag(ids, name);
      await collections.reload();
    },
    [collections],
  );

  const removeTag = useCallback(
    async (ids: number[], tagId: number) => {
      await api.removeTag(ids, tagId);
      await collections.reload();
      if (typeof view === "object" && "tag" in view && view.tag === tagId) await onLibraryChanged();
    },
    [collections, view, onLibraryChanged],
  );

  return (
    <div className="photos-view">
      {view === "all" && isFilterEmpty(filter) && selection.size === 0 && (
        <OnThisDay
          onPlay={(days) =>
            Promise.all(days.map((d) => api.assetIds("all", { fromDay: d, toDay: d })))
              .then((lists) => play(lists.flat(), "On this day"))
              .catch(onError)
          }
        />
      )}
      {album && <AlbumHeader key={`${album.id}:${album.name}`} album={album} onError={onError} />}
      {eventId !== null && (
        <EventHeader
          id={eventId}
          version={eventVersion}
          onError={onError}
          onPlay={() => api.assetIds(view, filter).then((ids) => play(ids, "Event"), onError)}
        />
      )}
      {tripId !== null && (
        <TripHeader
          id={tripId}
          version={eventVersion}
          onError={onError}
          onPlay={() => api.assetIds(view, filter).then((ids) => play(ids, "Trip"), onError)}
        />
      )}
      {personId !== null && (
        <PersonHeader
          id={personId}
          version={eventVersion}
          onError={onError}
          onPlay={() => api.assetIds(view, filter).then((ids) => play(ids, "Person"), onError)}
        />
      )}
      {selection.size > 0 && (
        <SelectionBar
          count={selection.size}
          ids={selectedIds}
          onSelectAll={() => run(selectAll)}
          onShiftDates={(delta) =>
            run(async () => {
              await api.shiftDates([...selection], delta);
              clearSelection();
              await afterTimelineChange();
            })
          }
          onRestoreDates={() =>
            run(async () => {
              await api.restoreDates([...selection]);
              clearSelection();
              await afterTimelineChange();
            })
          }
          view={view}
          onClear={clearSelection}
          onFavorite={(favorite) => run(() => setFavorite([...selection], favorite))}
          onRate={(rating) => run(() => api.setRating([...selection], rating))}
          onHide={(hidden) =>
            run(async () => {
              await setHidden([...selection], hidden);
              clearSelection();
            })
          }
          onRemove={() =>
            run(async () => {
              if (await remove([...selection])) clearSelection();
            })
          }
          onRotate={(quarters) => run(() => rotate([...selection], quarters))}
          onSetLocation={(lat, lon) =>
            run(async () => {
              const n = await api.setLocation([...selection], lat, lon);
              onNotice(`Location set for ${n.toLocaleString()} ${n === 1 ? "item" : "items"}.`);
              await afterTimelineChange();
            })
          }
          onPlaceOnMap={() => onPlaceOnMap([...selection])}
          onClearLocation={() =>
            run(async () => {
              const n = await api.clearLocation([...selection]);
              onNotice(
                n === 0
                  ? "None of these had a location."
                  : `Location removed from ${n.toLocaleString()} ${n === 1 ? "item" : "items"}.`,
              );
              await afterTimelineChange();
            })
          }
          onCollage={() => setCollage([...selection])}
          onSlideshow={() => play([...selection], `${selection.size.toLocaleString()} photos`)}
          onPasteEdit={(what) =>
            run(async () => {
              const recipe = copiedEdit.get();
              if (!recipe) return;
              const pasted = await api.editPaste([...selection], recipe, what);
              photoLooks.setVersions(pasted);
              const skipped = selection.size - pasted.length;
              onNotice(
                `Edit pasted onto ${pasted.length.toLocaleString()} ${pasted.length === 1 ? "photo" : "photos"}` +
                  (skipped > 0 ? ` (${skipped.toLocaleString()} ${skipped === 1 ? "video" : "videos"} skipped).` : "."),
              );
            })
          }
          onRename={(pattern) =>
            run(async () => {
              const report = await api.renameRun([...selection], pattern);
              onNotice(`${describeReport(report, "Renamed")}. Undo from Organize files.`);
              await onLibraryChanged();
            })
          }
          onExport={(options) =>
            run(async () => {
              await api.exportPhotos([...selection], options);
              onNotice(`Exporting ${selection.size.toLocaleString()} photos to ${options.folder}…`);
            })
          }
          onAddToAlbum={(id) =>
            run(async () => {
              await api.addToAlbum(id, [...selection]);
              clearSelection();
              await afterCollectionChange(false);
            })
          }
          onCreateAlbum={(name) =>
            run(async () => {
              const id = await api.createAlbum(name, [...selection]);
              clearSelection();
              await collections.reload();
              collections.navigate({ album: id });
            })
          }
          onCreateEvent={(name) =>
            run(async () => {
              const id = await api.createEvent([...selection], name);
              clearSelection();
              collections.navigate({ event: id });
            })
          }
          onSplitEvent={
            eventId === null
              ? undefined
              : () =>
                  run(async () => {
                    await api.splitEvent(eventId, [...selection][0]);
                    clearSelection();
                    await afterTimelineChange();
                  })
          }
          onTag={(name) =>
            run(async () => {
              await addTag([...selection], name);
              clearSelection();
            })
          }
          onRemoveFromAlbum={
            albumId === null
              ? undefined
              : () =>
                  run(async () => {
                    await api.removeFromAlbum(albumId, [...selection]);
                    clearSelection();
                    await afterCollectionChange(true);
                  })
          }
          onNotThisPerson={
            personId === null
              ? undefined
              : () =>
                  run(async () => {
                    const n = await api.aiRemoveFromPerson(personId, [...selection]);
                    clearSelection();
                    onNotice(
                      n === 0
                        ? "Those photos weren't grouped with this person."
                        : `Taken out: ${n.toLocaleString()} ${n === 1 ? "photo" : "photos"} won't be grouped with this person again.`,
                    );
                    await afterTimelineChange();
                  })
          }
          onPersonCover={
            personId === null
              ? undefined
              : () =>
                  run(async () => {
                    await api.aiPersonCover(personId, [...selection][0]);
                    clearSelection();
                    setEventVersion((v) => v + 1);
                  })
          }
          onSetCover={
            albumId === null
              ? undefined
              : () =>
                  run(async () => {
                    await api.setAlbumCover(albumId, [...selection][0]);
                    clearSelection();
                    await collections.reload();
                  })
          }
        />
      )}
      <Timeline
        store={store}
        selection={selection}
        keyboard={!viewing}
        onActivate={onActivate}
        onToggle={toggle}
        onToggleDay={toggleDay}
        reveal={reveal}
      />
      {viewing && (
        <Viewer
          store={store}
          start={viewing}
          onClose={(last) => {
            setViewing(null);
            setReveal(last);
          }}
          onFavorite={setFavorite}
          onHidden={setHidden}
          onRemove={remove}
          onRotate={rotate}
          onAddTag={addTag}
          onDatesChanged={afterTimelineChange}
          onError={onError}
          onRemoveTag={removeTag}
          onNavigate={(target) => {
            setViewing(null);
            collections.navigate(target);
          }}
        />
      )}
      {slideshow && (
        <Slideshow
          ids={slideshow.ids}
          title={slideshow.title}
          onClose={() => setSlideshow(null)}
          onNotice={onNotice}
          onError={onError}
        />
      )}
      {collage && (
        <CollageEditor ids={collage} onClose={() => setCollage(null)} onNotice={onNotice} onError={onError} />
      )}
    </div>
  );
}
