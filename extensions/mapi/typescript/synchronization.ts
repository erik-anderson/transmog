import { bounded, nested } from "./common.js";
import { enums } from "./constants.js";
import { ParseError } from "./model.js";
import type { RecordReader } from "./record.js";

function globset(record: RecordReader): void {
  const prefixes: number[] = [];
  let commonLength = 0;
  while (record.reader.remaining !== 0) {
    const command = record.integer("Command", "u8");
    if (command === 0) {
      if (prefixes.length !== 0) {
        throw new ParseError("malformed", "Unbalanced GLOBSET prefix stack", record.reader.range());
      }
      return;
    }
    if (command >= 1 && command <= 6) {
      if (commonLength + command > 6) {
        throw new ParseError(
          "malformed",
          "GLOBSET prefix exceeds six bytes",
          record.reader.range(),
        );
      }
      record.bytes("CommonBytes", command);
      if (commonLength + command < 6) {
        prefixes.push(command);
        commonLength += command;
      }
    } else if (command === 0x50) {
      const previous = prefixes.pop();
      if (previous === undefined) {
        throw new ParseError("malformed", "GLOBSET prefix stack underflow", record.reader.range());
      }
      commonLength -= previous;
    } else if (command === 0x42) {
      if (commonLength !== 5) {
        throw new ParseError(
          "malformed",
          "GLOBSET bitmask needs a five-byte prefix",
          record.reader.range(),
        );
      }
      const start = record.integer("StartValue", "u8");
      const mask = record.integer("Bitmask", "u8");
      if (start + 32 - Math.clz32(mask) > 255) {
        throw new ParseError(
          "malformed",
          "GLOBSET bitmask exceeds its low-order byte",
          record.reader.range(),
        );
      }
    } else if (command === 0x52) {
      const low = record.bytes("LowValue", 6 - commonLength);
      const high = record.bytes("HighValue", 6 - commonLength);
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
