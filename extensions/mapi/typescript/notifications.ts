import { notificationFlags, tableEvents } from "./wire.js";
import { bounded, nested, readEnum, readObjectId, repeat } from "./common.js";
import type { Conversation } from "./conversation.js";
import { ParseError } from "./model.js";
import { readPropertyRow, readPropertyTag } from "./properties.js";
import type { RecordReader } from "./record.js";

const unspecifiedTagCount = 0xffff;

export function readNotification(record: RecordReader, conversation: Conversation): void {
  const handle = record.integer("NotificationHandle", "u32");
  record.integer("LogonId", "u8");
  const flags = readEnum(record, "NotificationFlags", "NotificationTypes");
  const message = (flags & notificationFlags.message) !== 0;
  const searchFolder = (flags & notificationFlags.searchFolder) !== 0;
  const created = (flags & notificationFlags.created) !== 0;
  const deleted = (flags & notificationFlags.deleted) !== 0;
  const modified = (flags & notificationFlags.modified) !== 0;
  const movedOrCopied = (flags & (notificationFlags.moved | notificationFlags.copied)) !== 0;
  const tableModified = (flags & notificationFlags.tableModified) !== 0;
  const extended = (flags & notificationFlags.extended) !== 0;
  if (tableModified) {
    const event = readEnum(record, "TableEventType", "TableEventType");
    const rowChanged =
      event === tableEvents.added ||
      event === tableEvents.deleted ||
      event === tableEvents.modified;
    const rowHasData = event === tableEvents.added || event === tableEvents.modified;
    if (rowChanged) {
      nested(record, "TableRowFolderID", readObjectId);
      if (message) {
        nested(record, "TableRowMessageID", readObjectId);
        record.integer("TableRowInstance", "u32");
      }
    }
    if (rowHasData && message) {
      nested(record, "InsertAfterTableRowFolderID", readObjectId);
      nested(record, "InsertAfterTableRowID", readObjectId);
      record.integer("InsertAfterTableRowInstance", "u32");
    }
    if (rowHasData) {
      const size = record.integer("TableRowDataSize", "u16");
      const tags = conversation.columns.get(handle);
      if (!tags) {
        throw new ParseError(
          "missing-context",
          "Missing table columns for notification",
          record.reader.range(),
        );
      }
      bounded(record, "TableRowData", size, (child) => readPropertyRow(child, tags, false));
    }
  }
  if (!tableModified && !extended) {
    nested(record, "FolderId", readObjectId);
    if (message) {
      nested(record, "MessageId", readObjectId);
    }
  }
  if (
    (created || deleted || movedOrCopied) &&
    ((searchFolder && message) || (!searchFolder && !message))
  ) {
    nested(record, "ParentFolderId", readObjectId);
  }
  if (movedOrCopied) {
    nested(record, "OldFolderId", readObjectId);
    nested(record, message ? "OldMessageId" : "OldParentFolderId", readObjectId);
  }
  if (created || modified) {
    const count = record.integer("TagCount", "u16");
    if (count !== unspecifiedTagCount) {
      repeat(record, count, () => nested(record, "Tag", readPropertyTag), 4);
    }
  }
  if ((flags & notificationFlags.totalCount) !== 0) {
    record.integer("TotalMessageCount", "u32");
  }
  if ((flags & notificationFlags.unreadCount) !== 0) {
    record.integer("UnreadMessageCount", "u32");
  }
  if ((flags & notificationFlags.newMail) !== 0) {
    record.integer("MessageFlags", "u32");
    const unicode = record.integer("UnicodeFlag", "u8");
    if (unicode !== 0 && unicode !== 1) {
      throw new ParseError(
        "malformed",
        "Invalid notification string encoding",
        record.reader.range(),
      );
    }
    record.string("MessageClass", unicode === 1);
  }
}
