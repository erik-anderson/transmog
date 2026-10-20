import { bounded, nested, readEnum } from "./common.js";
import type { Conversation } from "./conversation.js";
import { enums } from "./constants.js";
import { ParseError } from "./model.js";
import type { DerivedSource, Direction } from "./model.js";
import { BinaryReader } from "./reader.js";
import { RecordReader } from "./record.js";
import { readRop, ropName } from "./rops.js";
import type { RopPayloadReaders } from "./rops.js";

/** MS-XCA LZ77: flags, literal bytes and overlapping backward matches. */
export function decompressLz77(reader: BinaryReader, expectedLength: number): number[] {
  if (
    !Number.isSafeInteger(expectedLength) ||
    expectedLength < 0 ||
    expectedLength > 8 * 1024 * 1024
  ) {
    throw new ParseError("limit", "Decompressed buffer size limit exceeded", reader.range());
  }
  const output: number[] = [];
  let flags = 0;
  let remainingFlags = 0;
  let highNibble: number | undefined;
  while (reader.remaining !== 0) {
    reader.budget.step(reader.range());
    if (remainingFlags === 0) {
      flags = reader.integer("u32");
      remainingFlags = 32;
      continue;
    }
    const match = (flags & 0x80000000) !== 0;
    flags = flags << 1;
    remainingFlags--;
    if (!match) {
      if (output.length >= expectedLength) {
        throw new ParseError(
          "malformed",
          "Literal exceeds decompressed buffer size",
          reader.range(),
        );
      }
      output.push(reader.integer("u8"));
      continue;
    }
    const token = reader.integer("u16");
    const distance = (token >> 3) + 1;
    let length = (token & 7) + 3;
    if (length === 10) {
      if (highNibble === undefined) {
        const byte = reader.integer("u8");
        length += byte & 15;
        highNibble = byte >> 4;
      } else {
        length += highNibble;
        highNibble = undefined;
      }
    }
    if (length === 25) {
      length += reader.integer("u8");
    }
    if (length === 280) {
      length = reader.integer("u16") + 3;
    }
    if (distance > output.length || length > expectedLength - output.length) {
      throw new ParseError("malformed", "Invalid LZ77 backward match", reader.range());
    }
    for (let index = 0; index < length; index++) {
      output.push(output[output.length - distance]!);
    }
  }
  if (output.length !== expectedLength) {
    throw new ParseError(
      "malformed",
      "Decompressed size does not match RPC header",
      reader.range(),
    );
  }
  return output;
}

export class RpcDecoder {
  readonly derivedSources: DerivedSource[] = [];
  private derivedBytes = 0;

  constructor(
    private readonly conversation: Conversation,
    private readonly payloadReaders: RopPayloadReaders,
  ) {}

  read(record: RecordReader, length: number, auxiliary: boolean, direction: Direction): void {
    bounded(record, auxiliary ? "AuxiliaryBuffer" : "RopBuffer", length, (buffers) => {
      while (buffers.reader.remaining !== 0) {
        nested(buffers, "RPC buffer", (buffer) => {
          let flags = 0;
          let size = 0;
          let actualSize = 0;
          nested(buffer, "RPC_HEADER_EXT", (header) => {
            header.integer("Version", "u16");
            flags = readEnum(header, "Flags", "RpcHeaderFlags");
            size = header.integer("Size", "u16");
            actualSize = header.integer("SizeActual", "u16");
          });
          const payload = this.payload(buffer.reader, flags, size, actualSize);
          const decoded = new RecordReader(
            payload,
            auxiliary ? "Auxiliary payload" : "ROP payload",
          );
          if (auxiliary) {
            this.readAuxiliary(decoded);
          } else {
            this.readRops(decoded, direction);
          }
          buffer.add(decoded.finish());
        });
      }
    });
  }

  private payload(
    reader: BinaryReader,
    flags: number,
    size: number,
    actualSize: number,
  ): BinaryReader {
    const parent = reader.range(reader.offset, size);
    const encoded = reader.child(size);
    if ((flags & 3) === 0) {
      if (size !== actualSize) {
        throw new ParseError("malformed", "Plain RPC payload sizes differ", parent);
      }
      return encoded;
    }
    let bytes = encoded.take(encoded.remaining);
    if ((flags & 2) !== 0) {
      bytes = bytes.map((value) => value ^ 0xa5);
    }
    if ((flags & 1) !== 0) {
      bytes = decompressLz77(new BinaryReader(bytes, parent.sourceId, reader.budget), actualSize);
    } else if (bytes.length !== actualSize) {
      throw new ParseError("malformed", "XOR payload size differs from RPC header", parent);
    }
    this.derivedBytes += bytes.length;
    if (this.derivedSources.length >= 128 || this.derivedBytes > 8 * 1024 * 1024) {
      throw new ParseError("limit", "Derived RPC source limit exceeded", parent);
    }
    const id = `mapi-derived.${this.derivedSources.length}`;
    const transforms: string[] = [];
    if ((flags & 2) !== 0) {
      transforms.push("MAPI XOR 0xA5");
    }
    if ((flags & 1) !== 0) {
      transforms.push("MAPI LZ77");
    }
    this.derivedSources.push({
      id,
      bytes,
      parents: [parent],
      transform: transforms.join(" then "),
      exactMapping: (flags & 1) === 0,
    });
    return new BinaryReader(bytes, id, reader.budget);
  }

  private readRops(record: RecordReader, direction: Direction): void {
    const start = record.reader.offset;
    const size = record.integer("RopSize", "u16");
    if (size < 2 || size > record.reader.end - start) {
      throw new ParseError("malformed", "Invalid ROP list size", record.reader.range());
    }
    const rops = record.reader.child(size - 2);
    const handles: number[] = [];
    const handleFields = new RecordReader(record.reader, "ServerObjectHandleTable");
    if (record.reader.remaining % 4 !== 0) {
      throw new ParseError("malformed", "Incomplete server handle table", record.reader.range());
    }
    while (record.reader.remaining !== 0) {
      handles.push(handleFields.integer("Handle", "u32"));
    }
    const operations = new RecordReader(rops, "Operations");
    while (rops.remaining !== 0) {
      const position = rops.offset;
      nested(operations, ropName(rops.peekUInt8(), direction), (operation) => {
        readRop(operation, direction, handles, this.conversation, this.payloadReaders);
      });
      if (rops.offset <= position) {
        throw new ParseError("malformed", "ROP did not consume input", rops.range());
      }
    }
    record.add(operations.finish());
    record.add(handleFields.finish());
  }

  private readAuxiliary(record: RecordReader): void {
    while (record.reader.remaining !== 0) {
      const size = record.reader.peekUInt8() + record.reader.peekUInt8(1) * 256;
      if (size < 4) {
        throw new ParseError(
          "malformed",
          "Auxiliary block is smaller than its header",
          record.reader.range(),
        );
      }
      bounded(record, "Auxiliary block", size, (block) => {
        block.integer("Size", "u16");
        const version = readEnum(block, "Version", "PayloadDataVersion");
        const enumType = version === 1 ? "AuxiliaryBlockType_1" : "AuxiliaryBlockType_2";
        const type = readEnum(block, "Type", enumType);
        const name = Object.entries(enums[enumType]!.values).find(
          ([, value]) => value === type,
        )?.[0];
        if (name === "AUX_TYPE_ENDPOINT_CAPABILITIES") {
          readEnum(block, "EndpointCapabilityFlag", "EndpointCapabilityFlag");
        } else {
          block.bytes("Auxiliary data", block.reader.remaining);
          block.field.issue = {
            kind: "unsupported",
            message: `Unsupported auxiliary block ${name ?? type}`,
          };
        }
      });
    }
  }
}
