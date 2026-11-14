import {
  propertyTypes,
  propertyIds,
  wireSizes,
  mailboxLogonFolderCount,
  privateMailboxFlag,
} from "./wire.js";
import { nested, readEnum, readFields, readLongTermId, readObjectId, repeat } from "./common.js";
import type { RopRequest, Conversation } from "./conversation.js";
import { enums } from "./constants.js";
import { ParseError } from "./model.js";
import {
  readNamedProperty,
  readPropertyList,
  readPropertyRow,
  readPropertyTag,
} from "./properties.js";
import type { RecordReader } from "./record.js";
import type { RopPayloadReaders } from "./rops.js";
import { tags, sizedBytes, countedIds, restriction } from "./rop-values.js";
import { typedString, readRecipient } from "./rop-values.js";
import { successLayouts } from "./rop-layouts.js";
import { readLayout } from "./rop-values.js";

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

function readServers(
  record: RecordReader,
  countName = "ServerCount",
  cheapName = "CheapServerCount",
): void {
  const count = record.integer(countName, "u16");
  record.integer(cheapName, "u16");
  repeat(record, count, () => record.string("Server", false));
}

export function readResponseTail(
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
      const flags = readEnum(record, "LogonFlags", "LogonFlags");
      repeat(
        record,
        mailboxLogonFolderCount,
        () => nested(record, "FolderId", readObjectId),
        wireSizes.objectId,
      );
      const privateMailbox = (flags & privateMailboxFlag) !== 0;
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
        record.bytes("ClientData", wireSizes.clientData);
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
        { type: propertyTypes.integer64, id: propertyIds.folder },
        { type: propertyTypes.string8, id: propertyIds.messageClass },
        { type: propertyTypes.time, id: propertyIds.lastModificationTime },
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
      record.bytes("ObjectId", wireSizes.objectId);
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
