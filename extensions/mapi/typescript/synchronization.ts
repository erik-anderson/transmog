import { globsetCommands, wireSizes } from "./wire.js";
import { bounded, nested } from "./common.js";
import { enums } from "./constants.js";
import { ParseError } from "./model.js";
import type { RecordReader } from "./record.js";

const integerBitWidth = 32;
const maximumByte = 255;

function globset(record: RecordReader): void {
  const prefixes: number[] = [];
  let commonLength = 0;
  while (record.reader.remaining !== 0) {
    const command = record.integer("Command", "u8");
    if (command === globsetCommands.end) {
      if (prefixes.length !== 0) {
        throw new ParseError("malformed", "Unbalanced GLOBSET prefix stack", record.reader.range());
      }
      return;
    }
    if (command >= 1 && command <= wireSizes.globalCounter) {
      if (commonLength + command > wireSizes.globalCounter) {
        throw new ParseError(
          "malformed",
          "GLOBSET prefix exceeds six bytes",
          record.reader.range(),
        );
      }
      record.bytes("CommonBytes", command);
      if (commonLength + command < wireSizes.globalCounter) {
        prefixes.push(command);
        commonLength += command;
      }
    } else if (command === globsetCommands.pop) {
      const previous = prefixes.pop();
      if (previous === undefined) {
        throw new ParseError("malformed", "GLOBSET prefix stack underflow", record.reader.range());
      }
      commonLength -= previous;
    } else if (command === globsetCommands.bitmask) {
      if (commonLength !== wireSizes.globalCounter - 1) {
        throw new ParseError(
          "malformed",
          "GLOBSET bitmask needs a five-byte prefix",
          record.reader.range(),
        );
      }
      const start = record.integer("StartValue", "u8");
      const mask = record.integer("Bitmask", "u8");
      if (start + integerBitWidth - Math.clz32(mask) > maximumByte) {
        throw new ParseError(
          "malformed",
          "GLOBSET bitmask exceeds its low-order byte",
          record.reader.range(),
        );
      }
    } else if (command === globsetCommands.range) {
      const low = record.bytes("LowValue", wireSizes.globalCounter - commonLength);
      const high = record.bytes("HighValue", wireSizes.globalCounter - commonLength);
      const different = low.findIndex((value, index) => value !== high[index]);
      if (different !== -1 && low[different]! > high[different]!) {
        throw new ParseError("malformed", "GLOBSET range is inverted", record.reader.range());
      }
    } else {
      throw new ParseError(
        "malformed",
        `Unknown GLOBSET command 0x${command.toString(16)}`,
        record.reader.range(),
      );
    }
  }
  throw new ParseError(
    "malformed",
    "GLOBSET has no end command",
    record.reader.range(),
    "truncated",
  );
}

function xid(record: RecordReader, length: number): void {
  bounded(record, "XID", length, (value) => {
    value.scalar("NamespaceGuid", "guid");
    value.bytes("LocalId", value.reader.remaining);
  });
}

/** Returns false for ordinary binary properties, whose bytes remain a leaf. */
export function readSynchronizationBinary(
  record: RecordReader,
  id: number,
  length: number,
): boolean {
  const property = enums.PidTagPropertyEnum!.values;
  if (
    [property.PidTagSourceKey, property.PidTagParentSourceKey, property.PidTagChangeKey].includes(
      id,
    )
  ) {
    xid(record, length);
    return true;
  }
  if (id === property.PidTagPredecessorChangeList) {
    bounded(record, "Predecessor change list", length, (list) => {
      while (list.reader.remaining !== 0) {
        const size = list.integer("XidSize", "u8");
        xid(list, size);
      }
    });
    return true;
  }
  const guidSets = [
    property.MetaTagIdsetGiven,
    property.MetaTagCnsetSeen,
    property.MetaTagCnsetSeenFAI,
    property.MetaTagCnsetRead,
  ];
  const replicaSets = [
    property.MetaTagIdsetRead,
    property.MetaTagIdsetUnread,
    property.MetaTagIdsetDeleted,
    property.MetaTagIdsetNoLongerInScope,
    property.MetaTagIdsetExpired,
  ];
  if (guidSets.includes(id) || replicaSets.includes(id)) {
    bounded(record, "Identifier set", length, (set) => {
      while (set.reader.remaining !== 0) {
        nested(set, "Replica", (replica) => {
          replica.scalar("Replica", guidSets.includes(id) ? "guid" : "u16");
          nested(replica, "GLOBSET", globset);
        });
      }
    });
    return true;
  }
  return false;
}
