import type { EventSummary } from "../api";

// Event times are wall-clock ms, so format as UTC (see ../format.ts).
const day = new Intl.DateTimeFormat(undefined, {
  weekday: "short",
  day: "numeric",
  month: "short",
  year: "numeric",
  timeZone: "UTC",
});
const full = new Intl.DateTimeFormat(undefined, { day: "numeric", month: "short", year: "numeric", timeZone: "UTC" });
const dayMonth = new Intl.DateTimeFormat(undefined, { day: "numeric", month: "short", timeZone: "UTC" });

const sameDay = (a: number, b: number) => Math.floor(a / 86_400_000) === Math.floor(b / 86_400_000);

const yearOf = (ms: number) => new Date(ms).getUTCFullYear();

/** "Sat, 12 Mar 2022", "12 Mar – 14 Mar 2022" or "30 Dec 2021 – 2 Jan 2022". */
export function formatEventDates(e: Pick<EventSummary, "start" | "end">) {
  if (sameDay(e.start, e.end)) return day.format(e.start);
  const from = yearOf(e.start) === yearOf(e.end) ? dayMonth.format(e.start) : full.format(e.start);
  return `${from} – ${full.format(e.end)}`;
}

export const eventTitle = (e: EventSummary) => e.title ?? formatEventDates(e);

export const eventYear = (e: EventSummary) => yearOf(e.start);
