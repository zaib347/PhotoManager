import { openPath } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useState, useSyncExternalStore } from "react";
import { api, liveUrl, MediaKind, mediaUrl, thumbUrl, type AssetDetails, type View } from "../api";
import { formatTakenAt } from "../format";
import type { DayStore, Position } from "../timeline/dayStore";
import { InfoPanel } from "./InfoPanel";
import { ZoomableImage } from "./ZoomableImage";

const INFO_KEY = "viewer.showInfo";

type Props = {
  store: DayStore;
  start: Position;
  onClose: (last: Position) => void;
  onFavorite: (ids: number[], favorite: boolean) => Promise<void>;
  onHidden: (ids: number[], hidden: boolean) => Promise<void>;
  onAddTag: (ids: number[], name: string) => Promise<void>;
  onRemoveTag: (ids: number[], tagId: number) => Promise<void>;
  onNavigate: (view: View) => void;
};

function readShowInfo() {
  try {
    return localStorage.getItem(INFO_KEY) === "1";
  } catch {
    return false;
  }
}

/** Full-window viewer that steps through the same order as the timeline grid. */
export function Viewer({ store, start, onClose, onFavorite, onHidden, onAddTag, onRemoveTag, onNavigate }: Props) {
  useSyncExternalStore(store.subscribe, store.getVersion);
  const [pos, setPos] = useState(start);
  const [asset, setAsset] = useState<AssetDetails | null>(null);
  const [failed, setFailed] = useState(false);
  const [showInfo, setShowInfo] = useState(readShowInfo);
  const [playingLive, setPlayingLive] = useState(false);
  const id = store.idAt(pos);

  const [detailsVersion, setDetailsVersion] = useState(0);
  const refreshDetails = useCallback(() => setDetailsVersion((v) => v + 1), []);

  useEffect(() => {
    setFailed(false);
    setAsset(null);
    setPlayingLive(false);
  }, [id]);

  useEffect(() => {
    if (id === undefined) return;
    let current = true;
    api.asset(id).then((a) => current && setAsset(a));
    return () => {
      current = false;
    };
  }, [id, detailsVersion]);

  // Warm up neighbours so stepping through photos feels instant.
  useEffect(() => {
    for (const delta of [1, -1] as const) {
      store.step(pos, delta).then((p) => {
        const data = p && store.get(p.day);
        if (p && data?.kinds[p.index] === MediaKind.Photo) new Image().src = mediaUrl(data.ids[p.index]);
      });
    }
  }, [pos, store]);

  const go = useCallback(
    async (delta: 1 | -1) => {
      const next = await store.step(pos, delta);
      if (next) setPos(next);
    },
    [pos, store],
  );

  const toggleInfo = useCallback(() => {
    setShowInfo((v) => {
      try {
        localStorage.setItem(INFO_KEY, v ? "0" : "1");
      } catch {
        // Preference just won't persist.
      }
      return !v;
    });
  }, []);

  const toggleFavorite = useCallback(async () => {
    if (!asset) return;
    const favorite = !asset.favorite;
    setAsset({ ...asset, favorite });
    await onFavorite([asset.id], favorite);
  }, [asset, onFavorite]);

  const setRating = useCallback(
    async (rating: number | null) => {
      if (!asset) return;
      const next = rating === asset.rating ? null : rating;
      setAsset({ ...asset, rating: next });
      await api.setRating([asset.id], next);
    },
    [asset],
  );

  const toggleHidden = useCallback(async () => {
    if (!asset) return;
    await onHidden([asset.id], !asset.hidden);
    // The item leaves this view, so the same index now holds the next one.
    await store.ensure([pos.day]);
    const ids = store.get(pos.day)?.ids ?? [];
    if (pos.index < ids.length) setPos({ ...pos });
    else if (ids.length > 0) setPos({ day: pos.day, index: ids.length - 1 });
    else {
      const next = (await store.step(pos, 1)) ?? (await store.step(pos, -1));
      if (next) setPos(next);
      else onClose(pos);
    }
  }, [asset, onHidden, store, pos, onClose]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      // Typing a tag name must not trigger shortcuts.
      if (e.target instanceof HTMLInputElement && e.key !== "Escape") return;
      if (e.key === "Escape") onClose(pos);
      else if (e.key === "ArrowRight") go(1);
      else if (e.key === "ArrowLeft") go(-1);
      else if (e.key === "i") toggleInfo();
      else if (e.key === "f") toggleFavorite();
      else if (e.key === "l") setPlayingLive((p) => !p);
      else if (/^[0-5]$/.test(e.key)) setRating(e.key === "0" ? null : Number(e.key));
      else return;
      e.preventDefault();
      e.stopPropagation();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [pos, go, onClose, toggleInfo, toggleFavorite, setRating]);

  const isVideo = asset?.mediaKind === MediaKind.Video;
  const onlineFile = asset?.files.find((f) => f.online);

  return (
    <div className="viewer" role="dialog" aria-label="Photo viewer">
      <header className="viewer-bar">
        <button className="ghost" onClick={() => onClose(pos)} title="Back (Esc)">
          ← Back
        </button>
        <span className="viewer-title">{asset ? formatTakenAt(asset.takenAt, asset.tzOffsetMin) : ""}</span>
        <div className="spacer" />
        {asset && (
          <>
            {asset.liveClipId !== null && (
              <button
                className={`ghost${playingLive ? " active" : ""}`}
                onClick={() => setPlayingLive((p) => !p)}
                title="Play Live Photo (L)"
              >
                ◉ Live
              </button>
            )}
            <div className="stars" role="radiogroup" aria-label="Rating">
              {[1, 2, 3, 4, 5].map((n) => (
                <button
                  key={n}
                  className={`ghost star${asset.rating && n <= asset.rating ? " on" : ""}`}
                  onClick={() => setRating(n)}
                  title={`${n} star${n > 1 ? "s" : ""} (${n})`}
                >
                  ★
                </button>
              ))}
            </div>
            <button
              className={`ghost${asset.favorite ? " fav-on" : ""}`}
              onClick={toggleFavorite}
              title="Favorite (F)"
            >
              {asset.favorite ? "♥" : "♡"}
            </button>
            <button className="ghost" onClick={toggleHidden}>
              {asset.hidden ? "Unhide" : "Hide"}
            </button>
          </>
        )}
        <button className={`ghost${showInfo ? " active" : ""}`} onClick={toggleInfo} title="Info (I)">
          Info
        </button>
      </header>

      <div className="viewer-body">
        <div className="viewer-stage">
          {id !== undefined &&
            (failed ? (
              <div className="viewer-message">
                <img src={thumbUrl(id)} alt="" className="viewer-fallback" />
                <p>
                  {!onlineFile
                    ? "The drive with this file isn't connected."
                    : isVideo
                      ? "This video format can't be played here yet."
                      : "This photo format can't be previewed yet."}
                </p>
                {onlineFile && (
                  <button onClick={() => openPath(onlineFile.path)}>Open in default app</button>
                )}
              </div>
            ) : isVideo ? (
              <video
                key={id}
                className="viewer-video"
                src={mediaUrl(id)}
                poster={thumbUrl(id)}
                controls
                autoPlay
                onError={() => setFailed(true)}
              />
            ) : (
              <ZoomableImage key={id} placeholder={thumbUrl(id)} src={mediaUrl(id)} onError={() => setFailed(true)} />
            ))}
          {id !== undefined && playingLive && (
            <video
              className="viewer-video live-overlay"
              src={liveUrl(id)}
              autoPlay
              onEnded={() => setPlayingLive(false)}
              onError={() => setPlayingLive(false)}
            />
          )}
          <button className="nav prev" onClick={() => go(-1)} aria-label="Previous (←)">
            ‹
          </button>
          <button className="nav next" onClick={() => go(1)} aria-label="Next (→)">
            ›
          </button>
        </div>
        {showInfo && (
          <InfoPanel
            asset={asset}
            onAddTag={(name) => asset && onAddTag([asset.id], name).then(refreshDetails)}
            onRemoveTag={(tagId) => asset && onRemoveTag([asset.id], tagId).then(refreshDetails)}
            onNavigate={onNavigate}
          />
        )}
      </div>
    </div>
  );
}
