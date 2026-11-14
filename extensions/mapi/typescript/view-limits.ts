/** Leave room beneath API 1's document limits for encoding and metadata. */
const mebibyte = 1024 * 1024;
export const displayLimits = {
  nodes: 18_000,
  textBytes: 2 * mebibyte,
  sources: 128,
  derivedBytes: 2 * mebibyte,
  serializedBytes: 7 * mebibyte,
  depth: 32,
  eagerDepth: 6,
  eagerChildren: 6,
  minimumNodes: 128,
  minimumTextBytes: 16_384,
  fieldName: 384,
  protocolName: 128,
  enumName: 256,
  flagNames: 64,
  diagnosticCode: 128,
  diagnosticMessage: 2048,
  diagnostics: 255,
  title: 512,
  operationName: 64,
  utf8BytesPerCodeUnit: 3,
} as const;
