import { open, save } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useRef, useState } from "react";
import { api, mediaUrl } from "../api";

const SPEEDS = [3, 4, 6, 8];
const TRANSITION_MS = 1000;
/** Controls hide after the mouse rests this long. */
const IDLE_MS = 2500;

/** The camera moves of the video, matched here: zoom in or out, drifting. */
const MOVES: [string, string][] = [
  ["scale(1) translate(2%, 0)", "scale(1.12) translate(-2%, 0)"],
  ["scale(1.12) translate(0, 2%)", "scale(1) translate(0, -1.5%)"],
  ["scale(1) translate(-2%, -1%)", "scale(1.12) translate(2%, 1%)"],
  ["scale(1.12)", "scale(1)"],
];

type Props = {
  /** Photos in playing order (from `slideshowPick`). */
  ids: number[];
  title: string;
  onClose: () => void;
  onNotice: (message: string) => void;
  onError: (e: unknown) => void;
};

/** One photo on screen, moving slowly; shown whole over a blur if it's a portrait. */
function Slide({ id, index, seconds, visible }: { id: number; index: number; seconds: number; visible: boolean }) {
  const [moved, setMoved] = useState(false);
  const [whole, setWhole] = useState(false);
  const [from, to] = MOVES[index % MOVES.length];

  useEffect(() => {
    setMoved(false);
    // Start the move on the next frame, so the transition runs.
    const frame = requestAnimationFrame(() => requestAnimationFrame(() => setMoved(true)));
    return () => cancelAnimationFrame(frame);
  }, [id, index]);

  const motion = {
    transform: moved ? (whole ? "scale(1.04)" : to) : whole ? "scale(0.98)" : from,
    transition: `transform ${seconds + TRANSITION_MS / 1000}s linear`,
  };
  const src = mediaUrl(id);
  return (
    <div className={`slide${visible ? " visible" : ""}`}>
      {whole && <img className="slide-backdrop" src={src} alt="" />}
      <img
        className={whole ? "slide-photo whole" : "slide-photo"}
        src={src}
        alt=""
        style={motion}
        onLoad={(e) => {
          const img = e.currentTarget;
          const aspect = img.naturalWidth / img.naturalHeight;
          const frame = window.innerWidth / window.innerHeight;
          setWhole(aspect < frame * 0.72 || aspect > frame * 1.6);
        }}
      />
    </div>
  );
}

/**
 * Full-screen memory slideshow with gentle pan and zoom, optional music from the
 * user's own file, and saving the same as an MP4.
 */
export function Slideshow({ ids, title, onClose, onNotice, onError }: Props) {
  const [index, setIndex] = useState(0);
  const [playing, setPlaying] = useState(true);
  const [seconds, setSeconds] = useState(4);
  const [music, setMusic] = useState<{ path: string; url: string } | null>(null);
  const [idle, setIdle] = useState(false);
  const audio = useRef<HTMLAudioElement>(null);
  const idleTimer = useRef<number | undefined>(undefined);

  const go = useCallback((delta: number) => setIndex((i) => (i + delta + ids.length) % ids.length), [ids.length]);

  // Advance while playing; stop after the last photo.
  useEffect(() => {
    if (!playing) return;
    const timer = window.setTimeout(() => {
      if (index + 1 >= ids.length) setPlaying(false);
      else go(1);
    }, seconds * 1000);
    return () => window.clearTimeout(timer);
  }, [playing, index, seconds, ids.length, go]);

  // Warm up the next photo.
  useEffect(() => {
    const next = ids[(index + 1) % ids.length];
    if (next !== undefined) new Image().src = mediaUrl(next);
  }, [index, ids]);

  useEffect(() => {
    const a = audio.current;
    if (!a) return;
    if (playing) a.play().catch(() => {});
    else a.pause();
  }, [playing, music]);

  useEffect(() => {
    if (!music) return;
    return () => URL.revokeObjectURL(music.url);
  }, [music]);

  const close = useCallback(() => {
    if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
    onClose();
  }, [onClose]);

  useEffect(() => {
    document.documentElement.requestFullscreen?.().catch(() => {});
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
      else if (e.key === " ") setPlaying((p) => !p);
      else if (e.key === "ArrowRight") go(1);
      else if (e.key === "ArrowLeft") go(-1);
      else return;
      e.preventDefault();
      e.stopPropagation();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [close, go]);

  const wake = () => {
    setIdle(false);
    window.clearTimeout(idleTimer.current);
    idleTimer.current = window.setTimeout(() => setIdle(true), IDLE_MS);
  };

  const pickMusic = async () => {
    const path = await open({
      title: "Music for the slideshow",
      multiple: false,
      filters: [{ name: "Music", extensions: ["mp3", "m4a", "aac", "wav", "flac", "ogg", "wma"] }],
    });
    if (typeof path !== "string") return;
    try {
      const bytes = await api.readAudio(path);
      setMusic({ path, url: URL.createObjectURL(new Blob([bytes])) });
    } catch (e) {
      onError(e);
    }
  };

  const saveVideo = async () => {
    setPlaying(false);
    const path = await save({
      title: "Save slideshow as a video",
      defaultPath: `${title || "Memories"}.mp4`,
      filters: [{ name: "MP4 video", extensions: ["mp4"] }],
    });
    if (!path) return;
    try {
      await api.makeVideo(ids, { secondsPerPhoto: seconds, music: music?.path ?? null }, path);
      onNotice(`Making the video of ${ids.length.toLocaleString()} photos in the background…`);
    } catch (e) {
      onError(e);
    }
  };

  // The current photo and the one before it (fading out).
  const shown = [index - 1, index].filter((i) => i >= 0);

  return (
    <div className={`slideshow${idle ? " idle" : ""}`} role="dialog" aria-label="Slideshow" onMouseMove={wake}>
      {shown.map((i) => (
        <Slide key={i} id={ids[i]} index={i} seconds={seconds} visible={i === index} />
      ))}
      {music && <audio ref={audio} src={music.url} loop autoPlay={playing} />}
      <div className="slideshow-bar">
        <span className="slideshow-title">
          {title} · {index + 1} / {ids.length}
        </span>
        <div className="spacer" />
        <button className="ghost" onClick={() => go(-1)} aria-label="Previous (←)">
          ⏮
        </button>
        <button className="ghost" onClick={() => setPlaying((p) => !p)} aria-label={playing ? "Pause (Space)" : "Play (Space)"}>
          {playing ? "⏸" : "▶"}
        </button>
        <button className="ghost" onClick={() => go(1)} aria-label="Next (→)">
          ⏭
        </button>
        <select value={seconds} onChange={(e) => setSeconds(Number(e.target.value))} title="Seconds per photo">
          {SPEEDS.map((s) => (
            <option key={s} value={s}>
              {s} s
            </option>
          ))}
        </select>
        {music ? (
          <button className="ghost" onClick={() => setMusic(null)} title={music.path}>
            ♪ Remove music
          </button>
        ) : (
          <button className="ghost" onClick={pickMusic}>
            ♪ Music…
          </button>
        )}
        <button className="ghost" onClick={saveVideo}>
          Save as video…
        </button>
        <button className="ghost" onClick={close} title="Close (Esc)">
          ✕
        </button>
      </div>
    </div>
  );
}
