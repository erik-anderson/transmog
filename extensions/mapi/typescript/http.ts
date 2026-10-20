import { bounded, nested, readEnum, readFields, repeat } from "./common.js";
import { ParseError } from "./model.js";
import type { Direction } from "./model.js";
import {
  readPropertyList,
  readPropertyRow,
  readPropertyTag,
  readPropertyTags,
  readRestriction,
  readTaggedValue,
} from "./properties.js";
import type { PropertyTag } from "./properties.js";
import type { RecordReader } from "./record.js";

export interface HttpPayloadReaders {
  readonly auxiliary: (record: RecordReader, length: number) => void;
  readonly rops: (record: RecordReader, length: number, direction: Direction) => void;
}

function readState(record: RecordReader): void {
  readFields(record, {
    SortType: "u32",
    ContainerID: "u32",
    CurrentRec: "u32",
    Delta: "u32",
    NumPos: "u32",
    TotalRecs: "u32",
    CodePage: "u32",
    TemplateLocale: "u32",
    SortLocale: "u32",
  });
}

function optionalState(record: RecordReader, byteFlag = false): boolean {
  const present = byteFlag ? record.integer("HasState", "u8") !== 0 : record.boolean("HasState");
  if (present) {
    nested(record, "State", readState, "STAT");
  }
  return present;
}

function minimalId(record: RecordReader, name: string): void {
  nested(record, name, (child) => child.integer("MinEntryID", "u32"), "MinimalEntryID");
}

function minimalIds(record: RecordReader, name = "MinimalIds", countName = "MinimalIdCount"): void {
  const count = record.integer(countName, "u32");
  repeat(record, count, () => minimalId(record, name), 4);
}

function optionalIds(record: RecordReader): void {
  if (record.boolean("HasMinimalIds")) {
    minimalIds(record);
  }
}

function optionalColumns(record: RecordReader, name = "Columns", flag = "HasColumns"): void {
  if (record.boolean(flag)) {
    nested(record, name, (child) => readPropertyTags(child, true), "LargePropertyTagArray");
  }
}

function columnsAndRows(record: RecordReader, flag = "HasColsAndRows", name = "Columns"): void {
  if (!record.boolean(flag)) {
    return;
  }
  let tags: PropertyTag[] = [];
  nested(
    record,
    name,
    (child) => {
      tags = readPropertyTags(child, true);
    },
    "LargePropertyTagArray",
  );
  const count = record.integer("RowCount", "u32");
  repeat(record, count, () => {
    nested(
      record,
      "RowData",
      (child) => readPropertyRow(child, tags, true),
      "AddressBookPropertyRow",
    );
  });
}

function readEntryId(record: RecordReader): void {
  const size = record.integer("EntryIDSize", "u32");
  bounded(record, "EntryID", size, (child) => {
    const ephemeral = child.reader.peekUInt8() === 0x87;
    child.integer(ephemeral ? "Type" : "IDType", "u8");
    readFields(child, { R1: "u8", R2: "u8", R3: "u8", ProviderUID: "guid", R4: "u32" });
    readEnum(child, ephemeral ? "DisplayType" : "DisplayTypeString", "DisplayTypeValues");
    if (ephemeral) {
      minimalId(child, "Mid");
    } else {
      child.string("DistinguishedName", false);
    }
  });
}

function readRequest(record: RecordReader, operation: string, payload: HttpPayloadReaders): void {
  switch (operation) {
    case "Connect":
      record.string("UserDn", false);
      readFields(record, {
        Flags: "u32",
        DefaultCodePage: "u32",
        LcidSort: "u32",
        LcidString: "u32",
      });
      return;
    case "Disconnect":
      return;
    case "NotificationWait":
      record.integer("Flags", "u32");
      return;
    case "Execute": {
      record.integer("Flags", "u32");
      const size = record.integer("RopBufferSize", "u32");
      payload.rops(record, size, "request");
      record.integer("MaxRopOut", "u32");
      return;
    }
    case "Bind":
      record.integer("Flags", "u32");
      optionalState(record, true);
      return;
    case "Unbind":
      record.integer("Reserved", "u32");
      return;
    case "CompareMIds":
      record.integer("Reserved", "u32");
      optionalState(record, true);
      minimalId(record, "MinimalId1");
      minimalId(record, "MinimalId2");
      return;
    case "DNToMId": {
      record.integer("Reserved", "u32");
      if (record.boolean("HasNames")) {
        const count = record.integer("NameCount", "u32");
        repeat(record, count, () => record.string("NameValues", false));
      }
      return;
    }
    case "GetMatches":
      record.integer("Reserved", "u32");
      optionalState(record);
      optionalIds(record);
      record.integer("InterfaceOptionFlags", "u32");
      if (record.boolean("HasFilter")) {
        nested(record, "Filter", (child) => readRestriction(child, "address-book"));
      }
      if (record.boolean("HasPropertyName")) {
        readFields(record, { PropertyNameGuid: "guid", PropertyNameId: "u32" });
      }
      record.integer("RowCount", "u32");
      optionalColumns(record);
      return;
    case "GetPropList":
      record.integer("Flags", "u32");
      minimalId(record, "MinimalId");
      record.integer("CodePage", "u32");
      return;
    case "GetProps":
      record.integer("Flags", "u32");
      optionalState(record);
      optionalColumns(record, "PropertyTags", "HasPropertyTags");
      return;
    case "GetSpecialTable":
      record.integer("Flags", "u32");
      optionalState(record);
      if (record.boolean("HasVersion")) {
        record.integer("Version", "u32");
      }
      return;
    case "GetTemplateInfo":
      readFields(record, { Flags: "u32", DisplayType: "u32" });
      if (record.boolean("HasTemplateDn")) {
        record.string("TemplateDn", false);
      }
      readFields(record, { CodePage: "u32", LocaleId: "u32" });
      return;
    case "ModLinkAtt":
      record.integer("Flags", "u32");
      nested(record, "PropertyTag", readPropertyTag);
      minimalId(record, "MinimalId");
      if (record.boolean("HasEntryIds")) {
        const count = record.integer("EntryIdCount", "u32");
        repeat(record, count, () => nested(record, "EntryIds", readEntryId));
      }
      return;
    case "ModProps":
      record.integer("Reserved", "u32");
      optionalState(record);
      optionalColumns(record, "PropertiesTags", "HasPropertyTags");
      if (record.boolean("HasPropertyValues")) {
        nested(record, "PropertyValues", (child) => readPropertyList(child, true));
      }
      return;
    case "QueryRows":
      record.integer("Flags", "u32");
      optionalState(record);
      minimalIds(record, "ExplicitTable", "ExplicitTableCount");
      record.integer("RowCount", "u32");
      optionalColumns(record);
      return;
    case "QueryColumns":
      readFields(record, { Reserved: "u32", MapiFlags: "u32" });
      return;
    case "ResolveNames":
      record.integer("Reserved", "u32");
      optionalState(record);
      optionalColumns(record, "PropertyTags", "HasPropertyTags");
      if (record.boolean("HasNames")) {
        nested(record, "Names", (child) => {
          const count = child.integer("CValues", "u32");
          repeat(child, count, () => child.string("LppszW", true), 2);
        });
      }
      return;
    case "ResortRestriction":
      record.integer("Reserved", "u32");
      optionalState(record);
      optionalIds(record);
      return;
    case "SeekEntries":
      record.integer("Reserved", "u32");
      optionalState(record);
      if (record.boolean("HasTarget")) {
        nested(record, "Target", (child) => readTaggedValue(child, "address-book"));
      }
      if (record.boolean("HasExplicitTable")) {
        minimalIds(record, "ExplicitTable", "ExplicitTableCount");
      }
      optionalColumns(record);
      return;
    case "UpdateStat":
      record.integer("Reserved", "u32");
      optionalState(record);
      record.boolean("DeltaRequested");
      return;
    case "GetMailboxUrl":
    case "GetAddressBookUrl":
      record.integer("Flags", "u32");
      record.string(operation === "GetMailboxUrl" ? "ServerDn" : "UserDn", true);
      return;
    default:
      throw new ParseError(
        "unsupported",
        `Unknown MAPI HTTP operation: ${operation}`,
        record.reader.range(),
      );
  }
}

function readResponse(record: RecordReader, operation: string, payload: HttpPayloadReaders): void {
  readEnum(record, "ErrorCode", "ErrorCodes");
  switch (operation) {
    case "Connect":
      readFields(record, { PollsMax: "u32", RetryCount: "u32", RetryDelay: "u32" });
      record.string("DnPrefix", false);
      record.string("DisplayName", true);
      return;
    case "Execute": {
      record.integer("Flags", "u32");
      const size = record.integer("RopBufferSize", "u32");
      payload.rops(record, size, "response");
      return;
    }
    case "NotificationWait":
      record.integer("EventPending", "u32");
      return;
    case "Bind":
      record.scalar("ServerGuid", "guid");
      return;
    case "CompareMIds":
      record.integer("Result", "i32");
      return;
    case "DNToMId":
      record.boolean("HasMinimalIds");
      minimalIds(record);
      return;
    case "GetMatches":
      optionalState(record);
      optionalIds(record);
      columnsAndRows(record);
      return;
    case "GetPropList":
      optionalColumns(record, "PropertyTags", "HasPropertyTags");
      return;
    case "GetProps":
      record.integer("CodePage", "u32");
      if (record.boolean("HasPropertyValues")) {
        nested(record, "PropertyValues", (child) => readPropertyList(child, true));
      }
      return;
    case "GetSpecialTable":
      record.integer("CodePage", "u32");
      if (record.boolean("HasVersion")) {
        record.integer("Version", "u32");
      }
      if (record.boolean("HasRows")) {
        const count = record.integer("RowsCount", "u32");
        repeat(
          record,
          count,
          () => nested(record, "Rows", (child) => readPropertyList(child, true)),
          4,
        );
      }
      return;
    case "GetTemplateInfo":
      record.integer("CodePage", "u32");
      if (record.boolean("HasRow")) {
        nested(record, "Row", (child) => readPropertyList(child, true));
      }
      return;
    case "QueryRows":
      optionalState(record);
      columnsAndRows(record);
      return;
    case "QueryColumns":
      optionalColumns(record);
      return;
    case "ResolveNames":
      record.integer("CodePage", "u32");
      optionalIds(record);
      columnsAndRows(record, "HasRowsAndCols", "PropertyTags");
      return;
    case "ResortRestriction":
      optionalState(record);
      optionalIds(record);
      return;
    case "SeekEntries":
      if (optionalState(record)) {
        columnsAndRows(record);
      }
      return;
    case "UpdateStat":
      if (optionalState(record) && record.boolean("HasDelta")) {
        record.integer("Delta", "i32");
      }
      return;
    case "GetMailboxUrl":
    case "GetAddressBookUrl":
      record.string("ServerUrl", true);
      return;
    case "Disconnect":
    case "Unbind":
    case "ModLinkAtt":
    case "ModProps":
      return;
    default:
      throw new ParseError(
        "unsupported",
        `Unknown MAPI HTTP operation: ${operation}`,
        record.reader.range(),
      );
  }
}

export function readHttpMessage(
  record: RecordReader,
  operation: string,
  direction: Direction,
  payload: HttpPayloadReaders,
): void {
  let status = 0;
  if (direction === "response") {
    let line: string;
    do {
      line = record.line("Response metadata");
    } while (line !== "");
    status = record.integer("StatusCode", "u32");
    if (status === 0) {
      readResponse(record, operation, payload);
    }
  } else {
    readRequest(record, operation, payload);
  }

  // The inspector omits this field on a failed ResortRestriction envelope.
  if (
    (operation === "Execute" || (operation === "ResortRestriction" && status !== 0)) &&
    record.reader.remaining === 0
  ) {
    return;
  }
  const size = record.integer("AuxiliaryBufferSize", "u32");
  if (size !== 0) {
    payload.auxiliary(record, size);
  }
}
