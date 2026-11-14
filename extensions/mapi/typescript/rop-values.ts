import { wireSizes, stringTypes, recipientFlags, recipientAddressTypes } from "./wire.js";
import type { ScalarType } from "./record.js";
import type { WireLayout } from "./rop-layouts.js";
import { bounded, nested, readEnum, repeat, readObjectId, readLongTermId } from "./common.js";
import { ParseError } from "./model.js";
import { readPropertyTag, readPropertyRow, readRestriction } from "./properties.js";
import type { PropertyTag } from "./properties.js";
import type { RecordReader } from "./record.js";

export function tags(record: RecordReader, count: number): PropertyTag[] {
  const result: PropertyTag[] = [];
  repeat(
    record,
    count,
    () => nested(record, "PropertyTag", (child) => result.push(readPropertyTag(child))),
    wireSizes.propertyTag,
  );
  return result;
}

export function sizedBytes(record: RecordReader, name: string, wide = false): void {
  const size = record.integer(`${name}Size`, wide ? "u32" : "u16");
  record.bytes(name, size);
}

export function countedIds(record: RecordReader, count: number, type: string): void {
  repeat(record, count, () => nested(record, type, readObjectId), wireSizes.objectId);
}

export function restriction(record: RecordReader, size: number): void {
  if (size !== 0) {
    bounded(record, "RestrictionData", size, readRestriction);
  }
}

export function typedString(record: RecordReader): void {
  const type = readEnum(record, "StringType", "StringTypeEnum");
  if (type === stringTypes.ascii || type === stringTypes.reducedUnicode) {
    record.string("String", false);
  } else if (type === stringTypes.unicode) {
    record.string("String", true);
  } else if (type !== stringTypes.absent && type !== stringTypes.empty) {
    throw new ParseError("malformed", "Unknown typed string encoding", record.reader.range());
  }
}

export function readRecipient(
  record: RecordReader,
  size: number,
  columns: readonly PropertyTag[],
): void {
  bounded(record, "Recipient", size, (child) => {
    const flags = child.integer("RecipientFlags", "u16");
    const addressType = (flags >> recipientFlags.addressTypeShift) & recipientFlags.addressTypeMask;
    if (addressType === recipientAddressTypes.exchange) {
      child.integer("AddressPrefixUsed", "u8");
      readEnum(child, "DisplayType", "DisplayType");
      child.string("X500DN", false);
    } else if (
      addressType === recipientAddressTypes.personalDistributionList ||
      addressType === recipientAddressTypes.distributionList
    ) {
      sizedBytes(child, "EntryID");
      sizedBytes(child, "SearchKey");
    } else if (
      addressType === recipientAddressTypes.none &&
      (flags & recipientFlags.hasAddressType) !== 0
    ) {
      child.string("AddressType", false);
    }
    const wide = (flags & recipientFlags.unicode) !== 0;
    for (const [bit, name] of [
      [recipientFlags.emailAddress, "EmailAddress"],
      [recipientFlags.displayName, "DisplayName"],
      [recipientFlags.simpleDisplayName, "SimpleDisplayName"],
      [recipientFlags.transmittableDisplayName, "TransmittableDisplayName"],
    ] as const) {
      if ((flags & bit) !== 0) {
        child.string(name, wide);
      }
    }
    const count = child.integer("RecipientColumnCount", "u16");
    if (count > columns.length) {
      throw new ParseError(
        "malformed",
        "Recipient column count exceeds declared columns",
        child.reader.range(),
      );
    }
    nested(child, "RecipientProperties", (row) =>
      readPropertyRow(row, columns.slice(0, count), false),
    );
  });
}

const scalarTypes = new Set([
  "u8",
  "i8",
  "u16",
  "i16",
  "u32",
  "i32",
  "u64",
  "i64",
  "f32",
  "f64",
  "bool",
  "guid",
]);
export function readLayout(record: RecordReader, layout: WireLayout): Map<string, number> {
  const values = new Map<string, number>();
  for (const [name, type] of Object.entries(layout)) {
    if (scalarTypes.has(type)) {
      const value = record.scalar(name, type as ScalarType);
      if (typeof value === "number" || typeof value === "boolean") {
        values.set(name, Number(value));
      }
    } else if (type === "bool16") {
      values.set(name, Number(record.boolean(name, "u16")));
    } else if (type === "FolderID" || type === "MessageID") {
      nested(record, name, readObjectId, type);
    } else if (type === "LongTermID") {
      nested(record, name, readLongTermId, type);
    } else if (type === "PropertyTag") {
      nested(record, name, readPropertyTag);
    } else {
      values.set(name, readEnum(record, name, type));
    }
  }
  return values;
}
