import { bounded, nested, readEnum, readFields, readObjectId, repeat } from "./common.js";
import { ParseError } from "./model.js";
import { readTaggedValue } from "./properties.js";
import type { PropertyContext } from "./properties.js";
import type { RecordReader } from "./record.js";

function entryId(record: RecordReader, message: boolean): void {
  readFields(record, { Flags: "u32", ProviderUID: "guid" });
  readEnum(record, message ? "MessageType" : "FolderType", "StoreObjectType");
  record.scalar("DatabaseGuid", "guid");
  record.bytes("GlobalCounter", 6);
  record.integer("Pad", "u16");
  if (message) {
    record.scalar("MessageDatabaseGuid", "guid");
    record.bytes("MessageGlobalCounter", 6);
    record.integer("Pad2", "u16");
  }
}

function countedBytes(record: RecordReader, name: string, extended: boolean): number {
  const size = record.integer(`${name}Size`, extended ? "u8" : "u16");
  record.bytes(name, size);
  return size;
}

export function readRuleActions(record: RecordReader, context: PropertyContext): void {
  const extended = context !== "rop";
  const countType = extended ? "u32" : "u16";
  const count = record.integer("NoOfActions", countType);
  repeat(record, count, () => {
    const size = record.integer("ActionLength", countType);
    bounded(record, "Action", size, (action) => {
      const type = readEnum(action, "ActionType", "ActionType");
      readFields(action, { ActionFlavor: "u32", ActionFlags: "u32" });
      switch (type) {
        case 1:
        case 2: {
          const sameStore = extended ? false : action.boolean("FolderInThisStore");
          countedBytes(action, "StoreEID", extended);
          const size = action.integer("FolderEIDSize", extended ? "u8" : "u16");
          bounded(action, "FolderEID", size, (folder) => {
            if (extended) {
              entryId(folder, false);
            } else if (sameStore) {
              folder.boolean("Ours");
              nested(folder, "FolderId", readObjectId);
              readFields(folder, { MessageId: "u64", Instance: "i32" });
            } else {
              folder.bytes("EntryID", size);
            }
          });
          return;
        }
        case 3:
        case 4:
          if (extended) {
            const size = action.integer("MessageEIDSize", "u8");
            bounded(action, "ReplyTemplateMessageEID", size, (message) => entryId(message, true));
          } else {
            nested(action, "ReplyTemplateFID", readObjectId);
            nested(action, "ReplyTemplateMID", readObjectId);
          }
          action.scalar("ReplyTemplateGUID", "guid");
          return;
        case 5:
          action.bytes("Deferred action data", action.reader.remaining);
          return;
        case 6:
          readEnum(action, "BounceCode", "BounceCodeEnum");
          return;
        case 7:
        case 8: {
          const count = action.integer("RecipientCount", "u16");
          repeat(action, count, () =>
            nested(action, "Recipient", (recipient) => {
              recipient.integer("Reserved", "u8");
              const count = recipient.integer("NoOfProperties", "u16");
              repeat(recipient, count, () => nested(recipient, "Property", readTaggedValue), 4);
            }),
          );
          return;
        }
        case 9:
          nested(action, "Property", (property) => readTaggedValue(property, context));
          return;
        case 10:
        case 11:
          return;
        default:
          throw new ParseError(
            "unsupported",
            `Unsupported rule action ${type}`,
            action.reader.range(),
          );
      }
    });
  });
}
