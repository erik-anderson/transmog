import assert from "node:assert/strict";
import { test } from "node:test";
import * as body from "../dist/body.js";
import { inputLimits } from "../dist/limits.js";
import { BinaryReader } from "../dist/reader.js";
import { enums } from "../dist/constants.js";
import { wireSizes } from "../dist/wire.js";

const selectedId = 1000;
const u16 = (value) => [value & 255, value >>> 8];
const u32 = (value) => [value & 255, (value >>> 8) & 255, (value >>> 16) & 255, value >>> 24];
const rop = { release: 0x01, getPropertiesAll: 0x08, putBuffer: 0x54 };
const rpcLastBuffer = 4;
const handle = 0x12345678;
const maximumRopOutputBytes = 65_536;
const maximumContinuationSteps = 1024;

function execute(operation, direction = "request") {
  const payload = [...u16(operation.length + 2), ...operation, ...u32(handle)];
  const rpc = [
    ...u16(0),
    ...u16(rpcLastBuffer),
    ...u16(payload.length),
    ...u16(payload.length),
    ...payload,
  ];
  return direction === "request"
    ? [...u32(0), ...u32(rpc.length), ...rpc, ...u32(maximumRopOutputBytes), ...u32(0)]
    : [13, 10, ...u32(0), ...u32(0), ...u32(0), ...u32(rpc.length), ...rpc, ...u32(0)];
}

function input(id, direction = "request", operation = "Execute") {
  const headers = (values) =>
    Object.entries(values).map(([name, value], index) => ({
      index,
      name,
      redacted: false,
      value: Array.from(Buffer.from(value, "ascii")),
    }));
  return {
    instanceId: "test",
    exchangeId: String(id),
    requestNumber: String(id),
    generation: "1",
    direction,
    request: {
      evidenceId: `request-${id}`,
      headers: headers({ "content-type": "application/mapi-http", "x-requesttype": operation }),
    },
    response: {
      evidenceId: `response-${id}`,
      headers: headers({ "content-type": "application/mapi-http", "x-responsecode": "0" }),
    },
  };
}

function attach(
  message,
  side,
  bytes,
  sources,
  { revision = 0, availability = "complete", encoded = false } = {},
) {
  const sourceId = `${message.exchangeId}.${side}.${revision}`;
  sources.set(sourceId, bytes);
  message[side].body = { sourceId, length: String(bytes.length), availability };
  if (encoded) {
    message[side].headers.push({
      index: message[side].headers.length,
      name: "content-encoding",
      redacted: false,
      value: Array.from(Buffer.from("gzip")),
    });
  }
}

function readPages(step, sources) {
  let reads = 0;
  while (step.kind === "needInput") {
    assert.ok(++reads <= maximumContinuationSteps, "Body acquisition did not terminate");
    step = body.resume(
      step.token,
      step.reads.map((read) => ({
        sourceId: read.sourceId,
        offset: read.offset,
        bytes: sources
          .get(read.sourceId)
          .slice(Number(read.offset), Number(read.offset) + read.maxBytes),
      })),
    );
  }
  return step;
}

function event(message, revision = 1) {
  return {
    sequence: "1",
    exchangeId: message.exchangeId,
    direction: message.direction,
    phase: "bodyCompleted",
    revision: String(revision),
    input: message,
  };
}

function transferInput(sources) {
  const message = input(selectedId);
  attach(message, "request", execute([rop.putBuffer, 7, 0, ...u16(0)]), sources);
  return message;
}

test("per-user writes acquire logon context only when their layout needs it", () => {
  const logonId = 7;
  const logon = [enums.RopIdType.values.RopLogon, logonId, 0, 0, ...u32(0), ...u32(0), ...u16(0)];
  for (const [offset, includeLogon] of [
    [0, false],
    [0, true],
    [1, false],
  ]) {
    const sources = new Map();
    const message = input(selectedId);
    const write = [
      enums.RopIdType.values.RopWritePerUserInformation,
      logonId,
      0,
      ...Array(wireSizes.longTermId).fill(0),
      1,
      ...u32(offset),
      ...u16(0),
    ];
    attach(message, "request", execute([...(includeLogon ? logon : []), ...write]), sources);
    let step = readPages(body.visualize(message), sources);
    if (offset === 0 && !includeLogon) {
      assert.equal(step.kind, "needContext");
      const history = input(1);
      attach(history, "request", execute(logon), sources);
      step = readPages(
        body.resumeContext(step.token, {
          anchorOrder: String(selectedId),
          exchanges: [{ order: history.requestNumber, input: history }],
          status: "complete",
        }),
        sources,
      );
    }
    assert.equal(
      step.kind,
      "document",
      `Unexpected context acquisition for offset ${offset}, local logon ${includeLogon}`,
    );
    assert.equal(step.document.status, "complete");
    assert.equal(step.document.updates, "final");
  }
});

test("FastTransfer setup and self-contained read flags do not acquire unrelated history", () => {
  const logonId = 7;
  const operations = [
    [
      enums.RopIdType.values.RopFastTransferDestinationConfigure,
      logonId,
      0,
      0,
      enums.SourceOperation.values.CopyTo,
      0,
    ],
    [
      enums.RopIdType.values.RopLogon,
      logonId,
      0,
      enums.LogonFlags.values.Private,
      ...u32(0),
      ...u32(0),
      ...u16(0),
      enums.RopIdType.values.RopSetMessageReadFlag,
      logonId,
      0,
      0,
      0,
    ],
  ];
  for (const operation of operations) {
    const sources = new Map();
    const message = input(selectedId);
    attach(message, "request", execute(operation), sources);
    const step = readPages(body.visualize(message), sources);
    assert.equal(step.kind, "document");
    assert.equal(step.document.status, "complete");
  }
});

test("selected HTTP decoding distinguishes resource limits from unavailable codecs", () => {
  for (const [reason, status] of [
    ["limited", "limited"],
    ["unsupported", "unavailable"],
  ]) {
    const sources = new Map();
    const message = input(selectedId, "request", "Connect");
    attach(message, "request", [0], sources, { encoded: true });
    const step = body.visualize(message);
    assert.equal(step.kind, "needDecoded");
    const result = body.resumeDecoded(step.token, { kind: "unavailable", reason });
    assert.equal(result.kind, "document");
    assert.equal(result.document.status, status);
  }
});

test("paired HTTP decoding limits retain the selected response with Limited status", () => {
  const sources = new Map();
  const message = input(selectedId, "response");
  attach(
    message,
    "response",
    execute([rop.getPropertiesAll, 0, ...u32(0), ...u16(0)], "response"),
    sources,
  );
  attach(message, "request", [0], sources, { encoded: true });
  const step = readPages(body.visualize(message), sources);
  assert.equal(step.kind, "needDecoded");
  const result = body.resumeDecoded(step.token, { kind: "unavailable", reason: "limited" });
  assert.equal(result.kind, "document");
  assert.equal(result.document.status, "limited");
  assert.ok(result.document.roots.length > 0);
  assert.ok(result.document.diagnostics.some((item) => item.code === "http-decoding-limited"));
});

test("context replies distinguish resource limits from unavailable history", () => {
  for (const [status, expected] of [
    ["limited", "limited"],
    ["unavailable", "partial"],
  ]) {
    const sources = new Map();
    const step = readPages(body.visualize(transferInput(sources)), sources);
    assert.equal(step.kind, "needContext");
    const result = body.resumeContext(step.token, {
      anchorOrder: String(selectedId),
      exchanges: [],
      status,
    });
    assert.equal(result.kind, "document");
    assert.equal(result.document.status, expected);
    assert.ok(result.document.roots.length > 0);
  }
});

test("a full context window reports Limited when more history remains", () => {
  const sources = new Map();
  let step = readPages(body.visualize(transferInput(sources)), sources);
  let count = 0;
  while (step.kind === "needContext") {
    assert.ok(step.request.limit > 0);
    const exchanges = Array.from({ length: step.request.limit }, () => {
      const message = input(++count);
      return { order: message.requestNumber, input: message };
    });
    step = body.resumeContext(step.token, {
      anchorOrder: String(selectedId),
      exchanges,
      status: "more",
      nextCursor: `cursor-${count}`,
    });
  }
  assert.equal(count, inputLimits.relatedExchanges);
  assert.equal(step.kind, "document");
  assert.equal(step.document.status, "limited");
});

test("live context filling the window never requests a zero-sized history page", () => {
  const sources = new Map();
  const initial = input(selectedId);
  attach(initial, "request", execute([rop.release, 7, 0]), sources, { availability: "capturing" });
  assert.equal(readPages(body.visualize(initial), sources).kind, "document");
  const history = Array.from({ length: inputLimits.relatedExchanges }, (_, index) =>
    event(input(index + 1)),
  );
  assert.equal(body.onTraffic(history, { missedHints: "0" }).kind, "document");

  const updated = transferInput(sources);
  const updatedBytes = sources.get(updated.request.body.sourceId);
  attach(updated, "request", updatedBytes, sources, { revision: 2 });
  const result = readPages(body.onTraffic([event(updated, 2)], { missedHints: "0" }), sources);
  assert.equal(result.kind, "document");
  assert.equal(result.document.status, "limited");
  assert.ok(result.document.roots.length > 0);
});

test("Connect viewers ignore earlier Execute updates and their body limits", () => {
  const sources = new Map();
  const selected = input(selectedId, "request", "Connect");
  const connect = [0, ...u32(0), ...u32(0), ...u32(0), ...u32(0), ...u32(0)];
  attach(selected, "request", connect, sources, { availability: "capturing" });
  const initial = readPages(body.visualize(selected), sources);
  assert.equal(initial.kind, "document");
  assert.equal(initial.document.status, "partial");

  const earlier = input(1);
  attach(earlier, "request", [0], sources);
  earlier.request.body.length = String(inputLimits.snapshotBytes + 1);
  const result = body.onTraffic([event(earlier)], { missedHints: "0" });
  assert.equal(result, initial, "Irrelevant traffic changed the selected interpretation");
});

test("authoring exceptions reach the host without triggering context acquisition", () => {
  const sources = new Map();
  const selected = input(selectedId);
  attach(selected, "request", execute([rop.release, 7, 0]), sources);
  const step = body.visualize(selected);
  assert.equal(step.kind, "needInput");
  const fault = new Error("Synthetic authoring failure");
  const integer = BinaryReader.prototype.integer;
  try {
    BinaryReader.prototype.integer = () => {
      throw fault;
    };
    assert.throws(
      () => readPages(step, sources),
      (error) => error === fault,
    );
  } finally {
    BinaryReader.prototype.integer = integer;
  }
  const recovered = readPages(body.visualize(selected), sources);
  assert.equal(recovered.kind, "document");
  assert.equal(recovered.document.status, "complete");
});
