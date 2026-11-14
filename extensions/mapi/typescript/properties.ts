import {
  propertyTypes,
  restrictionTypes,
  propertyNameKinds,
  propertyRowFlags,
  wireSizes,
} from "./wire.js";
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
  [propertyTypes.integer16]: "i16",
  [propertyTypes.integer32]: "i32",
  [propertyTypes.float32]: "f32",
  [propertyTypes.float64]: "f64",
  [propertyTypes.currency]: "i64",
  [propertyTypes.floatingTime]: "f64",
  [propertyTypes.boolean]: "bool",
  [propertyTypes.integer64]: "i64",
  [propertyTypes.time]: "u64",
  [propertyTypes.guid]: "guid",
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
    wireSizes.propertyTag,
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
  if ((type & propertyTypes.multiValue) !== 0) {
    const count = record.integer("Count", "u32");
    repeat(record, count, () => {
      nested(record, propertyName(type & ~propertyTypes.multiValue), (child) => {
        readPropertyValue(child, type & ~propertyTypes.multiValue, context, addressBookElement);
      });
    });
    return;
  }

  const scalar = scalarProperties[type];
  if (scalar) {
    const { value, field } = record.scalarField("Value", scalar);
    if (type === propertyTypes.currency) {
      field.units = "1/10000 currency unit";
    }
    if (type === propertyTypes.floatingTime) {
      field.units = "OLE Automation DATE";
    }
    if (type === propertyTypes.time) {
      field.units = "100 ns since 1601-01-01 UTC";
      const timestamp = filetime(value as bigint);
      if (timestamp) {
        field.timestamp = timestamp;
      }
    }
    return;
  }

  switch (type) {
    case propertyTypes.unspecified:
    case propertyTypes.null:
    case propertyTypes.object:
      return;
    case propertyTypes.error:
      readEnum(record, "Value", "ErrorCodes");
      return;
    case propertyTypes.string8:
    case propertyTypes.string:
      if (!addressBookElement || record.boolean("HasValue")) {
        record.string("Value", type === propertyTypes.string);
      }
      return;
    case propertyTypes.binary: {
      if (addressBookElement && !record.boolean("HasValue")) {
        return;
      }
      const count = record.integer("Count", context === "rop" ? "u16" : "u32");
      record.bytes("Value", count);
      return;
    }
    case propertyTypes.serverId: {
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
    case propertyTypes.restriction:
      readRestriction(record, context);
      return;
    case propertyTypes.ruleAction:
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
  const hasPresence =
    (type & propertyTypes.multiValue) !== 0 ||
    [propertyTypes.string8, propertyTypes.string, propertyTypes.binary].includes(type);
  if (!hasPresence || record.boolean("HasValue")) {
    nested(record, propertyName(type), (child) => {
      // Only multivalued variable-length elements have their own presence byte.
      readPropertyValue(child, type, "address-book", (type & propertyTypes.multiValue) !== 0);
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
    wireSizes.propertyTag,
  );
}

export function readPropertyRow(
  record: RecordReader,
  tags: readonly PropertyTag[],
  addressBook: boolean,
): void {
  const flag = record.integer(addressBook ? "Flags" : "Flag", "u8");
  if (flag !== propertyRowFlags.ordinary && flag !== propertyRowFlags.flagged) {
    throw new ParseError("malformed", "Unknown property row flag", record.reader.range());
  }
  repeat(
    record,
    tags.length,
    (index) => {
      const tag = tags[index]!;
      nested(record, `Property 0x${tag.id.toString(16)}`, (child) => {
        let type = tag.type & ~propertyTypes.multiValueInstance;
        if (type === propertyTypes.unspecified) {
          type = readEnum(child, "PropertyType", "PropertyDataType");
        }
        if (flag === propertyRowFlags.flagged) {
          const valueFlag = child.integer("Flag", "u8");
          if (valueFlag === propertyRowFlags.absent) {
            return;
          }
          if (valueFlag === propertyRowFlags.error) {
            type = propertyTypes.error;
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
  if (kind === propertyNameKinds.none) {
    return;
  }
  if (kind !== propertyNameKinds.numeric && kind !== propertyNameKinds.string) {
    throw new ParseError(
      "unsupported",
      `Unsupported named property kind ${kind}`,
      record.reader.range(),
    );
  }
  record.scalar("GUID", "guid");
  if (kind === propertyNameKinds.numeric) {
    record.integer("LID", "u32");
  } else {
    const size = record.integer("NameSize", "u8");
    record.string("Name", true, size);
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
    case restrictionTypes.and:
    case restrictionTypes.or: {
      const count = record.integer("RestrictCount", context === "rop" ? "u16" : "u32");
      repeat(record, count, () => readChild("Restriction"));
      return;
    }
    case restrictionTypes.not:
      readChild("Restriction");
      return;
    case restrictionTypes.content:
    case restrictionTypes.property: {
      if (type === restrictionTypes.content) {
        readEnum(record, "FuzzyLevelLow", "FuzzyLevelLowEnum");
        readEnum(record, "FuzzyLevelHigh", "FuzzyLevelHighEnum");
      } else {
        readEnum(record, "RelOp", "RelOpType");
      }
      const tag = readTag(type === restrictionTypes.content ? "PropertyTag" : "PropTag");
      nested(record, "TaggedValue", (child) => {
        readPropertyTag(child);
        readPropertyValue(child, tag.type & ~propertyTypes.multiValue, context);
      });
      return;
    }
    case restrictionTypes.compareProperties:
      readEnum(record, "RelOp", "RelOpType");
      readTag("PropTag1");
      readTag("PropTag2");
      return;
    case restrictionTypes.bitMask:
      readEnum(record, "BitmapRelOp", "BitmapRelOpType");
      readTag("PropTag");
      record.integer("Mask", "u32");
      return;
    case restrictionTypes.size:
      readEnum(record, "RelOp", "RelOpType");
      readTag("PropTag");
      record.integer("Size", "u32");
      return;
    case restrictionTypes.exist:
      readTag("PropTag");
      return;
    case restrictionTypes.subObject:
      readTag("Subobject");
      readChild("Restriction");
      return;
    case restrictionTypes.comment: {
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
    case restrictionTypes.count:
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
