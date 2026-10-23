import { useEffect, useState } from "react";
import { api, type WallRange } from "../api";
import { formatTakenAt } from "../format";

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** `datetime-local` value for wall-clock ms (shown as-is, no zone conversion). */
const toInput = (ms: number) => new Date(ms).toISOString().slice(0, 16);
const fromInput = (value: string) => (value ? Date.parse(`${value}:00Z`) : NaN);
const wall = (ms: number) => formatTakenAt(ms, 0);

type Props = {
  ids: number[];
  onApply: (deltaMs: number) => void;
  onRestore: () => void;
};

/**
 * Set the earliest item to a date (everything else moves by the same amount, so the
 * order is kept), or shift everything by days/hours/minutes, e.g. a camera whose
 * clock was off.
 */
export function DateFixer({ ids, onApply, onRestore }: Props) {
  const [range, setRange] = useState<WallRange | null>(null);
  const [mode, setMode] = useState<"set" | "shift">("set");
  const [target, setTarget] = useState("");
  const [shift, setShift] = useState({ days: 0, hours: 0, minutes: 0 });

  useEffect(() => {
    api.dateRange(ids).then((r) => {
      setRange(r);
      if (r) setTarget(toInput(r.first));
    });
  }, [ids]);

  if (!range) return <p className="muted small">Loading…</p>;

  const delta =
    mode === "set"
      ? fromInput(target) - range.first
      : shift.days * DAY + shift.hours * HOUR + shift.minutes * MINUTE;
  const valid = Number.isFinite(delta) && delta !== 0;
  const single = ids.length === 1;

  return (
    <div className="date-fixer">
      <div className="segmented">
        <button className={mode === "set" ? "on" : ""} onClick={() => setMode("set")}>
          Set date
        </button>
        <button className={mode === "shift" ? "on" : ""} onClick={() => setMode("shift")}>
          Shift by
        </button>
      </div>

      {mode === "set" ? (
        <label className="field">
          <span className="muted small">{single ? "New date and time" : "Earliest item becomes"}</span>
          <input type="datetime-local" value={target} onChange={(e) => setTarget(e.target.value)} />
        </label>
      ) : (
        <div className="shift-fields">
          {(["days", "hours", "minutes"] as const).map((unit) => (
            <label key={unit} className="field">
              <span className="muted small">{unit}</span>
              <input
                type="number"
                value={shift[unit]}
                onChange={(e) => setShift({ ...shift, [unit]: Number(e.target.value) || 0 })}
              />
            </label>
          ))}
        </div>
      )}

      <div className="date-preview small">
        {single ? (
          <p>
            {wall(range.first)}
            <br />→ {valid ? wall(range.first + delta) : "…"}
          </p>
        ) : (
          <>
            <p className="muted">All {ids.length.toLocaleString()} items move together, keeping their order.</p>
            <p>
              First: {wall(range.first)} → {valid ? wall(range.first + delta) : "…"}
            </p>
            <p>
              Last: {wall(range.last)} → {valid ? wall(range.last + delta) : "…"}
            </p>
          </>
        )}
      </div>

      <div className="date-actions">
        <button className="link small" onClick={onRestore}>
          Restore original dates
        </button>
        <button className="primary" disabled={!valid} onClick={() => onApply(delta)}>
          Apply
        </button>
      </div>
    </div>
  );
}
