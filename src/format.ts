const DAY_MS = 86_400_000;

// Local days and wall-clock times are pre-shifted, so format them as UTC to avoid a
// second time-zone shift by the browser.
const dayFormat = new Intl.DateTimeFormat(undefined, {
  weekday: "short",
  day: "numeric",
  month: "short",
  year: "numeric",
  timeZone: "UTC",
});
const monthFormat = new Intl.DateTimeFormat(undefined, { month: "short", year: "numeric", timeZone: "UTC" });
const dateTimeFormat = new Intl.DateTimeFormat(undefined, {
  weekday: "long",
  day: "numeric",
  month: "long",
  year: "numeric",
  hour: "numeric",
  minute: "2-digit",
  timeZone: "UTC",
});

export const formatDay = (day: number) => dayFormat.format(day * DAY_MS);
export const formatMonth = (day: number) => monthFormat.format(day * DAY_MS);
export const yearOf = (day: number) => new Date(day * DAY_MS).getUTCFullYear();

/** Capture time in the photo's own time zone. */
export const formatTakenAt = (takenAt: number, tzOffsetMin: number) =>
  dateTimeFormat.format(takenAt + tzOffsetMin * 60_000);

export function formatOffset(min: number) {
  const sign = min < 0 ? "−" : "+";
  const abs = Math.abs(min);
  return `UTC${sign}${String(Math.floor(abs / 60)).padStart(2, "0")}:${String(abs % 60).padStart(2, "0")}`;
}

export function formatDuration(ms: number) {
  const total = Math.round(ms / 1000);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = String(total % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${s}` : `${m}:${s}`;
}

export function formatBytes(bytes: number) {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value.toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`;
}

export const fileName = (path: string) => path.split(/[\\/]/).filter(Boolean).pop() ?? path;
