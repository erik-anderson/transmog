/** Developer-only bridge for the reference corpus. Never packaged or imported by an entry. */
import { DecodeSession } from "./decoder-state.js";
import { enums } from "./constants.js";
import type { DecodeResult, Direction, Exchange } from "./model.js";
import { readPropertyTag, readPropertyValue, readRestriction } from "./properties.js";
import type { RecordReader } from "./record.js";
import { readRop } from "./rops.js";
export { base64 } from "./shared.js";

interface FixtureOptions {
  readonly type?: string;
  readonly arguments?: readonly unknown[];
  readonly offset?: number;
  readonly count?: number;
  readonly prepareRequest?: boolean;
}

function propertyFixture(record: RecordReader, type: string, arguments_: readonly unknown[]): void {
  if (type === "PropertyTag") {
    readPropertyTag(record);
  } else if (type === "RestrictionType") {
    readRestriction(record, arguments_[0] === 0 ? "rop" : "address-book");
  } else if (type === "PtypBooleanShort") {
    record.boolean("Value", "i16");
  } else {
    const dataType = enums.PropertyDataType!.values[type];
    if (dataType === undefined) throw new Error(`Unknown fixture type ${type}`);
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
}

export function decodeFixture(
  exchanges: readonly Exchange[],
  selectedId: number,
  direction: Direction,
  options: FixtureOptions,
): DecodeResult {
  const session = new DecodeSession(exchanges, selectedId, direction);
  session.prepare(direction === "response" || options.prepareRequest === true);
  const type = options.type?.split(".").at(-1);
  const record = session.record(
    type ?? `${session.selected.requestType} ${direction}`,
    options.offset,
    options.count || undefined,
  );
  return session.capture(record, () => {
    if (type === "ExtendedBuffer")
      session.rpc.read(record, record.reader.remaining, true, direction);
    else if (type?.startsWith("Rop"))
      readRop(record, direction, [], session.conversation, session.payloadReaders);
    else if (type) propertyFixture(record, type, options.arguments ?? []);
    else session.readMessage(record, session.selected.requestType, direction);
  });
}
