import { bounded, filetime, nested, readEnum, readObjectId, repeat } from "./common.js";
import { enums } from "./constants.js";
import { ParseError } from "./model.js";
import type { RecordReader, ScalarType } from "./record.js";
import { readRuleActions } from "./rules.js";

export type PropertyContext = "rop" | "address-book" | "fast-transfer" | "extended-rule";

export interface PropertyTag {
  readonly type: number;
  readonly id: number;
}

const scalarProperties: Readonly<Record<number, ScalarType>> = {
  0x0002: "i16",
  0x0003: "i32",
  0x0004: "f32",
  0x0005: "f64",
  0x0006: "i64",
  0x0007: "f64",
  0x000b: "bool",
  0x0014: "i64",
  0x0040: "u64",
  0x0048: "guid",
};

export function readPropertyTag(record: RecordReader): PropertyTag {
  return {
    type: readEnum(record, "PropertyType", "PropertyDataType"),
    id: readEnum(record, "PropertyId", "PidTagPropertyEnum"),
  };
}

export function readPropertyTags(record: RecordReader, large: boolean): PropertyTag[] {
  const count = record.integer("PropertyTagCount", large ? "u32" : "u16");
  const tags: PropertyTag[] = [];
  repeat(
    record,
    count,
    () => {
      nested(record, "PropertyTag", (child) => tags.push(readPropertyTag(child)));
    },
    4,
  );
  return tags;
}

function propertyName(type: number): string {
  return (
    Object.entries(enums.PropertyDataType!.values).find(
      ([, candidate]) => candidate === type,
    )?.[0] ?? `Property 0x${type.toString(16)}`
  );
}

export function readPropertyValue(
  record: RecordReader,
  type: number,
  context: PropertyContext = "rop",
  addressBookElement = false,
): void {
  if ((type & 0x1000) !== 0) {
    const count = record.integer("Count", "u32");
    repeat(record, count, () => {
      nested(record, propertyName(type & ~0x1000), (child) => {
        readPropertyValue(child, type & ~0x1000, context, addressBookElement);
      });
    });
    return;
  }

  const scalar = scalarProperties[type];
  if (scalar) {
    const value = record.scalar("Value", scalar);
    if (type === 0x0040) {
      const timestamp = filetime(value as bigint);
      if (timestamp) {
        record.field.children.at(-1)!.timestamp = timestamp;
      }
    }
    return;
  }

  switch (type) {
    case 0x0000:
    case 0x0001:
    case 0x000d:
      return;
    case 0x000a:
      readEnum(record, "Value", "ErrorCodes");
      return;
    case 0x001e:
    case 0x001f:
      if (!addressBookElement || record.boolean("HasValue")) {
        record.string("Value", type === 0x001f);
      }
      return;
    case 0x0102: {
      if (addressBookElement && !record.boolean("HasValue")) {
        return;
      }
      const count = record.integer("Count", context === "rop" ? "u16" : "u32");
      record.bytes("Value", count);
      return;
    }
    case 0x00fb: {
      const count = record.integer("Count", "u16");
      bounded(record, "ServerId", count, (child) => {
        if (child.integer("Ours", "u8") === 1) {
          nested(child, "FolderID", readObjectId);
          nested(child, "MessageID", readObjectId);
          child.integer("Instance", "u32");
        } else {
          child.bytes("ClientData", child.reader.remaining);
        }
      });
      return;
    }
    case 0x00fd:
      readRestriction(record, context);
      return;
    case 0x00fe:
      readRuleActions(record, context);
      return;
    default:
      throw new ParseError(
        "unsupported",
        `Unsupported property type 0x${type.toString(16)}`,
        record.reader.range(),
      );
  }
}

export function readAddressBookValue(record: RecordReader, type: number): void {
  const hasPresence = (type & 0x1000) !== 0 || [0x001e, 0x001f, 0x0102].includes(type);
  if (!hasPresence || record.boolean("HasValue")) {
    nested(record, propertyName(type), (child) => {
      // Only multivalued variable-length elements have their own presence byte.
      readPropertyValue(child, type, "address-book", (type & 0x1000) !== 0);
    });
  }
}

export function readTaggedValue(record: RecordReader, context: PropertyContext = "rop"): void {
  const tag = readPropertyTag(record);
  nested(record, propertyName(tag.type), (child) => {
    if (context === "address-book") {
      readAddressBookValue(child, tag.type);
    } else {
      readPropertyValue(child, tag.type, context);
    }
  });
}

export function readPropertyList(record: RecordReader, addressBook: boolean): void {
  const count = record.integer("PropertyValueCount", addressBook ? "u32" : "u16");
  repeat(
    record,
    count,
    () => {
      nested(record, "TaggedPropertyValue", (child) => {
        readTaggedValue(child, addressBook ? "address-book" : "rop");
      });
    },
    4,
  );
}

export function readPropertyRow(
  record: RecordReader,
  tags: readonly PropertyTag[],
  addressBook: boolean,
): void {
  const flag = record.integer(addressBook ? "Flags" : "Flag", "u8");
  if (flag !== 0 && flag !== 1) {
    throw new ParseError("malformed", "Unknown property row flag", record.reader.range());
  }
  repeat(
    record,
    tags.length,
    (index) => {
      const tag = tags[index]!;
      nested(record, `Property 0x${tag.id.toString(16)}`, (child) => {
        let type = tag.type & ~0x2000;
        if (type === 0) {
          type = readEnum(child, "PropertyType", "PropertyDataType");
        }
        if (flag === 1) {
          const valueFlag = child.integer("Flag", "u8");
          if (valueFlag === 1) {
            return;
          }
          if (valueFlag === 0x0a) {
            type = 0x000a;
          } else if (valueFlag !== 0) {
            throw new ParseError("malformed", "Unknown property value flag", child.reader.range());
          }
        }
        if (addressBook) {
          readAddressBookValue(child, type);
        } else {
          readPropertyValue(child, type);
        }
      });
    },
    0,
  );
}

export function readNamedProperty(record: RecordReader): void {
  const kind = readEnum(record, "Kind", "KindEnum");
  if (kind === 0 || kind === 1) {
    record.scalar("GUID", "guid");
    if (kind === 0) {
      record.integer("LID", "u32");
    } else {
      const size = record.integer("NameSize", "u8");
      record.string("Name", true, size);
    }
  }
}

export function readRestriction(record: RecordReader, context: PropertyContext = "rop"): void {
  const type = readEnum(record, "RestrictType", "RestrictTypeEnum");
  const readChild = (name: string) =>
    nested(record, name, (child) => readRestriction(child, context));
  const readTag = (name: string) => {
    let tag: PropertyTag | undefined;
    nested(record, name, (child) => {
      tag = readPropertyTag(child);
    });
    return tag!;
  };
  switch (type) {
    case 0:
    case 1: {
      const count = record.integer("RestrictCount", context === "rop" ? "u16" : "u32");
      repeat(record, count, () => readChild("Restriction"));
      return;
    }
    case 2:
      readChild("Restriction");
      return;
    case 3:
    case 4: {
      if (type === 3) {
        readEnum(record, "FuzzyLevelLow", "FuzzyLevelLowEnum");
        readEnum(record, "FuzzyLevelHigh", "FuzzyLevelHighEnum");
      } else {
        readEnum(record, "RelOp", "RelOpType");
      }
      const tag = readTag(type === 3 ? "PropertyTag" : "PropTag");
      nested(record, "TaggedValue", (child) => {
        readPropertyTag(child);
        readPropertyValue(child, tag.type & ~0x1000, context);
      });
      return;
    }
    case 5:
      readEnum(record, "RelOp", "RelOpType");
      readTag("PropTag1");
      readTag("PropTag2");
      return;
    case 6:
      readEnum(record, "BitmapRelOp", "BitmapRelOpType");
      readTag("PropTag");
      record.integer("Mask", "u32");
      return;
    case 7:
      readEnum(record, "RelOp", "RelOpType");
      readTag("PropTag");
      record.integer("Size", "u32");
      return;
    case 8:
      readTag("PropTag");
      return;
    case 9:
      readTag("Subobject");
      readChild("Restriction");
      return;
    case 10: {
      const count = record.integer("TaggedValuesCount", "u8");
      repeat(
        record,
        count,
        () => nested(record, "TaggedValue", (child) => readTaggedValue(child, context)),
        4,
      );
      if (record.boolean("RestrictionPresent")) {
        readChild("Restriction");
      }
      return;
    }
    case 11:
      record.integer("Count", "u32");
      readChild("SubRestriction");
      return;
    default:
      throw new ParseError(
        "unsupported",
        `Unsupported restriction type ${type}`,
        record.reader.range(),
      );
  }
}

/** Used by protocol fixture tests without exposing a test hook to the host. */
export function parsePropertyFixture(
  record: RecordReader,
  type: string,
  arguments_: readonly unknown[],
): boolean {
  if (type === "PropertyTag") {
    readPropertyTag(record);
  } else if (type === "RestrictionType") {
    readRestriction(record, arguments_[0] === 0 ? "rop" : "address-book");
  } else if (type === "PtypBooleanShort") {
    record.boolean("Value", "i16");
  } else {
    const dataType = enums.PropertyDataType!.values[type];
    if (dataType === undefined) {
      return false;
    }
    if (type === "PtypBinary") {
      readPropertyValue(
        record,
        dataType,
        arguments_[0] === 0 ? "rop" : "address-book",
        arguments_[1] === true,
      );
    } else {
      readPropertyValue(record, dataType, "rop", arguments_[0] === true);
    }
  }
  return true;
}
