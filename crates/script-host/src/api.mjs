// Capability-free helpers shared by the sandbox and conformance tests.
export function pass(evidence = {}) {
  return { ...evidence, decision: { action: "continue" } };
}
export function edit(edits, evidence = {}) {
  return { ...evidence, decision: { action: "edit", edits } };
}

function checkedBytes(bytes) {
  if (!Array.isArray(bytes) || bytes.some((b) => !Number.isInteger(b) || b < 0 || b > 255)) {
    throw new TypeError("Expected an array of bytes");
  }
  return bytes;
}

export function utf8Encode(text) {
  if (typeof text !== "string") throw new TypeError("Expected text");
  const bytes = [];
  for (const character of text) {
    let cp = character.codePointAt(0);
    if (cp >= 0xd800 && cp <= 0xdfff) cp = 0xfffd;
    if (cp < 0x80) bytes.push(cp);
    else if (cp < 0x800) bytes.push(0xc0 | (cp >> 6), 0x80 | (cp & 63));
    else if (cp < 0x10000) bytes.push(0xe0 | (cp >> 12), 0x80 | ((cp >> 6) & 63), 0x80 | (cp & 63));
    else
      bytes.push(
        0xf0 | (cp >> 18),
        0x80 | ((cp >> 12) & 63),
        0x80 | ((cp >> 6) & 63),
        0x80 | (cp & 63),
      );
  }
  return bytes;
}

export function utf8Decode(input, { fatal = true } = {}) {
  const bytes = checkedBytes(input);
  const output = [];
  for (let i = 0; i < bytes.length;) {
    const first = bytes[i++];
    if (first < 0x80) {
      output.push(String.fromCharCode(first));
      continue;
    }
    const count =
      first >= 0xc2 && first <= 0xdf
        ? 1
        : first >= 0xe0 && first <= 0xef
          ? 2
          : first >= 0xf0 && first <= 0xf4
            ? 3
            : 0;
    let cp = first & (count === 1 ? 31 : count === 2 ? 15 : 7);
    let valid = count > 0;
    for (let n = 0; valid && n < count; n++) {
      const b = bytes[i];
      const min = n === 0 && first === 0xe0 ? 0xa0 : n === 0 && first === 0xf0 ? 0x90 : 0x80;
      const max = n === 0 && first === 0xed ? 0x9f : n === 0 && first === 0xf4 ? 0x8f : 0xbf;
      if (b === undefined || b < min || b > max) {
        valid = false;
        break;
      }
      i++;
      cp = (cp << 6) | (b & 63);
    }
    if (!valid && fatal) throw new TypeError("Invalid UTF-8");
    output.push(valid ? String.fromCodePoint(cp) : "\ufffd");
  }
  return output.join("");
}

const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
export function base64Encode(input) {
  const bytes = checkedBytes(input);
  let result = "";
  for (let i = 0; i < bytes.length; i += 3) {
    const value = (bytes[i] << 16) | ((bytes[i + 1] ?? 0) << 8) | (bytes[i + 2] ?? 0);
    result +=
      alphabet[(value >>> 18) & 63] +
      alphabet[(value >>> 12) & 63] +
      (i + 1 < bytes.length ? alphabet[(value >>> 6) & 63] : "=") +
      (i + 2 < bytes.length ? alphabet[value & 63] : "=");
  }
  return result;
}

export function base64Decode(text) {
  if (
    typeof text !== "string" ||
    !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(text)
  )
    throw new TypeError("Invalid base64");
  const result = [];
  for (let i = 0; i < text.length; i += 4) {
    const value =
      (alphabet.indexOf(text[i]) << 18) |
      (alphabet.indexOf(text[i + 1]) << 12) |
      (Math.max(0, alphabet.indexOf(text[i + 2])) << 6) |
      Math.max(0, alphabet.indexOf(text[i + 3]));
    result.push((value >>> 16) & 255);
    if (text[i + 2] !== "=") result.push((value >>> 8) & 255);
    if (text[i + 3] !== "=") result.push(value & 255);
  }
  if (base64Encode(result) !== text) throw new TypeError("Non-canonical base64");
  return result;
}

export function headerValues(headers, name) {
  const fields = headers.filter((h) => h.name.toLowerCase() === name.toLowerCase());
  if (fields.some((h) => h.redacted)) throw new TypeError("Header value was withheld");
  return fields.map((h) => h.value);
}
export function headerText(headers, name) {
  return headerValues(headers, name).map((v) => utf8Decode(v));
}
export function setHeader(name, value) {
  return { operation: "set", name, value };
}
export function appendHeader(name, value) {
  return { operation: "append", name, value };
}
export function removeHeader(name) {
  return { operation: "remove", name };
}
export function requestUrl(request) {
  const host =
    request.host.includes(":") && !request.host.startsWith("[")
      ? `[${request.host}]`
      : request.host;
  const port =
    (request.scheme === "https" && request.port === 443) ||
    (request.scheme === "http" && request.port === 80)
      ? ""
      : `:${request.port}`;
  return `${request.scheme}://${host}${port}${request.path}${request.query === null ? "" : `?${request.query}`}`;
}
export function parseQuery(query) {
  if (!query) return [];
  return query.split("&").map((pair) => {
    const index = pair.indexOf("=");
    const name = index < 0 ? pair : pair.slice(0, index);
    const value = index < 0 ? "" : pair.slice(index + 1);
    return [name, value].map((part) => decodeURIComponent(part.replace(/\+/g, " ")));
  });
}
export function formatQuery(pairs) {
  return pairs
    .map(([name, value]) =>
      [name, value]
        .map((part) =>
          encodeURIComponent(part)
            .replace(/[!'()~]/g, (c) => `%${c.charCodeAt(0).toString(16).toUpperCase()}`)
            .replace(/%20/g, "+"),
        )
        .join("="),
    )
    .join("&");
}
export function setQuery(query, name, value) {
  const pairs = parseQuery(query);
  const index = pairs.findIndex(([key]) => key === name);
  const filtered = pairs.filter(([key]) => key !== name);
  filtered.splice(index < 0 ? filtered.length : index, 0, [name, value]);
  return formatQuery(filtered);
}
export function readText(body, headers = []) {
  const types = headerText(headers, "content-type");
  if (types.length > 1) throw new TypeError("Ambiguous content type");
  const charsets = [...(types[0] ?? "").matchAll(/;\s*charset\s*=\s*(?:"([^"]*)"|([^;\s]*))/gi)];
  if (charsets.length > 1 || charsets.some((match) => !/^utf-?8$/i.test(match[1] ?? match[2])))
    throw new TypeError("Expected UTF-8 charset");
  return utf8Decode(body.bytes);
}
export function readJson(body, headers = []) {
  return JSON.parse(readText(body, headers).replace(/^\ufeff/, ""));
}
export function replaceText(text, mediaType = "text/plain") {
  return {
    decision: {
      action: "edit",
      edits: {
        bytes: utf8Encode(text),
        headers: [setHeader("content-type", `${mediaType}; charset=utf-8`)],
      },
    },
  };
}
export function replaceJson(value) {
  return replaceText(JSON.stringify(value), "application/json");
}
export function respondText(status, text, headers = []) {
  return {
    decision: {
      action: "respond",
      response: {
        status,
        headers: [
          ["content-type", "text/plain; charset=utf-8"],
          ...headers.filter(([name]) => name.toLowerCase() !== "content-type"),
        ],
        bytes: utf8Encode(text),
      },
    },
  };
}
export function respondJson(status, value, headers = []) {
  return {
    decision: {
      action: "respond",
      response: {
        status,
        headers: [
          ["content-type", "application/json; charset=utf-8"],
          ...headers.filter(([name]) => name.toLowerCase() !== "content-type"),
        ],
        bytes: utf8Encode(JSON.stringify(value)),
      },
    },
  };
}

export function respondAsset(assetRef) {
  return { decision: { action: "respond", response: { assetRef } } };
}

/** Cookie values stay encoded as supplied; fields and duplicate names retain order. */
export function readCookies(headers) {
  return headerText(headers, "cookie").flatMap((field) =>
    field.split(";").map((part) => {
      const index = part.indexOf("=");
      if (index < 1) throw new TypeError("Invalid Cookie pair");
      const pair = [part.slice(0, index).trim(), part.slice(index + 1).trim()];
      cookiePair(...pair);
      return pair;
    }),
  );
}
function cookiePair(name, value) {
  if (
    typeof name !== "string" ||
    !/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/.test(name) ||
    typeof value !== "string" ||
    !/^(?:[\x21\x23-\x2B\x2D-\x3A\x3C-\x5B\x5D-\x7E]*|"[\x21\x23-\x2B\x2D-\x3A\x3C-\x5B\x5D-\x7E]*")$/.test(
      value,
    )
  )
    throw new TypeError("Invalid cookie name or value");
  return `${name}=${value}`;
}
export function replaceCookies(pairs) {
  return pairs.length
    ? setHeader("cookie", pairs.map(([name, value]) => cookiePair(name, value)).join("; "))
    : removeHeader("cookie");
}
export function readSetCookies(headers) {
  return headerText(headers, "set-cookie").map((field) => {
    const [pair, ...parts] = field.split(";");
    const index = pair.indexOf("=");
    if (index < 1) throw new TypeError("Invalid Set-Cookie pair");
    const name = pair.slice(0, index).trim(),
      value = pair.slice(index + 1).trim();
    cookiePair(name, value);
    const attributes = parts
      .filter((part) => part.trim() !== "")
      .map((part) => {
        const index = part.indexOf("=");
        return index < 0
          ? [part.trim(), null]
          : [part.slice(0, index).trim(), part.slice(index + 1).trim()];
      });
    formatSetCookie({ name, value, attributes });
    return { name, value, attributes };
  });
}
function formatSetCookie({ name, value, attributes = [] }) {
  return (
    cookiePair(name, value) +
    attributes
      .map(([name, value]) => {
        if (
          !/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/.test(name) ||
          (value !== null && (typeof value !== "string" || /[^\x20-\x3A\x3C-\x7E]/.test(value)))
        )
          throw new TypeError("Invalid cookie attribute");
        return `; ${name}${value === null ? "" : `=${value}`}`;
      })
      .join("")
  );
}
export function replaceSetCookies(cookies) {
  return [
    removeHeader("set-cookie"),
    ...cookies.map((cookie) => appendHeader("set-cookie", formatSetCookie(cookie))),
  ];
}
export function readForm(body, headers) {
  const types = headerText(headers, "content-type");
  if (
    types.length !== 1 ||
    types[0].split(";", 1)[0].trim().toLowerCase() !== "application/x-www-form-urlencoded"
  )
    throw new TypeError("Expected URL-encoded form content type");
  return parseQuery(readText(body, headers));
}
export function replaceForm(pairs) {
  return replaceText(formatQuery(pairs), "application/x-www-form-urlencoded");
}
