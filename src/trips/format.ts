import type { TripSummary } from "../api";
import { formatEventDates } from "../events/format";

const DAY_MS = 86_400_000;

const joinNames = (names: string[]) =>
  names.length <= 1 ? (names[0] ?? "") : `${names.slice(0, -1).join(", ")} & ${names[names.length - 1]}`;

/** "Dubai", "Islamabad & Gilgit", "Japan" (many towns, one country) or "Rome, Paris, London & 4 more". */
export function tripPlaces(t: TripSummary) {
  const names = t.places.map((p) => p.name);
  if (names.length === 0) return "Trip";
  const more = t.placeCount - names.length;
  if (more === 0) return joinNames(names);
  if (t.countries.length <= 3) return joinNames(t.countries);
  return `${names.join(", ")} & ${more} more`;
}

export const tripTitle = (t: TripSummary) => t.title ?? tripPlaces(t);

export const formatTripDates = (t: TripSummary) =>
  formatEventDates({ start: t.firstDay * DAY_MS, end: t.lastDay * DAY_MS });

export const tripLength = (t: TripSummary) => `${(t.lastDay - t.firstDay + 1).toLocaleString()} days`;

export const tripYear = (t: TripSummary) => new Date(t.firstDay * DAY_MS).getUTCFullYear();
