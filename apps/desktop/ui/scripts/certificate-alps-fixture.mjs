// Shared production-template and native viewer fixture, including public raw bytes.
export function certificateAlpsFixture() {
  const origin = Buffer.from("https://localhost"),
    hints = Buffer.from("Sec-CH-UA-Platform, Sec-CH-UA-Model");
  const entry = Buffer.alloc(4 + origin.length + hints.length);
  entry.writeUInt16BE(origin.length);
  origin.copy(entry, 2);
  entry.writeUInt16BE(hints.length, 2 + origin.length);
  hints.copy(entry, 4 + origin.length);
  const header = Buffer.from([0, 0, entry.length, 0x89, 0, 0, 0, 0, 0]);
  const bytes = Buffer.concat([
    Buffer.from([0, 0, 6, 4, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 8]),
    header,
    entry,
  ]);
  return {
    bytes: Array.from(bytes),
    view: {
      peerBytes: bytes.length,
      fields: [
        { label: "Header table size (0x0001)", value: "8 bytes" },
        { label: "Client Hint origin", value: origin.toString() },
        { label: "Requested Client Hints (ignored)", value: hints.toString() },
      ],
      notes: [],
      payloadBase64: bytes.toString("base64"),
    },
  };
}
