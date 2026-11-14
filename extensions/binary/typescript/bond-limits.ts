import { MEBIBYTE } from "./limits.js";
/** BOND input, compact-index and visible-document safeguards. */
export const MAX_BOND_INPUT_BYTES = 16 * MEBIBYTE;
export const MAX_INDEX_VALUES = 500_000;
export const MAX_WIRE_DEPTH = 64;
export const INDEX_BLOCK_ROWS = 4096;
export const RANGE_PAGE_SIZE = 64;
export const RANGE_FANOUT = 16;
export const PROJECTION_NODE_THRESHOLD = 1500;
export const MAX_PROJECTION_TEXT_UNITS = 32_768;
export const FLATTEN_AT_TREE_DEPTH = 12;
export const MAX_AUTOMATIC_CHILDREN = 6;
