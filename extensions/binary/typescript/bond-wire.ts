/** Bond's published Compact Binary tags and header layout (no schema is inferred). */
export enum Type {
  Stop = 0,
  StopBase = 1,
  Bool = 2,
  UInt8 = 3,
  UInt16 = 4,
  UInt32 = 5,
  UInt64 = 6,
  Float32 = 7,
  Float64 = 8,
  String = 9,
  Struct = 10,
  List = 11,
  Set = 12,
  Map = 13,
  Int8 = 14,
  Int16 = 15,
  Int32 = 16,
  Int64 = 17,
  WString = 18,
}
export enum Version {
  V1 = 1,
  V2 = 2,
}
export const TYPE_NAMES = [
  "stop",
  "stop-base",
  "bool",
  "uint8",
  "uint16",
  "uint32",
  "uint64",
  "float",
  "double",
  "string",
  "struct",
  "list",
  "set",
  "map",
  "int8",
  "int16",
  "int32",
  "int64",
  "wstring",
] as const;
export const SUMMARY =
  "Schema-free Compact Binary. Field IDs and wire types are available; field names, enum labels and schema aliases are unknown.";
export const PROTOCOL_MAGIC = [0x43, 0x42] as const;
export const PROTOCOL_HEADER_BYTES = 4;
export const PROTOCOL_VERSION_OFFSET = PROTOCOL_MAGIC.length;
export const TYPE_TAG_MASK = 0x1f;
export const FIELD_ID_SHIFT = 5;
export const CONTAINER_COUNT_SHIFT = FIELD_ID_SHIFT;
export const EXTENDED_FIELD_ID8 = 6;
export const EXTENDED_FIELD_ID16 = 7;
export const BITS_PER_BYTE = 8;
export const UINT16_BYTES = 2;
export const LENGTH_BITS = 32;
export const FLOAT32_BYTES = 4;
export const FLOAT64_BYTES = 8;
export const INT8_SIGN_BIT = 0x80;
export const UINT8_MODULUS = 0x100;
export const VARINT_GROUP_BITS = 7;
export const VARINT_DATA_MASK = 0x7f;
export const VARINT_CONTINUATION_BIT = 0x80;

export function integerBits(kind: number): 16 | 32 | 64 {
  return kind === Type.UInt16 || kind === Type.Int16
    ? 16
    : kind === Type.UInt32 || kind === Type.Int32
      ? 32
      : 64;
}
export function isSignedInteger(kind: number): boolean {
  return kind >= Type.Int16 && kind <= Type.Int64;
}
export function fixedWidth(kind: number): number {
  return [Type.Bool, Type.UInt8, Type.Int8].includes(kind)
    ? 1
    : kind === Type.Float32
      ? FLOAT32_BYTES
      : kind === Type.Float64
        ? FLOAT64_BYTES
        : 0;
}
