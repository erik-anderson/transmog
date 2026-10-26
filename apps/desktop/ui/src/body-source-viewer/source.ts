/** The decoded preview and its display are independent of the retained bytes. */
export interface BodySourcePreview {
  key: string;
  text: string;
  originalText: string | null;
  mediaType: string | null;
  formatted: boolean;
  truncated: boolean;
}
export type SourceLanguage = "plaintext" | "json" | "html" | "xml" | "css" | "javascript";
/** Larger loaded previews retain a scrollable plain-text surface. */
export const SOURCE_EDITOR_BYTES = 1024 * 1024;
export function sourceLanguage(mediaType: string | null, formatted: boolean): SourceLanguage {
  if (formatted) {
    return "json";
  }
  const mime = (mediaType ?? "").split(";")[0]!.trim().toLowerCase();
  if (mime === "application/json" || mime.endsWith("+json")) {
    return "json";
  }
  if (mime === "text/html" || mime === "application/xhtml+xml") {
    return "html";
  }
  if (mime === "text/xml" || mime === "application/xml" || mime.endsWith("+xml")) {
    return "xml";
  }
  if (mime === "text/css") {
    return "css";
  }
  if (/(?:java|ecma)script/.test(mime)) {
    return "javascript";
  }
  return "plaintext";
}
