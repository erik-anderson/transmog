import {
  wireSizes,
  invalidHandle,
  extendedBufferSize,
  privateMailboxFlag,
  notificationFlags,
} from "./wire.js";
import {
  bounded,
  nested,
  readEnum,
  readFields,
  readLongTermId,
  readObjectId,
  repeat,
} from "./common.js";
import { requestParameter } from "./conversation.js";
import type { RopRequest, Conversation } from "./conversation.js";
import { ParseError } from "./model.js";
import {
  readNamedProperty,
  readPropertyList,
  readPropertyTag,
  readTaggedValue,
} from "./properties.js";
import type { RecordReader } from "./record.js";
import type { RopPayloadReaders } from "./rops.js";
import { tags, sizedBytes, countedIds, restriction, readRecipient } from "./rop-values.js";

export function readRequestTail(
  record: RecordReader,
  operation: string,
  request: RopRequest,
  conversation: Conversation,
  payload: RopPayloadReaders,
): void {
  const parameter = (name: string) => requestParameter(request, name);
  switch (operation) {
    case "RopLogon":
      if (parameter("EssdnSize") !== 0) {
        record.string("Essdn", false, parameter("EssdnSize"));
      }
      conversation.logons.set(request.logonId, parameter("LogonFlags"));
      return;
    case "RopGetReceiveFolder":
    case "RopSetReceiveFolder":
      record.string("MessageClass", false);
      return;
    case "RopLongTermIdFromId":
      record.bytes("ObjectId", wireSizes.objectId);
      return;
    case "RopWritePerUserInformation":
      record.bytes("Data", parameter("DataSize"));
      if (parameter("DataOffset") === 0) {
        const flags = conversation.logons.get(request.logonId);
        if (flags === undefined) {
          throw new ParseError(
            "missing-context",
            "Missing LogonFlags for per-user information",
            record.reader.range(),
          );
        }
        if ((flags & privateMailboxFlag) !== 0) {
          record.scalar("ReplGuid", "guid");
        }
      }
      return;
    case "RopCreateFolder":
      record.string("DisplayName", parameter("UseUnicodeStrings") !== 0);
      record.string("Comment", parameter("UseUnicodeStrings") !== 0);
      return;
    case "RopMoveFolder":
    case "RopCopyFolder":
      record.string("NewFolderName", parameter("UseUnicode") !== 0);
      return;
    case "RopMoveCopyMessages":
      countedIds(record, parameter("MessageIdCount"), "MessageID");
      readFields(record, { WantAsynchronous: "bool", WantCopy: "bool" });
      return;
    case "RopDeleteMessages":
    case "RopHardDeleteMessages":
    case "RopSetReadFlags":
      countedIds(record, parameter("MessageIdCount"), "MessageID");
      return;
    case "RopSetSearchCriteria":
      restriction(record, parameter("RestrictionDataSize"));
      countedIds(record, record.integer("FolderIdCount", "u16"), "FolderID");
      readEnum(record, "SearchFlags", "SearchRequestFlags");
      return;
    case "RopSetMessageReadFlag": {
      const flags = conversation.logons.get(request.logonId);
      if (flags === undefined) {
        throw new ParseError(
          "missing-context",
          "Missing LogonFlags for message read flag",
          record.reader.range(),
        );
      }
      if ((flags & privateMailboxFlag) === 0) {
        record.bytes("ClientData", wireSizes.clientData);
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
      request.tags = tags(record, parameter("PropertyTagCount"));
      return;
    case "RopCopyTo":
      tags(record, parameter("ExcludedTagCount"));
      return;
    case "RopSetProperties":
      bounded(record, "PropertyValues", parameter("PropertyValueSize"), (child) =>
        readPropertyList(child, false),
      );
      return;
    case "RopSetPropertiesNoReplicate":
      readPropertyList(record, false);
      return;
    case "RopGetPropertyIdsFromNames":
      repeat(record, parameter("PropertyNameCount"), () =>
        nested(record, "PropertyName", readNamedProperty),
      );
      return;
    case "RopGetNamesFromPropertyIds":
      repeat(record, parameter("PropertyIdCount"), () => record.integer("PropertyIds", "u16"), 2);
      return;
    case "RopQueryNamedProperties":
      if (parameter("HasGuid") !== 0) {
        record.scalar("PropertyGuid", "guid");
      }
      return;
    case "RopReadStream":
      if (parameter("ByteCount") === extendedBufferSize) {
        record.integer("MaximumByteCount", "u32");
      }
      return;
    case "RopWriteStream":
    case "RopWriteStreamExtended":
    case "RopWriteAndCommitStream":
      record.bytes("Data", parameter("DataSize"));
      return;
    case "RopSeekRowBookmark":
      record.bytes("Bookmark", parameter("BookmarkSize"));
      readFields(record, { RowCount: "i32", WantRowMovedCount: "bool" });
      return;
    case "RopFreeBookmark":
      record.bytes("Bookmark", parameter("BookmarkSize"));
      return;
    case "RopSetCollapseState":
      record.bytes("CollapseState", parameter("CollapseStateSize"));
      return;
    case "RopSortTable":
      repeat(
        record,
        parameter("SortOrderCount"),
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
      restriction(record, parameter("RestrictionDataSize"));
      if (operation === "RopFindRow") {
        readEnum(record, "Origin", "Bookmarks");
        sizedBytes(record, "Bookmark");
      }
      return;
    case "RopModifyPermissions":
    case "RopModifyRules":
      repeat(
        record,
        parameter(operation === "RopModifyRules" ? "RulesCount" : "ModifyCount"),
        () => {
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
                  readTaggedValue(value, operation === "RopModifyRules" ? "extended-rule" : "rop"),
                ),
              4,
            );
          });
        },
      );
      return;
    case "RopUpdateDeferredActionMessages":
      record.bytes("ServerEntryId", parameter("ServerEntryIdSize"));
      sizedBytes(record, "ClientEntryId");
      return;
    case "RopSynchronizationConfigure":
      restriction(record, parameter("RestrictionDataSize"));
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
      bounded(record, "MessageReadStates", parameter("MessageReadStatesSize"), (child) => {
        while (child.reader.remaining !== 0) {
          nested(child, "MessageReadState", (state) => {
            sizedBytes(state, "MessageId");
            state.boolean("MarkAsRead");
          });
        }
      });
      return;
    case "RopSynchronizationUploadStateStreamContinue":
      record.bytes("StreamData", parameter("StreamDataSize"));
      return;
    case "RopSetLocalReplicaMidsetDeleted":
      repeat(
        record,
        parameter("LongTermIdRangeCount"),
        () =>
          nested(record, "LongTermIdRange", (child) => {
            nested(child, "MinLongTermId", readLongTermId);
            nested(child, "MaxLongTermId", readLongTermId);
          }),
        wireSizes.longTermIdRange,
      );
      return;
    case "RopFastTransferSourceCopyMessages":
      countedIds(record, parameter("MessageIdCount"), "MessageID");
      readEnum(record, "CopyFlags", "CopyFlags_CopyMessages");
      readEnum(record, "SendOptions", "SendOptions");
      return;
    case "RopFastTransferSourceGetBuffer":
      if (parameter("BufferSize") === extendedBufferSize) {
        record.integer("MaximumBufferSize", "u16");
      }
      return;
    case "RopFastTransferDestinationPutBuffer":
    case "RopFastTransferDestinationPutBufferExtended":
      payload.transfer(
        record,
        parameter("TransferDataSize"),
        operation,
        request.handle ?? invalidHandle,
      );
      return;
    case "RopTellVersion":
      record.bytes("Version", wireSizes.version);
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
      if ((parameter("NotificationTypes") & notificationFlags.extended) !== 0) {
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
        payload.release?.(request.handle);
      }
      return;
    case "RopModifyRecipients": {
      const columns = tags(record, parameter("ColumnCount"));
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
