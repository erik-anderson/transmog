import type { TrafficTimelineRow } from "../models.js";

/** Compact chart labels; inspection details retain the exact measured value. */
export function timelineTime(micros: number | null): string {
  if (micros == null) {
    return "Unavailable";
  }
  const [scale, unit] = timeUnit(Math.abs(micros));
  return (micros / scale).toLocaleString(undefined, { maximumFractionDigits: 3 }) + " " + unit;
}
/** Both endpoints share units chosen for the visible span, preserving zoom detail. */
export function timelineRange(
  fromMicros: number,
  toMicros: number,
): { fromLabel: string; toLabel: string; range: string } {
  const span = Math.max(0.001, toMicros - fromMicros),
    [scale, unit] = timeUnit(span);
  const precision = Math.max(3, Math.min(9, Math.ceil(Math.log10(scale / span)) + 1));
  const label = (value: number) =>
    (value / scale).toLocaleString(undefined, { maximumFractionDigits: precision }) + " " + unit;
  const fromLabel = label(fromMicros),
    toLabel = label(toMicros);
  return { fromLabel, toLabel, range: fromLabel + " – " + toLabel };
}
function timeUnit(micros: number): [number, string] {
  return micros >= 3_600_000_000
    ? [3_600_000_000, "h"]
    : micros >= 60_000_000
      ? [60_000_000, "min"]
      : micros >= 1_000_000
        ? [1_000_000, "s"]
        : [1000, "ms"];
}
export function requestAddress(url: string): { path: string; host: string } {
  // Split display evidence rather than parsing it as a navigable URL. URL() would
  // remove dot segments, normalize escapes/authority, and discard an empty query.
  const address = /^[a-z][a-z\d+.-]*:\/\/([^/?#]+)([/?#].*)?$/is.exec(url);
  if (!address) {
    return { path: url, host: "" };
  }
  const suffix = address[2] ?? "";
  return { path: suffix.startsWith("/") ? suffix : "/" + suffix, host: address[1]! };
}

export function intervalEnd(row: TrafficTimelineRow): number | null {
  return row.startMillis == null || row.durationMicros == null
    ? null
    : row.startMillis + row.durationMicros / 1000;
}
export function overlaps(row: TrafficTimelineRow, from: number, to: number): boolean {
  const end = intervalEnd(row);
  // Unplaceable evidence remains visible and explicitly unavailable.
  return row.startMillis == null || end == null || (row.startMillis <= to && end >= from);
}
export function clampWindow(
  from: number,
  to: number,
  minimum: number,
  maximum: number,
  part: string,
  delta: number,
): [number, number] {
  const smallest = Math.min(1, Math.max(0.001, (maximum - minimum) / 1000));
  if (part === "start") {
    return [Math.max(minimum, Math.min(to - smallest, from + delta)), to];
  }
  if (part === "end") {
    return [from, Math.min(maximum, Math.max(from + smallest, to + delta))];
  }
  const movement = Math.max(minimum - from, Math.min(maximum - to, delta));
  return [from + movement, to + movement];
}
