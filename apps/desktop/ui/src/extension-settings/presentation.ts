import type {
  ContributionRow,
  ExtensionContribution,
  ExtensionEntry,
  ExtensionRow,
  ExtensionRule,
} from "./models.js";

type Group<T> = [T, ...T[]];

const bodyLabel = { none: "No body access", optional: "Optional", required: "Required" };
const headersLabel = (headers: string[]): string =>
  headers.length ? headers.join(", ") : "No headers";
const bodyAccess = (access: ExtensionContribution["inputs"]["request"]): string =>
  bodyLabel[access.body] + (access.decodeHttpContent ? " · HTTP decoding on demand" : "");
const directions = (rules: ExtensionRule[]): string => {
  const sides = new Set(rules.map((rule) => rule.direction));
  return sides.size === 2 ? "Request and response" : sides.has("request") ? "Request" : "Response";
};
const headerLabel = (header: ExtensionRule["headers"][number]): string =>
  `${header.direction} ${header.name}: ${header.test.op}${header.test.value === undefined ? "" : ` ${header.test.value}`}`;
const ruleLabel = (rule: ExtensionRule, direction: string): string =>
  [
    direction,
    ...(rule.contentTypes ?? []),
    ...(rule.methods ?? []),
    ...(rule.pathPrefixes ?? []),
    ...(rule.statuses ?? []).map(String),
    ...(rule.headers ?? []).map(headerLabel),
  ].join(" · ");

// Object property order is irrelevant; arrays keep their declared order unless
// the caller explicitly identifies them as unordered matching alternatives.
const canonical = (value: unknown): string =>
  JSON.stringify(value, (_key, part: unknown) =>
    part && typeof part === "object" && !Array.isArray(part)
      ? Object.fromEntries(
          Object.entries(part).sort(([left], [right]) =>
            left < right ? -1 : left > right ? 1 : 0,
          ),
        )
      : part,
  );
const ruleKey = (rule: ExtensionRule): string =>
  canonical([
    [...(rule.contentTypes ?? [])].sort(),
    [...(rule.methods ?? [])].sort(),
    [...(rule.pathPrefixes ?? [])].sort(),
    [...(rule.statuses ?? [])].sort((left, right) => left - right),
    (rule.headers ?? []).map(canonical).sort(),
  ]);

export function matchingMessages(rules: ExtensionRule[]): string[] {
  const groups = new Map<string, Group<ExtensionRule>>();
  for (const rule of rules) {
    const key = ruleKey(rule);
    const group = groups.get(key);
    if (group) {
      group.push(rule);
    } else {
      groups.set(key, [rule]);
    }
  }
  return [...groups.values()].map((group) => ruleLabel(group[0], directions(group)));
}

const singleDirection = (value: ExtensionContribution): ExtensionRule["direction"] | null => {
  const sides = new Set(value.matches.map((rule) => rule.direction));
  return sides.size === 1 ? (value.matches[0]?.direction ?? null) : null;
};
const groupingKey = (value: ExtensionContribution): string =>
  canonical([
    value.name,
    value.entry,
    [...value.surfaces].sort(),
    Boolean(value.automatic),
    value.liveUpdates ?? null,
    value.context ?? null,
    value.inputMatches ?? [],
    value.limits,
  ]);

function accessSummary(value: ExtensionContribution): string {
  const side = singleDirection(value);
  const selected = side ? value.inputs[side] : null;
  const paired = side ? value.inputs[side === "request" ? "response" : "request"] : null;
  const parts: string[] = [];
  if (selected && paired) {
    if (selected.body !== "none") {
      parts.push(`Selected body ${selected.body}`);
    }
    if (paired.body !== "none") {
      parts.push(`Paired body ${paired.body}`);
    }
  } else {
    for (const direction of ["request", "response"] as const) {
      if (value.inputs[direction].body !== "none") {
        parts.push(
          `${direction === "request" ? "Request" : "Response"} body ${value.inputs[direction].body}`,
        );
      }
    }
  }
  const hasHeaders = value.inputs.request.headers.length || value.inputs.response.headers.length;
  if (hasHeaders) {
    parts.push(parts.length ? "Declared headers" : "Declared headers only");
  }
  if (!parts.length) {
    parts.push("No message inputs");
  }
  return parts.join(" · ");
}

function contribution(values: Group<ExtensionContribution>): ContributionRow {
  const first = values[0];
  const rules = values.flatMap((value) => value.matches);
  const matches = matchingMessages(rules);
  const accesses = values.map(accessSummary);
  const access =
    new Set(accesses).size === 1
      ? accessSummary(first)
      : values
          .map((value, index) => `${directions(value.matches)} viewer: ${accesses[index]}`)
          .join("; ");
  const mediaTypes = [...new Set(rules.flatMap((rule) => rule.contentTypes ?? []))];
  return {
    id: first.id,
    name: first.name,
    surfaces:
      first.surfaces.map((side) => (side === "headers" ? "Headers" : "Body")).join(" and ") +
      (first.automatic ? " · Auto on complete parse" : ""),
    access:
      access + (first.context ? ` · Up to ${first.context.maxExchanges} related exchanges` : ""),
    inputs: (
      [
        ["Request headers", "request", "headers"],
        ["Request body", "request", "body"],
        ["Response headers", "response", "headers"],
        ["Response body", "response", "body"],
      ] as const
    ).flatMap(([name, side, part]) => {
      const opposite = side === "request" ? "response" : "request";
      const visible = values.filter((value) => {
        const access = value.inputs[side];
        const readable = part === "headers" ? access.headers.length > 0 : access.body !== "none";
        return singleDirection(value) !== opposite || readable;
      });
      const labels = visible.map((value) =>
        part === "headers"
          ? headersLabel(value.inputs[side].headers)
          : bodyAccess(value.inputs[side]),
      );
      const firstLabel = labels[0];
      if (firstLabel === undefined) {
        return [];
      }
      return [
        {
          name,
          value:
            new Set(labels).size === 1 && visible.length === values.length
              ? firstLabel
              : visible
                  .map((value, index) => `${directions(value.matches)} viewer: ${labels[index]}`)
                  .join("\n"),
        },
      ];
    }),
    matches,
    matchingSummary: [directions(rules), ...mediaTypes, `${matches.length} matching rules`].join(
      " · ",
    ),
    inputMatches: matchingMessages(first.inputMatches ?? []),
    targets: values.map((value) => ({ id: value.id, direction: directions(value.matches) })),
    live: first.liveUpdates
      ? `Optional ${first.liveUpdates.events.join("/")} updates${first.liveUpdates.bodyProgress ? ", including body progress" : ""}`
      : "Not supported",
    context: first.context
      ? `Up to ${first.context.maxExchanges} ${first.context.lookAhead ? "earlier or later" : "earlier"} exchanges from the same capture, origin, and conversation; ${first.context.headers.map((header) => `${header.direction} ${header.name}`).join(", ")}`
      : "Selected exchange only",
  };
}

export function contributionRows(values: ExtensionContribution[]): ContributionRow[] {
  const groups = new Map<string, Group<ExtensionContribution>>();
  for (const value of values) {
    const key = groupingKey(value);
    const group = groups.get(key);
    if (group) {
      group.push(value);
    } else {
      groups.set(key, [value]);
    }
  }
  const paired = new Map<ExtensionContribution, Group<ExtensionContribution>>();
  for (const group of groups.values()) {
    const second = group[1];
    if (group.length !== 2 || !second) {
      continue;
    }
    const left = singleDirection(group[0]),
      right = singleDirection(second);
    if (left && right && left !== right) {
      const pair: Group<ExtensionContribution> =
        left === "request" ? [group[0], second] : [second, group[0]];
      for (const value of group) {
        paired.set(value, pair);
      }
    }
  }
  // Keep manifest order and stable contribution keys through catalog refreshes.
  const emitted = new Set<Group<ExtensionContribution>>();
  return values.flatMap((value) => {
    const group = paired.get(value);
    if (!group) {
      return [contribution([value])];
    }
    if (emitted.has(group)) {
      return [];
    }
    emitted.add(group);
    return [contribution(group)];
  });
}

export function extensionRow(entry: ExtensionEntry): ExtensionRow {
  const { manifest, enabled, origin, digest } = entry.catalog;
  return {
    id: manifest.id,
    name: manifest.name,
    version: manifest.version,
    enabled,
    origin:
      origin === "bundled"
        ? "Included with Transmog"
        : origin === "development"
          ? "Unpacked development folder"
          : "Installed",
    bundled: origin === "bundled",
    development: origin === "development",
    directory: entry.directory ?? "",
    hotReload: entry.hotReload,
    runtime: manifest.runtime,
    revision: digest,
    visualizers: contributionRows(manifest.visualizers),
    columns: (manifest.trafficColumns ?? []).map((column) => ({
      id: column.id,
      name: column.name,
      requestHeaders: headersLabel(column.inputs.request.headers),
      responseHeaders: headersLabel(column.inputs.response.headers),
      values: column.values.map((value) => ({
        value: value.value,
        matches: value.matches.map((rule) => ruleLabel(rule, directions([rule]))),
      })),
    })),
  };
}
