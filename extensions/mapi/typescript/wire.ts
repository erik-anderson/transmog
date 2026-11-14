/** Named wire discriminators. Enum values come from the pinned generated tables. */
import { enums } from "./constants.js";

export const propertyTypes = {
  unspecified: enums.PropertyDataType!.values.PtypUnspecified!,
  null: enums.PropertyDataType!.values.PtypNull!,
  integer16: enums.PropertyDataType!.values.PtypInteger16!,
  integer32: enums.PropertyDataType!.values.PtypInteger32!,
  float32: enums.PropertyDataType!.values.PtypFloating32!,
  float64: enums.PropertyDataType!.values.PtypFloating64!,
  currency: enums.PropertyDataType!.values.PtypCurrency!,
  floatingTime: enums.PropertyDataType!.values.PtypFloatingTime!,
  error: enums.PropertyDataType!.values.PtypErrorCode!,
  boolean: enums.PropertyDataType!.values.PtypBoolean!,
  object: enums.PropertyDataType!.values.PtypObject_Or_PtypEmbeddedTable!,
  integer64: enums.PropertyDataType!.values.PtypInteger64!,
  string: enums.PropertyDataType!.values.PtypString!,
  string8: enums.PropertyDataType!.values.PtypString8!,
  time: enums.PropertyDataType!.values.PtypTime!,
  guid: enums.PropertyDataType!.values.PtypGuid!,
  serverId: enums.PropertyDataType!.values.PtypServerId!,
  restriction: enums.PropertyDataType!.values.PtypRestriction!,
  ruleAction: enums.PropertyDataType!.values.PtypRuleAction!,
  binary: enums.PropertyDataType!.values.PtypBinary!,
  multiValue: enums.PropertyDataType!.values.MultiValue!,
  multiValueInstance: enums.PropertyDataType!.values.MultivalueInstance!,
} as const;

export const restrictionTypes = {
  and: enums.RestrictTypeEnum!.values.AndRestriction!,
  or: enums.RestrictTypeEnum!.values.OrRestriction!,
  not: enums.RestrictTypeEnum!.values.NotRestriction!,
  content: enums.RestrictTypeEnum!.values.ContentRestriction!,
  property: enums.RestrictTypeEnum!.values.PropertyRestriction!,
  compareProperties: enums.RestrictTypeEnum!.values.ComparePropertiesRestriction!,
  bitMask: enums.RestrictTypeEnum!.values.BitMaskRestriction!,
  size: enums.RestrictTypeEnum!.values.SizeRestriction!,
  exist: enums.RestrictTypeEnum!.values.ExistRestriction!,
  subObject: enums.RestrictTypeEnum!.values.SubObjectRestriction!,
  comment: enums.RestrictTypeEnum!.values.CommentRestriction!,
  count: enums.RestrictTypeEnum!.values.CountRestriction!,
} as const;

export const ruleActions = {
  move: enums.ActionType!.values.OP_MOVE!,
  copy: enums.ActionType!.values.OP_COPY!,
  reply: enums.ActionType!.values.OP_REPLY!,
  outOfOfficeReply: enums.ActionType!.values.OP_OOF_REPLY!,
  defer: enums.ActionType!.values.OP_DEFER_ACTION!,
  bounce: enums.ActionType!.values.OP_BOUNCE!,
  forward: enums.ActionType!.values.OP_FORWARD!,
  delegate: enums.ActionType!.values.OP_DELEGATE!,
  tag: enums.ActionType!.values.OP_TAG!,
  delete: enums.ActionType!.values.OP_DELETE!,
  markAsRead: enums.ActionType!.values.OP_MARK_AS_READ!,
} as const;

export const stringTypes = {
  absent: enums.StringTypeEnum!.values.NoPresent!,
  empty: enums.StringTypeEnum!.values.Empty!,
  ascii: enums.StringTypeEnum!.values.CharacterString!,
  reducedUnicode: enums.StringTypeEnum!.values.ReducedUnicodeCharacterString!,
  unicode: enums.StringTypeEnum!.values.UnicodeCharacterString!,
} as const;

export const rpcFlags = {
  compressed: enums.RpcHeaderFlags!.values.Compressed!,
  xor: enums.RpcHeaderFlags!.values.XorMagic!,
  last: enums.RpcHeaderFlags!.values.Last!,
} as const;

export const notificationFlags = {
  newMail: enums.NotificationTypes!.values.NewMail!,
  created: enums.NotificationTypes!.values.ObjectCreated!,
  deleted: enums.NotificationTypes!.values.ObjectDeleted!,
  modified: enums.NotificationTypes!.values.ObjectModified!,
  moved: enums.NotificationTypes!.values.ObjectMoved!,
  copied: enums.NotificationTypes!.values.ObjectCopied!,
  tableModified: enums.NotificationTypes!.values.TableModified!,
  extended: enums.NotificationTypes!.values.Extended!,
  totalCount: enums.NotificationTypes!.values.T!,
  unreadCount: enums.NotificationTypes!.values.U!,
  searchFolder: enums.NotificationTypes!.values.S!,
  message: enums.NotificationTypes!.values.M!,
} as const;

export const tableEvents = {
  added: enums.TableEventType!.values.TableRowAdded!,
  deleted: enums.TableEventType!.values.TableRowDeleted!,
  modified: enums.TableEventType!.values.TableRowModified!,
} as const;

export const privateMailboxFlag = enums.LogonFlags!.values.Private!;
export const mailboxLogonFolderCount = 13;
export const invalidHandle = 0xffffffff;
export const extendedBufferSize = 0xbabe;
export const mapiXorMask = 0xa5;
export const namedPropertyIdStart = 0x8000;
export const propertyNameKinds = {
  numeric: enums.KindEnum!.values.LID!,
  string: enums.KindEnum!.values.Name!,
  none: enums.KindEnum!.values.NoPropertyName!,
} as const;
export const propertyRowFlags = { ordinary: 0, flagged: 1, absent: 1, error: 0x0a } as const;
export const wireSizes = {
  propertyTag: 4,
  objectId: 8,
  globalCounter: 6,
  longTermId: 24,
  longTermIdRange: 48,
  handle: 4,
  ropSize: 2,
  auxiliaryHeader: 4,
  version: 6,
  clientData: 24,
} as const;

export const recipientFlags = {
  addressTypeShift: 5,
  addressTypeMask: 7,
  hasAddressType: 0x0100,
  unicode: 0x4000,
  emailAddress: 0x10,
  displayName: 8,
  simpleDisplayName: 0x2000,
  transmittableDisplayName: 4,
} as const;
export const recipientAddressTypes = {
  none: 0,
  exchange: 1,
  personalDistributionList: 6,
  distributionList: 7,
} as const;
export const auxiliaryVersions = { original: 1, extended: 2 } as const;
export const ephemeralEntryIdType = 0x87;
export const globsetCommands = { end: 0, pop: 0x50, bitmask: 0x42, range: 0x52 } as const;

export const propertyIds = {
  message: enums.PidTagPropertyEnum!.values.PidTagMid!,
  folder: enums.PidTagPropertyEnum!.values.PidTagFolderId!,
  messageClass: enums.PidTagPropertyEnum!.values.PidTagMessageClass!,
  lastModificationTime: enums.PidTagPropertyEnum!.values.PidTagLastModificationTime!,
  // This additional FastTransfer identifier is decoded with the same ObjectId layout.
  additionalObjectId: 0x6714,
} as const;
