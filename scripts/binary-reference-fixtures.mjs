/** Independent encoders used only by parser qualification and the local HTTP service. */
import { createServer } from "node:http";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";
import { encode as msgpack, ExtData } from "@msgpack/msgpack";
import { encode as cbor } from "cborg";
import { BSON, Decimal128, Long, ObjectId } from "bson";
import protobuf from "protobufjs";

const sample = { greeting: "hello", values: [1, -2, 3.5], text: "<script>inert</script>" };
const packed = protobuf
  .parse(
    'syntax="proto3"; message Sample { repeated sint32 numbers = 1; repeated float values = 2; }',
  )
  .root.lookupType("Sample");
export const referenceCases = [
  ["msgpack", "application/msgpack", msgpack(sample)],
  [
    "msgpack-right",
    "application/msgpack",
    msgpack({ ...sample, greeting: "changed", values: [1, -2, 4.5] }),
  ],
  [
    "msgpack-extension",
    "application/vnd.msgpack",
    msgpack(new ExtData(42, new Uint8Array([1, 2, 3]))),
  ],
  ["cbor", "application/cbor", cbor(sample)],
  ["cbor-right", "application/cbor", cbor({ ...sample, greeting: "changed" })],
  [
    "bson",
    "application/bson",
    BSON.serialize({
      ...sample,
      wide: Long.MAX_VALUE,
      decimal: Decimal128.fromString("123.456"),
      id: new ObjectId("507f1f77bcf86cd799439011"),
    }),
  ],
  [
    "bson-right",
    "application/bson",
    BSON.serialize({ ...sample, greeting: "changed", wide: Long.MIN_VALUE }),
  ],
  [
    "protobuf-packed",
    "application/protobuf",
    packed.encode(packed.create({ numbers: [-1, 2, -3], values: [1.5, -0, Infinity] })).finish(),
  ],
].map(([name, contentType, bytes]) => ({
  name,
  contentType,
  bodyHex: Buffer.from(bytes).toString("hex"),
}));

export async function startReferenceService() {
  const server = createServer((request, response) => {
    const sample = referenceCases.find((item) => request.url === "/" + item.name);
    if (!sample) {
      response.writeHead(404).end();
      return;
    }
    const body = gzipSync(Buffer.from(sample.bodyHex, "hex"), { mtime: 0 });
    response.writeHead(200, {
      "Content-Type": sample.contentType,
      "Content-Encoding": "gzip",
      "Content-Length": body.length,
    });
    response.end(body);
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  return {
    url: `http://127.0.0.1:${server.address().port}`,
    close: () =>
      new Promise((resolve, reject) => {
        server.closeAllConnections();
        server.close((error) => (error ? reject(error) : resolve()));
      }),
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url))
  process.stdout.write(JSON.stringify(referenceCases));
