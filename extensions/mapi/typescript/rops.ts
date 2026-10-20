import {
  bounded,
  nested,
  readEnum,
  readFields,
  readLongTermId,
  readObjectId,
  repeat,
} from "./common.js";
import { Conversation, numberField } from "./conversation.js";
import type { RopRequest } from "./conversation.js";
import { enums } from "./constants.js";
import { ParseError } from "./model.js";
import { readNotification } from "./notifications.js";
import type { Direction } from "./model.js";
import {
  readNamedProperty,
  readPropertyList,
  readPropertyRow,
  readPropertyTag,
  readRestriction,
  readTaggedValue,
} from "./properties.js";
import type { PropertyTag } from "./properties.js";
import type { RecordReader, ScalarType } from "./record.js";
import { requestLayouts, successLayouts } from "./rop-layouts.js";
import type { WireLayout } from "./rop-layouts.js";

export interface RopPayloadReaders {
  readonly transfer: (record: RecordReader, length: number, stream: string, handle: number) => void;
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
const opcodes = new Map(
  Object.entries(enums.RopIdType!.values).map(([name, value]) => [value, name]),
);

const emptySuccessResponses = new Set([
  "RopSetReceiveFolder",
  "RopWritePerUserInformation",
  "RopSubmitMessage",
  "RopAbortSubmit",
  "RopSetSpooler",
  "RopSpoolerLockMessage",
  "RopTransportNewMail",
  "RopSetSearchCriteria",
  "RopFreeBookmark",
  "RopResetTable",
  "RopSeekRowFractional",
  "RopModifyRules",
  "RopGetRulesTable",
  "RopUpdateDeferredActionMessages",
  "RopGetPermissionsTable",
  "RopModifyPermissions",
  "RopCommitStream",
  "RopCloneStream",
  "RopSetStreamSize",
  "RopLockRegionStream",
  "RopUnlockRegionStream",
  "RopDeleteAttachment",
  "RopOpenAttachment",
  "RopGetAttachmentTable",
  "RopRemoveAllRecipients",
  "RopModifyRecipients",
  "RopSaveChangesAttachment",
  "RopFastTransferDestinationConfigure",
  "RopFastTransferSourceCopyFolder",
  "RopFastTransferSourceCopyMessages",
  "RopFastTransferSourceCopyProperties",
  "RopFastTransferSourceCopyTo",
  "RopSynchronizationConfigure",
  "RopSynchronizationGetTransferState",
  "RopSynchronizationOpenCollector",
  "RopSynchronizationImportDeletes",
  "RopSynchronizationImportReadStateChanges",
  "RopSynchronizationUploadStateStreamBegin",
  "RopSynchronizationUploadStateStreamContinue",
  "RopSynchronizationUploadStateStreamEnd",
  "RopSetLocalReplicaMidsetDeleted",
  "RopTellVersion",
  "RopRegisterNotification",
]);

function readLayout(record: RecordReader, layout: WireLayout): void {
  for (const [name, type] of Object.entries(layout)) {
    if (scalarTypes.has(type)) {
      record.scalar(name, type as ScalarType);
    } else if (type === "bool16") {
      record.boolean(name, "u16");
    } else if (type === "FolderID" || type === "MessageID") {
      nested(record, name, readObjectId, type);
    } else if (type === "LongTermID") {
      nested(record, name, readLongTermId, type);
    } else if (type === "PropertyTag") {
      nested(record, name, readPropertyTag);
    } else {
      readEnum(record, name, type);
    }
  }
}

function tags(record: RecordReader, count: number): PropertyTag[] {
  const result: PropertyTag[] = [];
  repeat(
    record,
    count,
    () => nested(record, "PropertyTag", (child) => result.push(readPropertyTag(child))),
    4,
  );
  return result;
}

function sizedBytes(record: RecordReader, name: string, wide = false): void {
  const size = record.integer(`${name}Size`, wide ? "u32" : "u16");
  record.bytes(name, size);
}

function countedIds(record: RecordReader, count: number, type: string): void {
  repeat(record, count, () => nested(record, type, readObjectId), 8);
}

function restriction(record: RecordReader, size: number): void {
  if (size !== 0) {
    bounded(record, "RestrictionData", size, readRestriction);
  }
}

function typedString(record: RecordReader): void {
  const type = readEnum(record, "StringType", "StringTypeEnum");
  if (type === 2 || type === 3) {
    record.string("String", false);
  } else if (type === 4) {
    record.string("String", true);
  } else if (type !== 0 && type !== 1) {
    throw new ParseError("malformed", "Unknown typed string encoding", record.reader.range());
  }
}

function readRequestTail(
  record: RecordReader,
  operation: string,
  request: RopRequest,
  conversation: Conversation,
  payload: RopPayloadReaders,
): void {
  const count = (name: string) => numberField(record.field, name);
  switch (operation) {
    case "RopLogon":
      if (count("EssdnSize") !== 0) {
        record.string("Essdn", false, count("EssdnSize"));
      }
      conversation.logons.set(count("LogonId"), count("LogonFlags"));
      return;
    case "RopGetReceiveFolder":
    case "RopSetReceiveFolder":
      record.string("MessageClass", false);
      return;
    case "RopLongTermIdFromId":
      record.bytes("ObjectId", 8);
      return;
    case "RopWritePerUserInformation":
      record.bytes("Data", count("DataSize"));
      if (
        count("DataOffset") === 0 &&
        ((conversation.logons.get(count("LogonId")) ?? 0) & 1) !== 0
      ) {
        record.scalar("ReplGuid", "guid");
      }
      return;
    case "RopCreateFolder":
      record.string("DisplayName", count("UseUnicodeStrings") !== 0);
      record.string("Comment", count("UseUnicodeStrings") !== 0);
      return;
    case "RopMoveFolder":
    case "RopCopyFolder":
      record.string("NewFolderName", count("UseUnicode") !== 0);
      return;
    case "RopMoveCopyMessages":
      countedIds(record, count("MessageIdCount"), "MessageID");
      readFields(record, { WantAsynchronous: "bool", WantCopy: "bool" });
      return;
    case "RopDeleteMessages":
    case "RopHardDeleteMessages":
    case "RopSetReadFlags":
      countedIds(record, count("MessageIdCount"), "MessageID");
      return;
    case "RopSetSearchCriteria":
      restriction(record, count("RestrictionDataSize"));
      countedIds(record, record.integer("FolderIdCount", "u16"), "FolderID");
      readEnum(record, "SearchFlags", "SearchRequestFlags");
      return;
    case "RopSetMessageReadFlag": {
      const flags = conversation.logons.get(count("LogonId"));
      if (flags === undefined) {
        throw new ParseError(
          "missing-context",
          "Missing LogonFlags for message read flag",
          record.reader.range(),
        );
      }
      if ((flags & 1) === 0) {
        record.bytes("ClientData", 24);
      }
      return;
    }
    case "RopGetPropertiesSpecific":
    case "RopSetColumns":
    case "RopDeleteProperties":
    case "RopDeletePropertiesNoReplicate":
    case "RopCopyProperties":
    case "RopFastTransferSourceCopyProperties":
    case "RopFastTransferSourceCopyTo":
      request.tags = tags(record, count("PropertyTagCount"));
      return;
    case "RopCopyTo":
      tags(record, count("ExcludedTagCount"));
      return;
    case "RopSetProperties":
      bounded(record, "PropertyValues", count("PropertyValueSize"), (child) =>
        readPropertyList(child, false),
      );
      return;
    case "RopSetPropertiesNoReplicate":
      readPropertyList(record, false);
      return;
    case "RopGetPropertyIdsFromNames":
      repeat(record, count("PropertyNameCount"), () =>
        nested(record, "PropertyName", readNamedProperty),
      );
      return;
    case "RopGetNamesFromPropertyIds":
      repeat(record, count("PropertyIdCount"), () => record.integer("PropertyIds", "u16"), 2);
      return;
    case "RopQueryNamedProperties":
      if (count("HasGuid") !== 0) {
        record.scalar("PropertyGuid", "guid");
      }
      return;
    case "RopReadStream":
      if (count("ByteCount") === 0xbabe) {
        record.integer("MaximumByteCount", "u32");
      }
      return;
    case "RopWriteStream":
    case "RopWriteStreamExtended":
    case "RopWriteAndCommitStream":
      record.bytes("Data", count("DataSize"));
      return;
    case "RopSeekRowBookmark":
      record.bytes("Bookmark", count("BookmarkSize"));
      readFields(record, { RowCount: "i32", WantRowMovedCount: "bool" });
      return;
    case "RopFreeBookmark":
      record.bytes("Bookmark", count("BookmarkSize"));
      return;
    case "RopSetCollapseState":
      record.bytes("CollapseState", count("CollapseStateSize"));
      return;
    case "RopSortTable":
      repeat(
        record,
        count("SortOrderCount"),
        () =>
          nested(record, "SortOrder", (child) => {
            readPropertyTag(child);
            readEnum(child, "Order", "OrderType");
          }),
        5,
      );
      return;
    case "RopFindRow":
    case "RopRestrict":
      restriction(record, count("RestrictionDataSize"));
      if (operation === "RopFindRow") {
        readEnum(record, "Origin", "Bookmarks");
        sizedBytes(record, "Bookmark");
      }
      return;
    case "RopModifyPermissions":
    case "RopModifyRules":
      repeat(record, count(operation === "RopModifyRules" ? "RulesCount" : "ModifyCount"), () => {
        nested(record, "Entry", (child) => {
          readEnum(
            child,
            "Flags",
            operation === "RopModifyRules" ? "RuleDataFlags" : "PermissionDataFlags",
          );
          const properties = child.integer("PropertyValueCount", "u16");
          repeat(
            child,
            properties,
            () =>
              nested(child, "PropertyValue", (value) =>
                readTaggedValue(value, operation === "RopModifyRules" ? "address-book" : "rop"),
              ),
            4,
          );
        });
      });
      return;
    case "RopUpdateDeferredActionMessages":
      record.bytes("ServerEntryId", count("ServerEntryIdSize"));
      sizedBytes(record, "ClientEntryId");
      return;
    case "RopSynchronizationConfigure":
      restriction(record, count("RestrictionDataSize"));
      readEnum(record, "SynchronizationExtraFlags", "SynchronizationExtraFlags");
      tags(record, record.integer("PropertyTagCount", "u16"));
      return;
    case "RopSynchronizationImportHierarchyChange": {
      const hierarchyCount = record.integer("HierarchyValueCount", "u16");
      repeat(record, hierarchyCount, () => nested(record, "HierarchyValue", readTaggedValue), 4);
      readPropertyList(record, false);
      return;
    }
    case "RopSynchronizationImportMessageChange":
    case "RopSynchronizationImportDeletes":
      readPropertyList(record, false);
      return;
    case "RopSynchronizationImportMessageMove":
      for (const name of [
        "SourceFolderId",
        "SourceMessageId",
        "PredecessorChangeList",
        "DestinationMessageId",
        "ChangeNumber",
      ]) {
        sizedBytes(record, name, true);
      }
      return;
    case "RopSynchronizationImportReadStateChanges":
      bounded(record, "MessageReadStates", count("MessageReadStatesSize"), (child) => {
        while (child.reader.remaining !== 0) {
          nested(child, "MessageReadState", (state) => {
            sizedBytes(state, "MessageId");
            state.boolean("MarkAsRead");
          });
        }
      });
      return;
    case "RopSynchronizationUploadStateStreamContinue":
      record.bytes("StreamData", count("StreamDataSize"));
      return;
    case "RopSetLocalReplicaMidsetDeleted":
      repeat(
        record,
        count("LongTermIdRangeCount"),
        () =>
          nested(record, "LongTermIdRange", (child) => {
            nested(child, "MinLongTermId", readLongTermId);
            nested(child, "MaxLongTermId", readLongTermId);
          }),
        48,
      );
      return;
    case "RopFastTransferSourceCopyMessages":
      countedIds(record, count("MessageIdCount"), "MessageID");
      readEnum(record, "CopyFlags", "CopyFlags_CopyMessages");
      readEnum(record, "SendOptions", "SendOptions");
      return;
    case "RopFastTransferSourceGetBuffer":
      if (count("BufferSize") === 0xbabe) {
        record.integer("MaximumBufferSize", "u16");
      }
      return;
    case "RopFastTransferDestinationPutBuffer":
    case "RopFastTransferDestinationPutBufferExtended":
      payload.transfer(record, count("TransferDataSize"), operation, request.handle ?? 0xffffffff);
      return;
    case "RopTellVersion":
      record.bytes("Version", 6);
      return;
    case "RopTransportNewMail":
      record.string("MessageClass", false);
      readEnum(record, "MessageFlags", "MessageFlags");
      return;
    case "RopOptionsData":
      record.string("AddressType", false);
      record.integer("WantWin32", "u8");
      return;
    case "RopRegisterNotification":
      if ((count("NotificationTypes") & 0x0400) !== 0) {
        record.integer("Reserved", "u8");
      }
      if (!record.boolean("WantWholeStore")) {
        nested(record, "FolderId", readObjectId);
        nested(record, "MessageId", readObjectId);
      }
      return;
    case "RopRelease":
      if (request.handle !== undefined) {
        conversation.columns.delete(request.handle);
      }
      return;
    case "RopModifyRecipients": {
      const columns = tags(record, count("ColumnCount"));
      const rowCount = record.integer("RowCount", "u16");
      repeat(record, rowCount, () =>
        nested(record, "RecipientRow", (child) => {
          child.integer("RowId", "u32");
          child.integer("RecipientType", "u8");
          const size = child.integer("RecipientRowSize", "u16");
          if (size !== 0) {
            readRecipient(child, size, columns);
          }
        }),
      );
      return;
    }
  }
}

function readRecipient(record: RecordReader, size: number, columns: readonly PropertyTag[]): void {
  bounded(record, "Recipient", size, (child) => {
    const flags = child.integer("RecipientFlags", "u16");
    const addressType = (flags >> 5) & 7;
    if (addressType === 1) {
      child.integer("AddressPrefixUsed", "u8");
      readEnum(child, "DisplayType", "DisplayType");
      child.string("X500DN", false);
    } else if (addressType === 6 || addressType === 7) {
      sizedBytes(child, "EntryID");
      sizedBytes(child, "SearchKey");
    } else if (addressType === 0 && (flags & 0x0100) !== 0) {
      child.string("AddressType", false);
    }
    const wide = (flags & 0x4000) !== 0;
    for (const [bit, name] of [
      [0x10, "EmailAddress"],
      [8, "DisplayName"],
      [0x2000, "SimpleDisplayName"],
      [4, "TransmittableDisplayName"],
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

function readServers(
  record: RecordReader,
  countName = "ServerCount",
  cheapName = "CheapServerCount",
): void {
  const count = record.integer(countName, "u16");
  record.integer(cheapName, "u16");
  repeat(record, count, () => record.string("Server", false));
}

function readResponseTail(
  record: RecordReader,
  operation: string,
  status: number,
  request: RopRequest | undefined,
  conversation: Conversation,
  payload: RopPayloadReaders,
  handle: number,
): void {
  const additional = enums.AdditionalErrorCodes!.values;
  const partialCompletion = [
    "RopDeleteFolder",
    "RopDeleteMessages",
    "RopEmptyFolder",
    "RopHardDeleteMessages",
    "RopHardDeleteMessagesAndSubfolders",
    "RopSetReadFlags",
  ];
  if (["RopCopyFolder", "RopMoveFolder", "RopMoveCopyMessages"].includes(operation)) {
    if (status === additional.NullDestinationObject) {
      record.integer("DestHandleIndex", "u32");
    }
    record.boolean("PartialCompletion");
    return;
  }
  if (partialCompletion.includes(operation)) {
    record.boolean("PartialCompletion");
    return;
  }
  if (operation === "RopCopyToStream") {
    if (status === additional.NullDestinationObject) {
      record.integer("DestHandleIndex", "u32");
    }
    readFields(record, { ReadByteCount: "u64", WrittenByteCount: "u64" });
    return;
  }
  if (
    [
      "RopReadStream",
      "RopWriteStream",
      "RopWriteAndCommitStream",
      "RopWriteStreamExtended",
    ].includes(operation)
  ) {
    if (operation === "RopReadStream") {
      sizedBytes(record, "Data");
    } else {
      record.integer("WrittenSize", operation === "RopWriteStreamExtended" ? "u32" : "u16");
    }
    return;
  }
  if (
    operation === "RopFastTransferSourceGetBuffer" ||
    operation.startsWith("RopFastTransferDestinationPutBuffer")
  ) {
    const extended = operation.endsWith("Extended");
    if (extended) {
      record.integer("TransferStatus", "u16");
    } else {
      readEnum(record, "TransferStatus", "TransferStatus");
    }
    record.integer("InProgressCount", extended ? "u32" : "u16");
    record.integer("TotalStepCount", extended ? "u32" : "u16");
    record.integer("Reserved", "u8");
    const size = record.integer(
      operation === "RopFastTransferSourceGetBuffer" ? "TransferBufferSize" : "BufferUsedSize",
      "u16",
    );
    if (operation === "RopFastTransferSourceGetBuffer") {
      if (status === 0) {
        payload.transfer(record, size, operation, handle);
      } else if (status === additional.ServerBusy) {
        record.integer("BackoffTime", "u32");
      }
    }
    return;
  }
  if (status !== 0) {
    if (operation === "RopLogon" && status === additional.WrongServer) {
      readEnum(record, "LogonFlags", "LogonFlags");
      const size = record.integer("ServerNameSize", "u8");
      record.string("ServerName", false, size);
    } else if (
      ["RopCopyProperties", "RopCopyTo"].includes(operation) &&
      status === additional.NullDestinationObject
    ) {
      record.integer("DestHandleIndex", "u32");
    }
    return;
  }
  readLayout(record, successLayouts[operation] ?? {});
  switch (operation) {
    case "RopLogon": {
      readEnum(record, "LogonFlags", "LogonFlags");
      repeat(record, 13, () => nested(record, "FolderId", readObjectId), 8);
      const privateMailbox = request && (numberField(request.field, "LogonFlags") & 1) !== 0;
      if (privateMailbox) {
        readEnum(record, "ResponseFlags", "ResponseFlags");
        record.scalar("MailboxGuid", "guid");
      }
      readFields(record, { ReplId: "u16", ReplGuid: "guid" });
      if (privateMailbox) {
        nested(record, "LogonTime", (child) => {
          readFields(child, { Seconds: "u8", Minutes: "u8", Hour: "u8" });
          readEnum(child, "DayOfWeek", "DayOfWeek");
          child.integer("Day", "u8");
          readEnum(child, "Month", "Month");
          child.integer("Year", "u16");
        });
        readFields(record, { GwartTime: "u64", StoreState: "u32" });
      } else {
        record.scalar("PerUserGuid", "guid");
      }
      return;
    }
    case "RopOpenFolder":
    case "RopCreateFolder":
      if (operation === "RopCreateFolder") {
        nested(record, "FolderId", readObjectId);
        if (!record.boolean("IsExistingFolder")) {
          return;
        }
      }
      record.boolean("HasRules");
      if (record.boolean("IsGhosted")) {
        readServers(record);
      }
      return;
    case "RopCreateMessage":
      if (record.boolean("HasMessageId")) {
        nested(record, "MessageId", readObjectId);
      }
      return;
    case "RopOpenMessage":
    case "RopOpenEmbeddedMessage":
    case "RopReloadCachedInformation": {
      if (operation === "RopOpenEmbeddedMessage") {
        record.integer("Reserved", "u8");
        nested(record, "MessageId", readObjectId);
      }
      record.boolean("HasNamedProperties");
      nested(record, "SubjectPrefix", typedString, "TypedString");
      nested(record, "NormalizedSubject", typedString, "TypedString");
      record.integer("RecipientCount", "u16");
      const columns = tags(record, record.integer("ColumnCount", "u16"));
      const count = record.integer("RowCount", "u8");
      repeat(record, count, () =>
        nested(record, "RecipientRow", (child) => {
          child.integer("RecipientType", "u8");
          readFields(child, { CodePageId: "u16", Reserved: "u16" });
          readRecipient(child, child.integer("RecipientRowSize", "u16"), columns);
        }),
      );
      return;
    }
    case "RopGetValidAttachments":
      repeat(
        record,
        record.integer("AttachmentIdCount", "u16"),
        () => record.integer("AttachmentId", "i32"),
        4,
      );
      return;
    case "RopSaveChangesMessage":
      record.integer("InputHandleIndex", "u8");
      nested(record, "MessageId", readObjectId);
      return;
    case "RopSetMessageReadFlag":
      if (record.boolean("ReadStatusChanged")) {
        record.integer("LogonId", "u8");
        record.bytes("ClientData", 24);
      }
      return;
    case "RopGetPropertiesAll":
      readPropertyList(record, false);
      return;
    case "RopTransportSend":
      record.integer("NoPropertiesReturned", "u8");
      readPropertyList(record, false);
      return;
    case "RopGetPropertiesSpecific":
      if (!request?.tags) {
        throw new ParseError(
          "missing-context",
          "Missing paired property tags",
          record.reader.range(),
        );
      }
      nested(
        record,
        "RowData",
        (child) => readPropertyRow(child, request.tags!, false),
        "PropertyRow",
      );
      return;
    case "RopGetPropertiesList":
    case "RopQueryColumnsAll":
      tags(record, record.integer("PropertyTagCount", "u16"));
      return;
    case "RopGetPropertyIdsFromNames":
      repeat(
        record,
        record.integer("PropertyIdCount", "u16"),
        () => record.integer("PropertyId", "u16"),
        2,
      );
      return;
    case "RopGetNamesFromPropertyIds":
      repeat(record, record.integer("PropertyNameCount", "u16"), () =>
        nested(record, "PropertyName", readNamedProperty),
      );
      return;
    case "RopQueryNamedProperties": {
      const count = record.integer("IdCount", "u16");
      repeat(record, count, () => record.integer("PropertyId", "u16"), 2);
      repeat(record, count, () => nested(record, "PropertyName", readNamedProperty));
      return;
    }
    case "RopSetProperties":
    case "RopSetPropertiesNoReplicate":
    case "RopDeleteProperties":
    case "RopDeletePropertiesNoReplicate":
    case "RopCopyProperties":
    case "RopCopyTo":
      repeat(
        record,
        record.integer("PropertyProblemCount", "u16"),
        () =>
          nested(record, "PropertyProblem", (child) => {
            child.integer("Index", "u16");
            nested(child, "PropertyTag", readPropertyTag);
            readEnum(child, "ErrorCode", "PropertyErrorCodes");
          }),
        10,
      );
      return;
    case "RopQueryRows":
    case "RopExpandRow":
    case "RopFindRow": {
      const columns = conversation.columns.get(handle);
      if (!columns) {
        throw new ParseError(
          "missing-context",
          "Missing SetColumns information for table rows",
          record.reader.range(),
        );
      }
      let count: number;
      if (operation === "RopQueryRows") {
        readEnum(record, "Origin", "Bookmarks");
        count = record.integer("RowCount", "u16");
      } else if (operation === "RopExpandRow") {
        record.integer("ExpandedRowCount", "u32");
        count = record.integer("RowCount", "u16");
      } else {
        record.boolean("RowNoLongerVisible");
        count = record.boolean("HasRowData") ? 1 : 0;
      }
      repeat(record, count, () =>
        nested(record, "RowData", (child) => readPropertyRow(child, columns, false), "PropertyRow"),
      );
      return;
    }
    case "RopSetColumns":
      if (request?.tags) {
        conversation.columns.set(handle, request.tags);
      }
      return;
    case "RopCreateBookmark":
    case "RopSetCollapseState":
      sizedBytes(record, "Bookmark");
      return;
    case "RopGetCollapseState":
      sizedBytes(record, "CollapseState");
      return;
    case "RopGetOwningServers":
      readServers(record, "OwningServersCount", "CheapServersCount");
      return;
    case "RopPublicFolderIsGhosted":
      if (record.boolean("IsGhosted")) {
        readServers(record, "ServersCount", "CheapServersCount");
      }
      return;
    case "RopGetReceiveFolder":
      nested(record, "FolderId", readObjectId);
      record.string("ExplicitMessageClass", false);
      return;
    case "RopGetReceiveFolderTable": {
      const columns = [
        { type: 0x0014, id: 0x6748 },
        { type: 0x001e, id: 0x001a },
        { type: 0x0040, id: 0x3008 },
      ];
      const count = record.integer("RowCount", "u32");
      repeat(record, count, () =>
        nested(record, "Row", (child) => readPropertyRow(child, columns, false), "PropertyRow"),
      );
      return;
    }
    case "RopGetSearchCriteria":
      restriction(record, record.integer("RestrictionDataSize", "u16"));
      record.integer("LogonId", "u8");
      countedIds(record, record.integer("FolderIdCount", "u16"), "FolderID");
      readEnum(record, "SearchFlags", "SearchResponseFlags");
      return;
    case "RopGetPerUserLongTermIds":
      repeat(
        record,
        record.integer("LongTermIdCount", "u16"),
        () => nested(record, "LongTermId", readLongTermId),
        24,
      );
      return;
    case "RopIdFromLongTermId":
      record.bytes("ObjectId", 8);
      return;
    case "RopReadPerUserInformation":
      record.boolean("HasFinished");
      sizedBytes(record, "Data");
      return;
    case "RopGetLocalReplicaIds":
      record.scalar("ReplGuid", "guid");
      record.bytes("GlobalCount", 6);
      return;
    case "RopGetAddressTypes": {
      const count = record.integer("AddressTypeCount", "u16");
      record.integer("AddressTypeSize", "u16");
      repeat(record, count, () => record.string("AddressType", false));
      return;
    }
    case "RopReadRecipients": {
      const count = record.integer("RowCount", "u8");
      repeat(record, count, () =>
        nested(record, "RecipientRow", (child) => {
          readFields(child, {
            RowId: "u32",
            RecipientType: "u8",
            CodePageId: "u16",
            Reserved: "u16",
          });
          sizedBytes(child, "RecipientRow");
        }),
      );
      return;
    }
    case "RopOptionsData": {
      record.integer("Reserved", "u8");
      sizedBytes(record, "OptionsInfo");
      const size = record.integer("HelpFileSize", "u16");
      if (size !== 0) {
        record.bytes("HelpFile", size);
        record.string("HelpFileName", false);
      }
      return;
    }
    default:
      if (!successLayouts[operation] && !emptySuccessResponses.has(operation)) {
        throw new ParseError(
          "unsupported",
          `Unsupported success payload for ${operation}`,
          record.reader.range(),
        );
      }
  }
}

/** Parses one ROP; the enclosing ROP buffer enforces forward progress. */
export function readRop(
  record: RecordReader,
  direction: Direction,
  handles: readonly number[],
  conversation: Conversation,
  payload: RopPayloadReaders,
): void {
  const opcode = readEnum(record, "RopId", "RopIdType");
  const operation = opcodes.get(opcode);
  if (!operation) {
    throw new ParseError(
      "unsupported",
      `Unknown ROP 0x${opcode.toString(16)}`,
      record.reader.range(),
    );
  }
  if (direction === "request") {
    const layout = requestLayouts[operation];
    if (!layout) {
      throw new ParseError(
        "unsupported",
        `Unsupported request ROP ${operation}`,
        record.reader.range(),
      );
    }
    record.integer("LogonId", "u8");
    readLayout(record, layout);
    const indexName =
      "InputHandleIndex" in layout
        ? "InputHandleIndex"
        : "SourceHandleIndex" in layout
          ? "SourceHandleIndex"
          : "OutputHandleIndex";
    const request: RopRequest = {
      operation,
      field: record.field,
      handle: handles[numberField(record.field, indexName)],
    };
    readRequestTail(record, operation, request, conversation, payload);
    conversation.requests.push(request);
    return;
  }
  if (operation === "RopPending") {
    record.integer("SessionIndex", "u16");
    return;
  }
  if (operation === "RopBackoff") {
    readFields(record, { LogonId: "u8", Duration: "u32" });
    const count = record.integer("BackoffRopCount", "u8");
    repeat(
      record,
      count,
      () =>
        nested(record, "BackoffRop", (child) =>
          readFields(child, { RopIdBackoff: "u8", Duration: "u32" }),
        ),
      5,
    );
    sizedBytes(record, "AdditionalData");
    return;
  }
  if (operation === "RopBufferTooSmall") {
    record.integer("SizeNeeded", "u16");
    record.bytes("RequestBuffers", record.reader.remaining);
    return;
  }
  if (operation === "RopNotify") {
    readNotification(record, conversation);
    return;
  }
  const index = record.integer("HandleIndex", "u8");
  const status = readEnum(record, "ReturnValue", "ErrorCodes");
  const request = conversation.requestFor(operation);
  readResponseTail(
    record,
    operation,
    status,
    request,
    conversation,
    payload,
    handles[index] ?? request?.handle ?? 0xffffffff,
  );
}

export function ropName(opcode: number, direction: Direction): string {
  return `${opcodes.get(opcode) ?? "Unknown ROP"} ${direction}`;
}
