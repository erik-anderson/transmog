/** Decoded editing fields retain their exact, independently authored URL bytes. */
export interface QueryParameterField {
  name: string;
  value: string;
  hasEquals: boolean;
  rawName: string;
  rawValue: string;
}

export interface QueryUrlDraft {
  prefix: string;
  fragment: string;
  hasQuery: boolean;
  rows: QueryParameterField[];
}

function decodeComponent(value: string): string {
  try {
    return decodeURIComponent(value.replace(/\+/g, " "));
  } catch {
    // Invalid percent escapes are still editable evidence; opening this editor
    // must not repair, discard, or reinterpret them.
    return value;
  }
}

function encodeComponent(value: string): string {
  // Native text inputs normally contain well-formed text, but a programmatic
  // edit can contain a lone surrogate. Match browser UTF-8 string conversion.
  const wellFormed = value.replace(/[\uD800-\uDBFF][\uDC00-\uDFFF]|[\uD800-\uDFFF]/g, (part) =>
    part.length === 2 ? part : "\uFFFD",
  );
  return encodeURIComponent(wellFormed);
}

/** Split the authored URL without applying URL or URLSearchParams normalization. */
export function parseQueryUrl(value: string): QueryUrlDraft {
  const fragmentAt = value.indexOf("#");
  const beforeFragment = fragmentAt < 0 ? value : value.slice(0, fragmentAt);
  const queryAt = beforeFragment.indexOf("?");
  const query = queryAt < 0 ? "" : beforeFragment.slice(queryAt + 1);
  return {
    prefix: queryAt < 0 ? beforeFragment : beforeFragment.slice(0, queryAt),
    fragment: fragmentAt < 0 ? "" : value.slice(fragmentAt),
    hasQuery: queryAt >= 0,
    rows: query
      ? query.split("&").map((part) => {
          const separator = part.indexOf("=");
          const rawName = separator < 0 ? part : part.slice(0, separator);
          const rawValue = separator < 0 ? "" : part.slice(separator + 1);
          return {
            name: decodeComponent(rawName),
            value: decodeComponent(rawValue),
            hasEquals: separator >= 0,
            rawName,
            rawValue,
          };
        })
      : [],
  };
}

export function formatQueryUrl(draft: QueryUrlDraft): string {
  const query = draft.rows
    .map((row) => row.rawName + (row.hasEquals ? `=${row.rawValue}` : ""))
    .join("&");
  return draft.prefix + (draft.hasQuery || draft.rows.length ? `?${query}` : "") + draft.fragment;
}

export function newQueryParameter(): QueryParameterField {
  return { name: "", value: "", hasEquals: true, rawName: "", rawValue: "" };
}

/** Encode only the component that actually changed, preserving all other bytes. */
export function editQueryParameter(
  row: QueryParameterField,
  field: "name" | "value",
  value: string,
): QueryParameterField {
  if (row[field] === value) {
    return { ...row };
  }
  return field === "name"
    ? { ...row, name: value, rawName: encodeComponent(value) }
    : { ...row, value, rawValue: encodeComponent(value), hasEquals: row.hasEquals || value !== "" };
}
