import { readEnum } from "./common.js";
import { enums } from "./constants.js";
import { ParseError } from "./model.js";
import { RecordReader } from "./record.js";
import type { ScalarType } from "./record.js";

type AuxiliaryField = ScalarType | { readonly enum: string } | { readonly bytes: number };
type AuxiliaryLayout = Readonly<Record<string, AuxiliaryField>>;

const timing = { TimeSinceRequest: "u32", TimeToCompleteRequest: "u32" } as const;
const session = { ClientID: "u16", ServerID: "u16", SessionID: "u16" } as const;
const request = { ...session, RequestID: "u16" } as const;
const failure = {
  TimeSinceRequest: "u32",
  TimeToFailRequest: "u32",
  ResultCode: { enum: "ErrorCodes" },
  RequestOperation: "u8",
  Reserved: { bytes: 3 },
} as const;

const layouts: Readonly<Record<string, AuxiliaryLayout>> = {
  AUX_ENDPOINT_CAPABILITIES: { EndpointCapabilityFlag: { enum: "EndpointCapabilityFlag" } },
  AUX_CLIENT_CONTROL: { EnableFlags: { enum: "EnableFlags" }, ExpiryTime: "u32" },
  AUX_EXORGINFO: { OrgFlags: { enum: "OrgFlags" } },
  AUX_OSVERSIONINFO: {
    OSVersionInfoSize: "u32",
    MajorVersion: "u32",
    MinorVersion: "u32",
    BuildNumber: "u32",
    Reserved1: { bytes: 132 },
    ServicePackMajor: "u16",
    ServicePackMinor: "u16",
    Reserved2: "u32",
  },
  AUX_PERF_ACCOUNTINFO: { ClientID: "u16", Reserved: "u16", Account: "guid" },
  AUX_PERF_DEFGC_SUCCESS: {
    ServerID: "u16",
    SessionID: "u16",
    ...timing,
    RequestOperation: "u8",
    Reserved: { bytes: 3 },
  },
  AUX_PERF_DEFMDB_SUCCESS: { ...timing, RequestID: "u16", Reserved: "u16" },
  AUX_PERF_FAILURE: { ...request, ...failure },
  AUX_PERF_FAILURE_V2: { ProcessID: "u16", ...request, Reserved1: "u16", ...failure },
  AUX_PERF_GC_SUCCESS: {
    ...session,
    Reserved1: "u16",
    ...timing,
    RequestOperation: "u8",
    Reserved2: { bytes: 3 },
  },
  AUX_PERF_GC_SUCCESS_V2: {
    ProcessID: "u16",
    ...session,
    ...timing,
    RequestOperation: "u8",
    Reserved: { bytes: 3 },
  },
  AUX_PERF_MDB_SUCCESS: { ...request, ...timing },
  AUX_PERF_MDB_SUCCESS_V2: { ProcessID: "u16", ...request, Reserved: "u16", ...timing },
  AUX_PERF_REQUESTID: { SessionID: "u16", RequestID: "u16" },
  AUX_PERF_SESSIONINFO: { SessionID: "u16", Reserved: "u16", SessionGuid: "guid" },
  AUX_PERF_SESSIONINFO_V2: {
    SessionID: "u16",
    Reserved: "u16",
    SessionGuid: "guid",
    ConnectionID: "u32",
  },
  AUX_PERF_PROCESSINFO: {
    ProcessID: "u16",
    Reserved1: "u16",
    ProcessGuid: "guid",
    ProcessNameOffset: "u16",
    Reserved2: "u16",
  },
  AUX_PERF_SERVERINFO: {
    ServerID: "u16",
    ServerType: { enum: "ServerType" },
    ServerDNOffset: "u16",
    ServerNameOffset: "u16",
  },
  AUX_CLIENT_CONNECTION_INFO: {
    ConnectionGUID: "guid",
    OffsetConnectionContextInfo: "u16",
    Reserved: "u16",
    ConnectionAttempts: "u32",
    ConnectionFlags: { enum: "ConnectionFlags" },
  },
  AUX_SERVER_SESSION_INFO: { OffsetServerSessionContextInfo: "u16" },
  AUX_PROTOCOL_DEVICE_IDENTIFICATION: {
    DeviceManufacturerOffset: "u16",
    DeviceModelOffset: "u16",
    DeviceSerialNumberOffset: "u16",
    DeviceVersionOffset: "u16",
    DeviceFirmwareVersionOffset: "u16",
  },
  AUX_PERF_CLIENTINFO: {
    AdapterSpeed: "u32",
    ClientID: "u16",
    MachineNameOffset: "u16",
    UserNameOffset: "u16",
    ClientIPSize: "u16",
    ClientIPOffset: "u16",
    ClientIPMaskSize: "u16",
    ClientIPMaskOffset: "u16",
    AdapterNameOffset: "u16",
    MacAddressSize: "u16",
    MacAddressOffset: "u16",
    ClientMode: { enum: "ClientModeFlag" },
    Reserved: "u16",
  },
};

const strings: Readonly<Record<string, readonly string[]>> = {
  AUX_PERF_PROCESSINFO: ["ProcessName"],
  AUX_PERF_SERVERINFO: ["ServerDN", "ServerName"],
  AUX_PROTOCOL_DEVICE_IDENTIFICATION: [
    "DeviceManufacturer",
    "DeviceModel",
    "DeviceSerialNumber",
    "DeviceVersion",
    "DeviceFirmwareVersion",
  ],
  AUX_PERF_CLIENTINFO: ["MachineName", "UserName", "AdapterName"],
};

/** Offsets are relative to the bounded AUX_HEADER, never a capture or a file. */
export function readAuxiliaryBody(record: RecordReader, type: number, version: number): void {
  if (version !== 1 && version !== 2) {
    record.bytes("Auxiliary data", record.reader.remaining);
    record.field.issue = {
      kind: "unsupported",
      message: `Unsupported auxiliary version ${version}`,
    };
    return;
  }
  const definition = enums[version === 1 ? "AuxiliaryBlockType_1" : "AuxiliaryBlockType_2"]!;
  const symbolic = Object.entries(definition.values).find(([, value]) => value === type)?.[0];
  let name = symbolic?.replace("AUX_TYPE_", "AUX_").replace(/_PERF_(?:BG|FG)_/, "_PERF_");
  if (
    version === 2 &&
    name &&
    [
      "AUX_PERF_FAILURE",
      "AUX_PERF_GC_SUCCESS",
      "AUX_PERF_MDB_SUCCESS",
      "AUX_PERF_SESSIONINFO",
    ].includes(name)
  ) {
    name += "_V2";
  }
  if (name === "AUX_EXCEPTION_TRACE") {
    record.integer("RopIndex", "u32");
    while (record.reader.remaining !== 0) {
      record.line("Exception message", true);
    }
    return;
  }
  const layout = name ? layouts[name] : undefined;
  if (!layout) {
    record.bytes("Auxiliary data", record.reader.remaining);
    record.field.issue = {
      kind: "unsupported",
      message: `Unsupported auxiliary block ${symbolic ?? type}`,
    };
    return;
  }
  const numbers = new Map<string, number>();
  for (const [field, type] of Object.entries(layout)) {
    if (typeof type === "string") {
      const value = record.scalar(field, type);
      if (typeof value === "number") numbers.set(field, value);
    } else if ("enum" in type) {
      readEnum(record, field, type.enum);
    } else {
      record.bytes(field, type.bytes);
    }
  }
  const readAt = (name: string, offset: number, length?: number): void => {
    if (offset === 0) return;
    const absolute = record.field.range.offset + offset;
    if (absolute < record.reader.offset) {
      throw new ParseError(
        "malformed",
        "Auxiliary offset overlaps its fixed fields",
        record.reader.range(),
      );
    }
    const child = new RecordReader(record.reader.at(absolute), name);
    if (length === undefined) child.string(name, true);
    else child.bytes(name, length);
    record.add(child.finish());
  };
  for (const field of strings[name!] ?? []) {
    readAt(field, numbers.get(`${field}Offset`) ?? 0);
  }
  if (name === "AUX_CLIENT_CONNECTION_INFO") {
    readAt("ConnectionContextInfo", numbers.get("OffsetConnectionContextInfo") ?? 0);
  } else if (name === "AUX_SERVER_SESSION_INFO") {
    readAt("ServerSessionContextInfo", numbers.get("OffsetServerSessionContextInfo") ?? 0);
  } else if (name === "AUX_PERF_CLIENTINFO") {
    for (const field of ["ClientIP", "ClientIPMask", "MacAddress"]) {
      readAt(field, numbers.get(`${field}Offset`) ?? 0, numbers.get(`${field}Size`) ?? 0);
    }
  }
  if (record.reader.remaining !== 0) {
    record.bytes("Auxiliary storage", record.reader.remaining);
  }
}
