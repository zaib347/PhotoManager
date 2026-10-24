import { useEffect, useState } from "react";
import { api, thumbUrl, todayLocalDay, type DayMemory } from "../api";
import { useCollections } from "../collections/context";

const DISMISS_KEY = "onThisDay.dismissed";

function dismissedToday(today: number) {
  try {
    return localStorage.getItem(DISMISS_KEY) === String(today);
  } catch {
    return false;
  }
}

/** "N years ago today" cards above the timeline. Hidden when there's nothing. */
export function OnThisDay({ onPlay }: { onPlay: (days: number[]) => void }) {
  const { showDay } = useCollections();
  const today = todayLocalDay();
  const [memories, setMemories] = useState<DayMemory[]>([]);
  const [hidden, setHidden] = useState(() => dismissedToday(today));

  useEffect(() => {
    if (hidden) return;
    api.onThisDay(today).then(setMemories, () => setMemories([]));
  }, [today, hidden]);

  if (hidden || memories.length === 0) return null;

  const dismiss = () => {
    setHidden(true);
    try {
      localStorage.setItem(DISMISS_KEY, String(today));
    } catch {
      // Shows again next launch; harmless.
    }
  };

  return (
    <section className="on-this-day" aria-label="On this day">
      <div className="otd-head">
        <strong>On this day</strong>
        <button className="ghost small" onClick={() => onPlay(memories.map((m) => m.day))} title="Slideshow">
          ▶ Play
        </button>
        <button className="ghost small" onClick={dismiss} title="Hide until tomorrow">
          ✕
        </button>
      </div>
      <div className="otd-cards">
        {memories.map((m) => (
          <button key={m.day} className="otd-card" onClick={() => showDay(m.day)}>
            <div className={`otd-collage n${Math.min(m.coverIds.length, 4)}`}>
              {m.coverIds.slice(0, 4).map((id) => (
                <img key={id} src={thumbUrl(id)} alt="" />
              ))}
            </div>
            <span className="otd-label">
              {m.yearsAgo === 1 ? "1 year ago" : `${m.yearsAgo} years ago`}
              <span className="muted"> · {m.count.toLocaleString()}</span>
            </span>
          </button>
        ))}
      </div>
    </section>
  );
}
