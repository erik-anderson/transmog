/** Finite input and materialized-output safeguards for the bundled binary readers. */
export const KIBIBYTE = 1024;
export const MEBIBYTE = KIBIBYTE * KIBIBYTE;
export const MAX_BINARY_INPUT_BYTES = 2 * MEBIBYTE;
export const MAX_INPUT_PAGE_BYTES = 64 * KIBIBYTE;
export const MAX_INLINE_DEPTH = 10;
export const MAX_INLINE_NODES = 4_000;
export const MAX_INLINE_TEXT_UNITS = 64 * KIBIBYTE;
export const MAX_TEXT_PREVIEW_UNITS = 1024;
export const MAX_INLINE_COLLECTION_ITEMS = MAX_INLINE_NODES;
export const MAX_INLINE_MAP_ENTRIES = 1_000;

export const TEXT_PREVIEW_LABEL = ` · preview (first ${MAX_TEXT_PREVIEW_UNITS.toLocaleString("en-US")} UTF-16 code units)`;
export const MAX_HEADER_VALUE_BYTES = 256;
