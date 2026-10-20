import {
  bounded,
  nested,
  readEnum,
  readFields,
  readLongTermId,
  readObjectId,
  repeat,
} from "./common.js";
import { enums } from "./constants.js";
import { ParseError } from "./model.js";
import type { ByteRange, DerivedSource, ParsedField } from "./model.js";
import { readPropertyTag, readPropertyValue } from "./properties.js";
import { BinaryReader } from "./reader.js";
import { RecordReader } from "./record.js";
import { readSynchronizationBinary } from "./synchronization.js";

interface Fragment {
  readonly bytes: readonly number[];
  readonly range: ByteRange;
}

const markers = new Set(Object.values(enums.Markers!.values));
const metaProperties = new Set(Object.values(enums.MetaProperties!.values));

function uint32Ahead(reader: BinaryReader): number {
  return (
    reader.peekUInt8() +
    reader.peekUInt8(1) * 256 +
    reader.peekUInt8(2) * 65536 +
    reader.peekUInt8(3) * 16777216
  );
}

function readVariableValue(record: RecordReader, type: number, id?: number): void {
  const length = record.integer("Length", "i32");
  if (id !== undefined && readSynchronizationBinary(record, id, length)) {
    return;
  }
  bounded(record, "Value", length, (value) => {
    if (type === 0x001f || type === enums.CodePageType!.values.PtypCodePageUnicode) {
      value.string("String", true, length);
    } else if (type === enums.CodePageType!.values.PtypCodePageUnicodeBigendian) {
      value.encodedString("String", "utf-16be", length);
    } else if (type === enums.CodePageType!.values.PtypCodePageWesternEuropean) {
      value.encodedString("String", "windows-1252", length);
    } else if (type === 0x001e) {
      value.string("String", false, length);
    } else {
      value.bytes("Bytes", length);
    }
  });
}

function readElement(record: RecordReader): void {
  const tag = uint32Ahead(record.reader);
  if (markers.has(tag)) {
    readEnum(record, "Marker", "Markers");
    return;
  }
  if (metaProperties.has(tag)) {
    const property = readPropertyTag(record);
    if (property.id === enums.PidTagPropertyEnum!.values.MetaTagDnPrefix) {
      record.string("Value", false);
    } else if (property.id === enums.PidTagPropertyEnum!.values.MetaTagNewFXFolder) {
      nested(record, "Folder replica information", (folder) => {
        readFields(folder, { Flags: "u32", Depth: "u32" });
        nested(folder, "FolderLongTermId", readLongTermId);
        const count = folder.integer("ServerDNCount", "u32");
        folder.integer("CheapServerDNCount", "u32");
        repeat(folder, count, () => folder.string("ServerDN", false));
      });
    } else if (property.id === enums.PidTagPropertyEnum!.values.MetaTagEcWarning) {
      readEnum(record, "Value", "ErrorCodes");
    } else {
      record.integer("Value", "u32");
    }
    return;
  }

  const property = readPropertyTag(record);
  if (property.id >= 0x8000) {
    nested(record, "Named property", (name) => {
      name.scalar("PropertySet", "guid");
      const kind = readEnum(name, "Kind", "KindEnum");
      if (kind === 0) {
        name.integer("Dispid", "u32");
      } else if (kind === 1) {
        name.string("Name", true);
      } else {
        throw new ParseError("malformed", "Invalid named property kind", name.reader.range());
      }
    });
  }

  if ((property.type & 0x1000) !== 0) {
    const count = record.integer("Length", "i32");
    const elementType = property.type & ~0x1000;
    repeat(record, count, () =>
      nested(record, "Value", (value) => {
        if (
          [0x001e, 0x001f, 0x0102].includes(elementType) ||
          Object.values(enums.CodePageType!.values).includes(elementType)
        ) {
          readVariableValue(value, elementType);
        } else {
          readPropertyValue(value, elementType, "fast-transfer");
        }
      }),
    );
  } else if (
    [0x000d, 0x001e, 0x001f, 0x00fb, 0x0102].includes(property.type) ||
    Object.values(enums.CodePageType!.values).includes(property.type) ||
    property.id === enums.PidTagPropertyEnum!.values.MetaTagIdsetGiven
  ) {
    readVariableValue(record, property.type, property.id);
  } else if (property.type === 0x000b) {
    record.boolean("Value", "u16");
  } else if (
    property.type === 0x0014 &&
    [
      enums.PidTagPropertyEnum!.values.PidTagMid,
      enums.PidTagPropertyEnum!.values.PidTagFolderId,
      0x6714,
    ].includes(property.id)
  ) {
    nested(record, "Identifier", readObjectId);
  } else {
    readPropertyValue(record, property.type, "fast-transfer");
  }
}

function remainingFragments(fragments: readonly Fragment[], consumed: number): Fragment[] {
  const result: Fragment[] = [];
  for (const fragment of fragments) {
    const skip = Math.min(consumed, fragment.bytes.length);
    consumed -= skip;
    if (skip !== fragment.bytes.length) {
      result.push({
        bytes: fragment.bytes.slice(skip),
        range: {
          ...fragment.range,
          offset: fragment.range.offset + skip,
          length: fragment.range.length - skip,
        },
      });
    }
  }
  return result;
}

/** A stream has one pending wire element, never a partial .NET parser object. */
export class FastTransferDecoder {
  readonly derivedSources: DerivedSource[] = [];
  private readonly pending = new Map<string, readonly Fragment[]>();
  private assemblyWork = 0;

  read(record: RecordReader, length: number, operation: string, handle: number): void {
    const chunk = record.reader.child(length);
    const key = `${operation}:${handle}`;
    const previous = this.pending.get(key);
    let reader = chunk;
    let fragments: readonly Fragment[];
    if (previous?.length) {
      const current = chunk.take(chunk.remaining);
      fragments = [...previous, { bytes: current, range: chunk.range(chunk.start, length) }];
      const count = fragments.reduce((total, fragment) => total + fragment.bytes.length, 0);
      this.assemblyWork += count;
      if (this.assemblyWork > 8 * 1024 * 1024 || this.derivedSources.length >= 128) {
        throw new ParseError("limit", "FastTransfer assembly limit exceeded", chunk.range());
      }
      const bytes = fragments.flatMap((fragment) => Array.from(fragment.bytes));
      const id = `mapi-transfer.${this.derivedSources.length}`;
      this.derivedSources.push({
        id,
        bytes,
        parents: fragments.map((fragment) => fragment.range),
        transform: "FastTransfer continuation (concatenated)",
        exactMapping: false,
      });
      reader = new BinaryReader(bytes, id, chunk.budget);
    } else {
      const bytes = chunk.fork().take(chunk.remaining);
      fragments = [{ bytes, range: chunk.range(chunk.start, length) }];
    }
    this.pending.delete(key);
    const output = new RecordReader(reader, "FastTransfer stream");
    try {
      while (reader.remaining !== 0) {
        const attempt = reader.fork();
        const element = new RecordReader(attempt, "Stream element");
        try {
          readElement(element);
        } catch (error) {
          if (!(error instanceof ParseError) || error.reason !== "truncated") {
            throw error;
          }
          const consumed = reader.offset - reader.start;
          this.pending.set(key, remainingFragments(fragments, consumed));
          const issue: ParsedField = {
            name: "Incomplete stream element",
            type: "FastTransfer fragment",
            range: reader.range(reader.offset, reader.remaining),
            children: [],
            value: { kind: "bytes" },
            issue: {
              kind: "missing-context",
              message: "This FastTransfer element continues in another retained exchange.",
            },
          };
          output.add(issue);
          reader.skip(reader.remaining);
          break;
        }
        const consumed = attempt.offset - reader.offset;
        if (consumed <= 0) {
          throw new ParseError(
            "malformed",
            "FastTransfer element made no progress",
            reader.range(),
          );
        }
        reader.skip(consumed);
        output.add(element.finish());
      }
    } finally {
      record.add(output.finish());
    }
  }

  release(handle: number): void {
    for (const key of this.pending.keys()) {
      if (key.endsWith(`:${handle}`)) {
        this.pending.delete(key);
      }
    }
  }
}
