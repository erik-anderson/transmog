import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import vm from "node:vm";

const sources = Object.fromEntries(
  await Promise.all(
    ["protobuf", "bond"].map(async (name) => [
      name,
      await readFile(new URL(`../dist/runtime/${name}.js`, import.meta.url), "utf8"),
    ]),
  ),
);

function freeze(value) {
  if (value && typeof value === "object") {
    Object.freeze(value);
    for (const child of Object.values(value)) freeze(child);
  }
  return value;
}

async function host(kind, payload, options = {}) {
  const context = vm.createContext({}, { codeGeneration: { strings: false, wasm: false } });
  const module = new vm.SourceTextModule(sources[kind], { context });
  await module.link(() => {
    throw new Error("Bundled entry acquired an external import");
  });
  await module.evaluate({ timeout: 2_000 });
  context.handlers = module.namespace;
  function call(method, ...args) {
    context.method = method;
    context.args = freeze(args);
    return JSON.parse(
      vm.runInContext("JSON.stringify(handlers[method](...args))", context, { timeout: 2_000 }),
    );
  }
  const direction = options.direction ?? "request";
  const headers =
    options.headers ??
    (options.compressed
      ? [
          {
            index: 0,
            name: "content-encoding",
            redacted: false,
            value: Array.from(Buffer.from("gzip")),
          },
        ]
      : []);
  const input = {
    instanceId: "test",
    exchangeId: "1",
    requestNumber: "9007199254740993",
    generation: "1",
    direction,
    [direction]: {
      evidenceId: "evidence",
      headers,
      ...(options.missing
        ? {}
        : {
            body: {
              sourceId: "encoded",
              length: String(options.length ?? payload.length),
              availability: options.availability ?? "complete",
            },
          }),
    },
  };
  let step = call("visualize", input);
  let reads = 0;
  let decodes = 0;
  for (let count = 0; count < 100; count++) {
    if (step.kind === "needDecoded") {
      decodes++;
      assert.equal(step.sourceId, "encoded");
      step = call(
        "resumeDecoded",
        step.token,
        options.decodeFailure
          ? { kind: "unavailable", reason: options.decodeFailure }
          : {
              kind: "ready",
              decoded: {
                body: {
                  sourceId: "decoded",
                  length: String(payload.length),
                  availability: options.availability ?? "complete",
                },
                parent: { sourceId: "encoded", offset: "0", length: "1" },
                codings: ["gzip"],
              },
            },
      );
    } else if (step.kind === "needInput") {
      reads++;
      assert.equal(step.reads.length, 1);
      const read = step.reads[0];
      assert.ok(read.maxBytes > 0 && read.maxBytes <= 65_536);
      assert.equal(read.sourceId, options.compressed ? "decoded" : "encoded");
      const page = Array.from(
        payload.slice(Number(read.offset), Number(read.offset) + read.maxBytes),
      );
      if (options.shortPage) page.pop();
      step = call("resume", step.token, [
        { sourceId: read.sourceId, offset: read.offset, bytes: page },
      ]);
    } else {
      assert.equal(step.kind, "document");
      validate(step.document, payload.length, options.compressed ? "decoded" : "encoded");
      return { document: step.document, reads, decodes, call, input };
    }
  }
  throw new Error("Unbounded continuation sequence");
}

function nodes(document) {
  const walk = (node) => [node, ...(node.children ?? []).flatMap(walk)];
  return document.roots.flatMap(walk);
}

function validate(document, length, sourceId) {
  assert.ok(
    ["complete", "partial", "malformed", "limited", "unavailable"].includes(document.status),
  );
  assert.equal(document.updates, "final");
  assert.equal(document.presentation, "tree");
  const all = nodes(document);
  assert.ok(all.length <= 8_010);
  assert.equal(new Set(all.map((node) => node.id)).size, all.length);
  for (const node of all) {
    assert.match(node.id, /^[a-zA-Z0-9._-]+$/);
    if (node.value?.kind === "integer")
      assert.equal(String(BigInt(node.value.value)), node.value.value);
    if (node.value?.kind === "float") assert.ok(Number.isFinite(node.value.value));
    for (const span of node.spans ?? []) {
      assert.equal(span.sourceId, sourceId);
      assert.ok(Number(span.offset) >= 0 && Number(span.length) >= 0);
      assert.ok(Number(span.offset) + Number(span.length) <= length);
    }
  }
  const depth = (node) => 1 + Math.max(0, ...(node.children ?? []).map(depth));
  assert.ok(Math.max(0, ...document.roots.map(depth)) <= 32);
  assert.ok(Buffer.byteLength(JSON.stringify(document)) < 4 * 1024 * 1024);
}

const hex = (value) => Uint8Array.from(Buffer.from(value.replaceAll(" ", ""), "hex"));
const varint = (raw) => {
  let value = BigInt(raw);
  const result = [];
  do {
    result.push(Number(value & 127n) | (value > 127n ? 128 : 0));
    value >>= 7n;
  } while (value);
  return result;
};
const field = (id, type, payload) => [...varint(id * 8 + type), ...payload];
const binary = (...parts) => Uint8Array.from(parts.flat());
const bondHeader = (version) => [0x43, 0x42, version, 0];
const bondStruct = (version, fields) =>
  version === 1 ? [...fields, 0] : [...varint(fields.length + 1), ...fields, 0];

test("published Protobuf examples retain duplicate order and label alternatives", async () => {
  const payload = hex("08 96 01 12 07 74 65 73 74 69 6e 67 08 01 1a 03 08 96 01");
  const { document } = await host("protobuf", payload);
  assert.equal(document.status, "complete");
  assert.deepEqual(
    document.roots.map((node) => node.name.split(" · ")[0]),
    ["Field 1", "Field 2", "Field 1", "Field 3"],
  );
  assert.equal(document.roots[0].value.value, "150");
  const all = nodes(document);
  assert.ok(
    all.some((node) => node.name === "Possible UTF-8 text" && node.value.value === "testing"),
  );
  assert.ok(all.some((node) => node.name === "Possible embedded message"));
  assert.ok(
    all.some((node) => node.name === "Possible ZigZag signed integer" && node.value.value === "75"),
  );
  assert.ok(all.some((node) => node.name === "Possible Boolean" && node.value.value === true));
  assert.deepEqual(document.roots[0].spans[0], { sourceId: "encoded", offset: "0", length: "3" });
});

test("Protobuf wide integers and fixed bit patterns remain exact", async () => {
  const { document } = await host(
    "protobuf",
    binary(
      field(1, 0, varint(18446744073709551615n)),
      field(2, 1, Array(8).fill(255)),
      field(3, 5, [0, 0, 128, 63]),
    ),
  );
  assert.equal(document.status, "complete");
  assert.equal(document.roots[0].value.value, "18446744073709551615");
  assert.equal(document.roots[1].value.value, "18446744073709551615");
  assert.ok(
    nodes(document).some(
      (node) => node.name === "Possible signed int64" && node.value.value === "-1",
    ),
  );
  assert.ok(
    nodes(document).some((node) => node.name === "Possible double" && node.value.value === "NaN"),
  );
  assert.ok(
    nodes(document).some((node) => node.name === "Possible float" && node.value.value === 1),
  );
});

test("Protobuf groups validate matching tags; invalid nested probes leave opaque bytes", async () => {
  assert.equal((await host("protobuf", hex("0b 10 01 0c"))).document.status, "complete");
  for (const payload of [
    "0b 10 01 14",
    "0b 10 01",
    "0c",
    "00",
    "0e",
    "0f",
    "08 80",
    "08 ffffffffffffffffff02",
    "12 80 80 80 80 10",
    "12 05 01",
  ]) {
    assert.equal((await host("protobuf", hex(payload))).document.status, "malformed", payload);
  }
  const { document } = await host("protobuf", hex("12 04 ff 80 c0 af"));
  assert.equal(document.status, "complete");
  assert.deepEqual(
    document.roots[0].children.map((node) => node.name),
    ["Raw payload"],
  );
});

test("malformed input preserves earlier complete fields and exposes the exact remainder", async () => {
  const { document } = await host("protobuf", hex("08 01 12 05 61"));
  assert.equal(document.status, "malformed");
  assert.equal(document.roots[0].value.value, "1");
  assert.deepEqual(document.roots.at(-1).spans[0], {
    sourceId: "encoded",
    offset: "2",
    length: "3",
  });
});

test("Bond raw v1 fixture decodes an encoded string, while labeling the inferred version", async () => {
  const { document } = await host("bond", hex("29 03 33 2e 30 00"));
  assert.equal(document.status, "complete");
  assert.match(document.roots[0].name, /v1.*inferred/);
  assert.ok(
    nodes(document).some((node) => node.name === "Field 1 · string" && node.value.value === "3.0"),
  );
  assert.ok(document.diagnostics.some((item) => item.code === "inferred-version"));
});

test("Bond marshaled v1 and v2 decode types, extended IDs and base boundaries", async () => {
  for (const version of [1, 2]) {
    const fields = [
      0x02,
      1,
      0x2e,
      255,
      0x4f,
      3,
      0xd0,
      42,
      ...varint(5),
      0xe6,
      0x34,
      0x12,
      ...varint(9007199254740993n),
      1,
      0x29,
      0,
    ];
    const { document } = await host(
      "bond",
      binary(bondHeader(version), bondStruct(version, fields)),
    );
    assert.equal(document.status, "complete", JSON.stringify(document.diagnostics));
    const all = nodes(document);
    assert.ok(all.some((node) => node.name === "Field 0 · bool" && node.value.value === true));
    assert.ok(all.some((node) => node.name === "Field 1 · int8" && node.value.value === "-1"));
    assert.ok(all.some((node) => node.name === "Field 2 · int16" && node.value.value === "-2"));
    assert.ok(all.some((node) => node.name === "Field 42 · int32" && node.value.value === "-3"));
    assert.ok(
      all.some(
        (node) => node.name === "Field 4660 · uint64" && node.value.value === "9007199254740993",
      ),
    );
    assert.ok(all.some((node) => node.name === "End of base struct"));
    assert.ok(!document.diagnostics.some((item) => item.code === "inferred-version"));
  }
});

test("Bond containers and nested structs honor version-specific lengths", async () => {
  for (const version of [1, 2]) {
    const list = version === 2 ? [0x6f, 1, 4] : [15, 2, 1, 4];
    const fields = [
      0x0b,
      ...list,
      0x2d,
      9,
      5,
      1,
      1,
      120,
      9,
      0x4a,
      ...bondStruct(version, [0x03, 7]),
    ];
    const { document } = await host(
      "bond",
      binary(bondHeader(version), bondStruct(version, fields)),
    );
    assert.equal(document.status, "complete", JSON.stringify(document.diagnostics));
    assert.ok(
      nodes(document).some((node) => node.name === "[0] · int16" && node.value.value === "-1"),
    );
    assert.ok(
      nodes(document).some((node) => node.name === "Key · string" && node.value.value === "x"),
    );
    assert.ok(
      nodes(document).some((node) => node.name === "Value · uint32" && node.value.value === "9"),
    );
    assert.ok(
      nodes(document).some((node) => node.name === "Field 0 · uint8" && node.value.value === "7"),
    );
  }
  const raw = await host("bond", binary(bondStruct(2, [0x29, 1, 120])));
  assert.equal(raw.document.status, "complete");
  assert.match(raw.document.roots[0].name, /v2.*inferred/);
});

test("Bond UTF-8, UTF-16 and nonfinite floats preserve valid values", async () => {
  const fields = [
    9, 4, 0xf0, 0x9f, 0x98, 0x80, 0x32, 2, 0x3d, 0xd8, 0x00, 0xde, 0x47, 0, 0, 128, 127,
  ];
  const { document } = await host("bond", binary(bondHeader(1), bondStruct(1, fields)));
  assert.equal(document.status, "complete");
  assert.equal(nodes(document).filter((node) => node.value?.value === "😀").length, 2);
  assert.ok(nodes(document).some((node) => node.value?.value === "Infinity"));
  for (const bad of [
    "09 02 c0 af 00",
    "12 01 00 d8 00",
    "02 02 00",
    "07 00 00",
    "0b 00 01 00",
    "05 ff ff ff ff 10 00",
    "00 01",
    "20",
  ]) {
    assert.equal(
      (await host("bond", binary(bondHeader(1), Array.from(hex(bad))))).document.status,
      "malformed",
      bad,
    );
  }
});

test("Bond rejects truncated headers, unsupported versions and incorrect v2 lengths", async () => {
  assert.equal((await host("bond", hex("43 42"))).document.status, "malformed");
  assert.equal((await host("bond", hex("43 42 03 00 00"))).document.status, "partial");
  for (const fields of ["00", "02 00 00", "01 01", "05 29 01 78 00"]) {
    assert.notEqual(
      (await host("bond", binary(bondHeader(2), Array.from(hex(fields))))).document.status,
      "complete",
      fields,
    );
  }
});

test("ambiguous Bond versions and header collisions remain explicitly partial", async () => {
  const versions = (await host("bond", hex("02 01 00"))).document;
  assert.equal(versions.status, "partial");
  assert.deepEqual(
    versions.roots.map((node) => node.name),
    ["Possible Compact Binary v1", "Possible Compact Binary v2"],
  );
  const framing = (await host("bond", hex("43 42 03 00 00"))).document;
  assert.equal(framing.status, "partial");
  assert.ok(framing.diagnostics.some((item) => item.code === "ambiguous-framing"));
  assert.ok(
    nodes(framing).some((node) => node.name === "Field 2 · uint8" && node.value.value === "66"),
  );
});

test("Bond cannot resolve a version or framing by exhausting the alternative's budget", async () => {
  // These bytes are valid raw v1 and v2. Each empty v2 struct is also a v1
  // struct with an empty base, costing twice as many v1 display nodes.
  const payload = binary(
    varint(6_661),
    Array.from({ length: 2_220 }, () => [10, 1, 0]).flat(),
    [0],
  );
  const v1 = (await host("bond", binary(bondHeader(1), Array.from(payload)))).document;
  assert.equal(v1.status, "limited");
  const raw = (await host("bond", payload)).document;
  assert.equal(raw.status, "limited");
  assert.ok(raw.diagnostics.some((item) => item.code === "version-limited"));
  assert.ok(nodes(raw).some((node) => node.name === "Field 0 · struct"));
  const marshaled = (await host("bond", binary(bondHeader(2), Array.from(payload)))).document;
  assert.equal(marshaled.status, "limited");
  assert.ok(marshaled.diagnostics.some((item) => item.code === "framing-limited"));
});

test("selected-side paging, HTTP decoding and unavailable evidence stay distinct", async () => {
  const payload = binary(field(1, 2, [...varint(70_000), ...Array(70_000).fill(120)]));
  for (const kind of ["protobuf", "bond"]) {
    const data = kind === "protobuf" ? payload : hex("29 03 33 2e 30 00");
    const result = await host(kind, data, { compressed: true, direction: "response" });
    assert.equal(result.document.status, "complete");
    assert.equal(result.decodes, 1);
    assert.equal(result.reads, kind === "protobuf" ? 2 : 1);
    assert.equal((await host(kind, data, { missing: true })).document.status, "unavailable");
    assert.equal((await host(kind, data, { shortPage: true })).document.status, "unavailable");
    assert.equal(
      (await host(kind, data, { availability: "evicted" })).document.status,
      "unavailable",
    );
    assert.equal((await host(kind, data, { length: 2_097_153 })).document.status, "limited");
    for (const reason of [
      "malformed",
      "unsupported",
      "metadataUnavailable",
      "limited",
      "incomplete",
    ]) {
      const failed = await host(kind, data, { compressed: true, decodeFailure: reason });
      assert.equal(failed.reads, 0);
      assert.notEqual(failed.document.status, "complete");
    }
  }
});

test("incomplete bodies never produce complete documents, including at valid field boundaries", async () => {
  for (const [kind, payload] of [
    ["protobuf", hex("08 01 12 03 61 62 63")],
    ["bond", hex("29 03 33 2e 30 00")],
  ]) {
    for (let end = 0; end <= payload.length; end++) {
      const { document } = await host(kind, payload.slice(0, end), { availability: "truncated" });
      assert.equal(document.status, "partial");
      assert.ok(document.diagnostics.some((item) => item.code === "incomplete-evidence"));
    }
  }
});

test("continuations expire when a new visualization starts", async () => {
  const result = await host("protobuf", hex("08 01"));
  const old = result.call("visualize", result.input);
  result.call("visualize", result.input);
  assert.throws(() => result.call("resume", old.token, []), /Expired/);
});

test("identity bypass accepts only one valid HTTP identity token", async () => {
  const header = (value) => ({
    index: 0,
    name: "content-encoding",
    redacted: false,
    value: Array.from(Buffer.from(value, "latin1")),
  });
  for (const [kind, payload] of [
    ["protobuf", hex("08 01")],
    ["bond", hex("29 03 33 2e 30 00")],
  ]) {
    for (const headers of [
      [header("identity"), header("identity")],
      [header("identity\xa0")],
      [header("\x0bidentity")],
      [header("identity, identity")],
    ]) {
      const result = await host(kind, payload, { headers, decodeFailure: "metadataUnavailable" });
      assert.equal(result.decodes, 1);
      assert.equal(result.reads, 0);
      assert.equal(result.document.status, "unavailable");
    }
    const identity = await host(kind, payload, {
      headers: [header("\t IdEnTiTy \t")],
      availability: "truncated",
    });
    assert.equal(identity.decodes, 0);
    assert.equal(identity.document.status, "partial");
  }
});

test("negative zero survives the JSON boundary for Protobuf and Bond floats", async () => {
  const protobuf = (await host("protobuf", hex("0d 00000080 11 0000000000000080"))).document;
  assert.equal(
    nodes(protobuf).filter(
      (node) =>
        ["Possible float", "Possible double"].includes(node.name) &&
        node.value?.kind === "string" &&
        node.value.value === "-0",
    ).length,
    2,
  );
  const bond = (
    await host("bond", binary(bondHeader(1), Array.from(hex("07 00000080 28 0000000000000080 00"))))
  ).document;
  assert.equal(bond.status, "complete");
  assert.equal(
    nodes(bond).filter((node) => node.value?.kind === "string" && node.value.value === "-0").length,
    2,
  );
});

test("large collections, text, repeated fields and nesting hit explicit budgets", async () => {
  const repeated = await host(
    "protobuf",
    binary(Array.from({ length: 5_000 }, () => [8, 2]).flat()),
  );
  assert.equal(repeated.document.status, "limited");
  let groups = [8, 1];
  for (let index = 0; index < 12; index++) groups = [11, ...groups, 12];
  assert.equal((await host("protobuf", binary(groups))).document.status, "limited");
  const collection = binary(bondHeader(1), [11, 3, ...varint(4_001), ...Array(4_001).fill(0), 0]);
  assert.equal((await host("bond", collection)).document.status, "limited");
  const long = binary(bondHeader(1), [9, ...varint(2_000), ...Array(2_000).fill(120), 0]);
  const preview = nodes((await host("bond", long)).document).find((node) =>
    node.name.includes("preview"),
  );
  assert.equal(preview.value.value.length, 1_024);
});

test("deterministic malformed-input corpus remains bounded and source-safe", async () => {
  let seed = 0x5a17;
  for (let index = 0; index < 160; index++) {
    const payload = Uint8Array.from({ length: index % 43 }, () => {
      seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
      return seed >>> 24;
    });
    for (const kind of ["protobuf", "bond"]) await host(kind, payload);
  }
});
