import type {
  ColumnId,
  TrafficFilter,
  TrafficFilterGroup,
  TrafficFilterValue,
  TrafficDimension,
} from "./models.js";
import { columnDefinitions } from "./table-model.js";
import { localDateTime } from "./traffic-ranges.js";
import catalog from "../../../../crates/app/src/sessions/filter-catalog.json";

export interface FilterChoice {
  key: string;
  label: string;
  value: TrafficFilterValue;
}
export function extensionColumnKey(dimension: TrafficDimension): string | null {
  return dimension.startsWith("extension:") ? dimension.slice("extension:".length) : null;
}
export const filterDimensions: Array<{
  id: TrafficDimension;
  label: string;
  numeric: boolean;
  placeholder: string;
}> = [
  {
    id: "traffic-type",
    label: "Traffic type",
    numeric: false,
    placeholder: "HTTP, WebSocket or SSE",
  },
  ...["method", "status", "state", "protocol"].map((id) =>
    columnDefinitions.find((column) => column.id === id)!,
  ),
  {
    id: "fetch-destination",
    label: "Fetch destination",
    numeric: false,
    placeholder: "Sec-Fetch-Dest",
  },
  ...columnDefinitions.filter(
    (column) => !["method", "status", "state", "protocol"].includes(column.id),
  ),
];
export function filterValueKey(value: TrafficFilterValue): string {
  return value.kind === "unavailable" || value.kind === "other"
    ? value.kind
    : value.kind + ":" + String(value.value).toLowerCase();
}
export function canonicalFilterValue(
  dimension: TrafficDimension,
  value: TrafficFilterValue,
): TrafficFilterValue {
  if (dimension === "host" && (value.kind === "text" || value.kind === "host-subdomains")) {
    return { ...value, value: canonicalHost(value.value, value.kind === "host-subdomains") };
  }
  if (value.kind === "text" && dimension === "content-type") {
    return { kind: "text", value: value.value.split(";")[0]!.trim().toLowerCase() };
  }
  if (dimension !== "protocol" || value.kind !== "text") {
    return value;
  }
  const text = value.value.toLowerCase(),
    preset = catalog.protocol.find(
      (option) => option.value === text || option.aliases?.includes(text),
    );
  return { kind: "text", value: preset?.value ?? text };
}
export function canonicalHost(input: string, wildcard = false): string {
  let value = input;
  if (value.includes(":") && !value.startsWith("[")) {
    // A bare IPv6 literal is valid; a host with a port is not.
    if ((value.match(/:/g)?.length ?? 0) < 2) {
      throw new Error("Ports are not allowed");
    }
    value = "[" + value + "]";
  }
  if (!value || value.length > 253 || /[\s/*%@?#\\]/u.test(value)) {
    throw new Error("Enter a host without scheme, port, path or credentials");
  }
  if (value.startsWith("[") && !value.endsWith("]")) {
    throw new Error("Ports are not allowed");
  }
  const parsed = new URL("http://" + value + "/");
  if (parsed.port || parsed.username || parsed.password || parsed.host !== parsed.hostname) {
    throw new Error("Ports and credentials are not allowed");
  }
  const host = parsed.hostname.toLowerCase().replace(/\.$/, "");
  if (
    !host.startsWith("[") &&
    host.split(".").some((label) => !/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(label))
  ) {
    throw new Error("Enter a valid DNS hostname or IP literal");
  }
  if (wildcard && (host.startsWith("[") || /^\d+\.\d+\.\d+\.\d+$/.test(host))) {
    throw new Error("Wildcards require a DNS name");
  }
  return host;
}
/** Parse the entire paste before changing either selections or durable data. */
export function parseFilterValues(
  dimension: TrafficDimension,
  input: string,
): TrafficFilterValue[] {
  const field = filterDimensions.find((field) => field.id === dimension);
  const literal = ["url", "path", "process"].includes(dimension);
  const tokens = literal ? [input] : input.split(/[,\r\n]/);
  const values = tokens
    .map((item) => item.trim())
    .filter(Boolean)
    .map((item): TrafficFilterValue => {
      try {
        if ([...item].length > 255 || /[\p{Cc}]/u.test(item)) {
          throw new Error("Use at most 255 characters without control characters");
        }
        if (dimension === "host") {
          const wildcard = item.startsWith("*.");
          return {
            kind: wildcard ? "host-subdomains" : "text",
            value: canonicalHost(wildcard ? item.slice(2) : item, wildcard),
          };
        }
        const fixed = fixedFilterChoices(dimension).find(
          (choice) =>
            choice.label.toLowerCase() === item.toLowerCase() || choice.key === item.toLowerCase(),
        );
        if (fixed) {
          return fixed.value;
        }
        if (field?.numeric) {
          if (
            !/^\d+$/.test(item) ||
            BigInt(item) > 18446744073709551615n ||
            (dimension === "request-number" && BigInt(item) < 1n)
          ) {
            throw new Error("Enter an unsigned decimal integer within this field's limits");
          }
          return { kind: "number", value: BigInt(item).toString() };
        }
        if (dimension === "traffic-type") {
          throw new Error("Choose HTTP, WebSocket or SSE");
        }
        if (item.includes("*")) {
          throw new Error("Wildcards are available only for Host");
        }
        return canonicalFilterValue(dimension, { kind: "text", value: item.toLowerCase() });
      } catch (error) {
        throw new Error(`“${item}”: ${error instanceof Error ? error.message : String(error)}`, {
          cause: error,
        });
      }
    });
  return [...new Map(values.map((value) => [filterValueKey(value), value])).values()];
}
export function hasFilterPresets(dimension: TrafficDimension): boolean {
  return Object.hasOwn(catalog, dimension);
}
export function filterLabel(dimension: TrafficDimension): string {
  return filterDimensions.find((field) => field.id === dimension)?.label ?? dimension;
}
const text = (value: string, label = value): FilterChoice => ({
  key: "text:" + value.toLowerCase(),
  label,
  value: { kind: "text", value: value.toLowerCase() },
});
export function fixedFilterChoices(dimension: TrafficDimension): FilterChoice[] {
  if (hasFilterPresets(dimension)) {
    return [
      ...catalog[dimension as keyof typeof catalog].map((option) =>
        text(option.value, option.label),
      ),
      ...(dimension === "method" || dimension === "protocol" || dimension === "fetch-destination"
        ? [
            {
              key: "unavailable",
              label:
                dimension === "method"
                  ? "Unavailable method"
                  : dimension === "protocol"
                    ? "Unavailable protocol"
                    : "Missing fetch destination",
              value: { kind: "unavailable" as const },
            },
          ]
        : []),
      { key: "other", label: "Other", value: { kind: "other" } },
    ];
  }
  switch (dimension) {
    case "process":
      return [{ key: "unavailable", label: "No process name", value: { kind: "unavailable" } }];
    case "content-type":
      return [
        { key: "unavailable", label: "Missing content type", value: { kind: "unavailable" } },
      ];
    case "stream-sent":
    case "stream-received":
    case "duration":
    case "http-duration":
    case "started-at":
      return [{ key: "unavailable", label: "Unavailable", value: { kind: "unavailable" } }];
    case "status":
      return [
        { key: "other", label: "Other", value: { kind: "other" } },
        { key: "unavailable", label: "Unavailable status", value: { kind: "unavailable" } },
        ...[1, 2, 3, 4, 5].map((value) => ({
          key: "status-family:" + value,
          label: value + "xx",
          value: { kind: "status-family" as const, value },
        })),
        ...[
          101, 200, 201, 204, 206, 301, 302, 304, 307, 308, 400, 401, 403, 404, 405, 408, 409, 413,
          415, 422, 429, 500, 502, 503, 504,
        ].map((value) => ({
          key: "number:" + value,
          label: String(value),
          value: { kind: "number" as const, value: String(value) },
        })),
      ];
    default:
      return [];
  }
}
export function valueLabel(dimension: TrafficDimension, value: TrafficFilterValue): string {
  if (value.kind === "host-subdomains") {
    return "*." + value.value;
  }
  if (extensionColumnKey(dimension) && value.kind === "unavailable") {
    return "No match";
  }
  value = canonicalFilterValue(dimension, value);
  const fixed = fixedFilterChoices(dimension).find(
    (choice) => choice.key === filterValueKey(value),
  );
  if (fixed) {
    return fixed.label;
  }
  if (value.kind === "text" && value.value === "") {
    return "(Empty value)";
  }
  return value.kind === "other"
    ? "Other"
    : value.kind === "unavailable"
      ? "Unavailable"
      : value.kind === "status-family"
        ? value.value + "xx"
        : ["method", "protocol"].includes(dimension)
          ? String(value.value).toUpperCase()
          : String(value.value);
}
export interface FilterChip {
  dimension: TrafficDimension;
  label: string;
}
export function trafficFilterChips(
  filters: TrafficFilter[],
  groups: TrafficFilterGroup[],
  destinations: string[],
  fields = filterDimensions,
): FilterChip[] {
  return fields.flatMap((field) => {
    const parts = [
      ...groups
        .filter((group) => group.dimension === field.id)
        .flatMap((group) => {
          const saved =
            group.selectionMode === "ignore-saved" ? 0 : (group.savedValues?.length ?? 0);
          if (saved) {
            return [
              saved +
                " saved" +
                (group.values.length
                  ? (group.selectionMode === "both" ? " and " : " or ") +
                    group.values.length +
                    " picked"
                  : ""),
            ];
          }
          return group.values.length
            ? [group.values.map((value) => valueLabel(field.id, value)).join(" or ")]
            : [];
        }),
      ...filters
        .filter((filter) => filter.column === field.id)
        .map(
          (filter) =>
            ({ contains: "contains", equals: "=", minimum: "≥", maximum: "≤" })[filter.operator] +
            " " +
            (field.id === "started-at"
              ? localDateTime(filter.value).replace("T", " ") || filter.value + " Unix ms"
              : filter.value +
                (["duration", "http-duration"].includes(field.id)
                  ? " ms"
                  : ["request-bytes", "response-bytes", "stream-sent", "stream-received"].includes(
                        field.id,
                      )
                    ? " B"
                    : "")),
        ),
    ];
    if (field.id === "fetch-destination" && destinations.length) {
      parts.push(
        destinations.map((value) => valueLabel(field.id, { kind: "text", value })).join(" or "),
      );
    }
    return parts.length
      ? [{ dimension: field.id, label: field.label + ": " + parts.join(" · ") }]
      : [];
  });
}
export function columnDimension(dimension: TrafficDimension): ColumnId | null {
  return columnDefinitions.some((column) => column.id === dimension)
    ? (dimension as ColumnId)
    : null;
}
