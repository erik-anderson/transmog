import type { ColumnId } from "./models.js";

export type RangeKind = "duration" | "bytes" | "integer" | "time" | "";
export const maxUnsigned = 18446744073709551615n;
export function rangeKind(column: string): RangeKind {
  return column === "started-at"
    ? "time"
    : column === "request-number"
      ? "integer"
      : ["duration", "http-duration"].includes(column)
        ? "duration"
        : ["request-bytes", "response-bytes", "stream-sent", "stream-received"].includes(column)
          ? "bytes"
          : "";
}
export function rangeUnits(kind: RangeKind): Array<{ value: string; label: string }> {
  return (
    kind === "duration" ? ["ms", "s", "min"] : kind === "bytes" ? ["B", "KiB", "MiB", "GiB"] : ["1"]
  ).map((value) => ({ value, label: value === "1" ? "requests" : value }));
}
export function decimalToBase(text: string, unit: string, positive = false): string {
  if (text.trim().length > 64) {
    throw new Error("Use at most 64 characters.");
  }
  const match = /^(\d+)(?:\.(\d+))?$/.exec(text.trim());
  if (!match) {
    throw new Error("Enter a nonnegative decimal number without signs or exponents.");
  }
  const factor = (
    {
      ms: 1n,
      s: 1000n,
      min: 60000n,
      B: 1n,
      KiB: 1024n,
      MiB: 1048576n,
      GiB: 1073741824n,
      "1": 1n,
    } as Record<string, bigint>
  )[unit];
  if (factor === undefined) {
    throw new Error("Choose a supported unit.");
  }
  const fraction = match[2] ?? "";
  if (fraction.length > 20) {
    throw new Error("Use at most 20 decimal places.");
  }
  const divisor = 10n ** BigInt(fraction.length),
    scaled = (BigInt(match[1]!) * divisor + BigInt(fraction || "0")) * factor;
  if (scaled % divisor !== 0n) {
    throw new Error("The value must resolve to whole milliseconds, bytes or request numbers.");
  }
  const value = scaled / divisor;
  if (value > maxUnsigned || (positive && value === 0n)) {
    throw new Error(
      positive
        ? "Enter a positive request number up to 18446744073709551615."
        : "The value must be at most 18446744073709551615.",
    );
  }
  return value.toString();
}
export function localDateTime(value: string): string {
  if (!/^\d+$/.test(value)) {
    return "";
  }
  const number = Number(value);
  if (!Number.isSafeInteger(number)) {
    return "";
  }
  const date = new Date(number);
  if (!Number.isFinite(date.valueOf()) || date.getFullYear() < 1970 || date.getFullYear() > 9999) {
    return "";
  }
  const pad = (n: number, width = 2) => String(n).padStart(width, "0");
  return `${pad(date.getFullYear(), 4)}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}.${pad(date.getMilliseconds(), 3)}`;
}
export function localToMillis(text: string): string {
  const match = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})(?::(\d{2})(?:\.(\d{1,3}))?)?$/.exec(text);
  if (!match) {
    throw new Error("Enter a complete local date and time.");
  }
  const values = [
    Number(match[1]),
    Number(match[2]) - 1,
    Number(match[3]),
    Number(match[4]),
    Number(match[5]),
    Number(match[6] ?? 0),
    Number((match[7] ?? "").padEnd(3, "0")),
  ];
  const date = new Date(0);
  date.setFullYear(values[0]!, values[1]!, values[2]!);
  date.setHours(values[3]!, values[4]!, values[5]!, values[6]!);
  const actual = [
    date.getFullYear(),
    date.getMonth(),
    date.getDate(),
    date.getHours(),
    date.getMinutes(),
    date.getSeconds(),
    date.getMilliseconds(),
  ];
  if (
    !Number.isSafeInteger(date.valueOf()) ||
    date.valueOf() < 0 ||
    values.some((value, index) => value !== actual[index])
  ) {
    throw new Error(
      "Enter a valid local time on or after 1970; skipped daylight-saving times are unavailable.",
    );
  }
  return String(date.valueOf());
}
export function inclusiveRange(
  column: ColumnId,
  from: string,
  to: string,
  unit: string,
  original?: { from?: { text: string; value: string }; to?: { text: string; value: string } },
): Array<{ column: ColumnId; operator: "minimum" | "maximum"; value: string; label: string }> {
  const parse = (text: string, bound: "from" | "to") =>
    rangeKind(column) === "time"
      ? original?.[bound]?.text === text
        ? original[bound]!.value
        : localToMillis(text)
      : decimalToBase(text, unit, column === "request-number");
  const lower = from ? parse(from, "from") : null,
    upper = to ? parse(to, "to") : null;
  if (lower !== null && upper !== null && BigInt(lower) > BigInt(upper)) {
    throw new Error("From must be less than or equal to To. Both bounds are inclusive.");
  }
  return [
    ...(lower === null ? [] : [{ column, operator: "minimum" as const, value: lower, label: "" }]),
    ...(upper === null ? [] : [{ column, operator: "maximum" as const, value: upper, label: "" }]),
  ];
}
