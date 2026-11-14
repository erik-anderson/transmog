import * as Wire from "./bond-wire.js";
import type { VisualizationDocument } from "transmog:extensions/v1";
import { document } from "./reader.js";
import { readInlineCandidate } from "./bond-inline.js";
import type { CandidateReader } from "./bond-inline.js";

/** Resolve framing and version without discarding viable alternative interpretations. */
export function parse(
  bytes: Uint8Array,
  sourceId: string,
  read: CandidateReader = readInlineCandidate,
): VisualizationDocument {
  const header = bytes[0] === Wire.PROTOCOL_MAGIC[0] && bytes[1] === Wire.PROTOCOL_MAGIC[1];
  // A leading protocol header frames the whole body. Probe the existing raw
  // single-struct alternative; do not count the header itself as a batch item.
  const raw = rawDocument(bytes, sourceId, !header, read);
  if (header) {
    // The protocol magic can also be a raw field header and value. Do not hide
    // a valid alternative or silently treat a damaged envelope as established raw data.
    const rawValid =
      raw.status === "complete" ||
      raw.diagnostics!.some(
        (item) => item.code === "ambiguous-version" || item.code === "version-limited",
      );
    if (bytes.length < Wire.PROTOCOL_HEADER_BYTES) {
      if (rawValid) {
        return framingAmbiguity(raw);
      }
      return {
        ...document("Bond Compact Binary", Wire.SUMMARY),
        status: "malformed",
        diagnostics: [{ code: "malformed", message: "Truncated Bond protocol/version header." }],
      };
    }
    const version = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint16(
      Wire.PROTOCOL_VERSION_OFFSET,
      true,
    );
    if (version !== Wire.Version.V1 && version !== Wire.Version.V2) {
      if (rawValid) {
        return framingAmbiguity(raw);
      }
      return {
        ...document("Bond Compact Binary", Wire.SUMMARY),
        status: "partial",
        diagnostics: [
          {
            code: "unsupported-version",
            message: `Compact Binary version ${version} is unsupported. Inspect the original bytes in Hex.`,
          },
        ],
      };
    }
    const doc = read(bytes, sourceId, version, Wire.PROTOCOL_HEADER_BYTES);
    doc.summary += ` The marshaled header declares version ${version}.`;
    doc.roots.unshift({
      id: "bond.header",
      name: `Protocol header · Compact Binary v${version}`,
      spans: [{ sourceId, offset: "0", length: String(Wire.PROTOCOL_HEADER_BYTES) }],
      value: { kind: "bytes" },
      children: [],
    });
    if (rawValid) {
      if (doc.status === "complete") {
        raw.roots.unshift(...doc.roots);
      }
      return framingAmbiguity(raw);
    }
    if (raw.status === "limited" && doc.status === "complete") {
      return limitedInterpretation(
        doc,
        "framing-limited",
        "A raw Compact Binary interpretation reached a parser limit and was not ruled out. Framing remains unresolved.",
      );
    }
    return doc;
  }
  return raw;
}

function framingAmbiguity(raw: VisualizationDocument): VisualizationDocument {
  if (raw.status !== "limited") {
    raw.status = "partial";
  }
  raw.summary =
    Wire.SUMMARY +
    " Framing is ambiguous: the protocol-header bytes can also be raw Compact Binary fields.";
  raw.diagnostics!.push({
    code: "ambiguous-framing",
    message:
      "The bytes resemble a marshaled header and also parse as raw fields. The available interpretations are shown; neither framing is established.",
  });
  for (const root of raw.roots) {
    if (!root.name.startsWith("Possible")) {
      root.name = "Possible " + root.name;
    }
  }
  return raw;
}

function limitedInterpretation(
  doc: VisualizationDocument,
  code: string,
  message: string,
): VisualizationDocument {
  doc.status = "limited";
  doc.summary += " " + message;
  doc.diagnostics!.push({ code, message });
  for (const root of doc.roots) {
    if (!root.name.startsWith("Possible")) {
      root.name = "Possible " + root.name;
    }
  }
  return doc;
}

function rawDocument(
  bytes: Uint8Array,
  sourceId: string,
  sequence: boolean,
  read: CandidateReader,
): VisualizationDocument {
  const first = read(bytes, sourceId, Wire.Version.V1, 0, sequence);
  const second = read(bytes, sourceId, Wire.Version.V2, 0, sequence);
  if (sequence && first.status === "malformed" && second.status === "malformed") {
    return {
      ...document(
        "Bond Compact Binary",
        Wire.SUMMARY +
          " Neither raw version parsed completely. Both incomplete interpretations are shown.",
      ),
      status: "malformed",
      roots: [first, second].flatMap((doc, index) =>
        doc.roots.map((root) => ({
          ...root,
          name: `Possible Compact Binary v${index + 1} (incomplete) · ${root.name}`,
        })),
      ),
      diagnostics: [first, second].flatMap((doc, index) =>
        doc.diagnostics!.map((item) => ({ ...item, message: `v${index + 1}: ${item.message}` })),
      ),
    };
  }
  if (first.status === "complete" && second.status === "complete") {
    return {
      ...document(
        "Bond Compact Binary",
        Wire.SUMMARY +
          " Version is ambiguous: both raw v1 and v2 interpretations parse completely.",
      ),
      status: "partial",
      roots: [
        ...first.roots.map((root) => ({ ...root, name: "Possible " + root.name })),
        ...second.roots.map((root) => ({ ...root, name: "Possible " + root.name })),
      ],
      diagnostics: [
        {
          code: "ambiguous-version",
          message:
            "No protocol/version header was captured. Both interpretations are shown; neither version is established.",
        },
      ],
    };
  }
  // Prefer a complete parse. Otherwise retain a limit (it cannot disprove a
  // version), then prefer the malformed parse with more completed fields.
  let doc = first;
  if (second.status === "complete") {
    doc = second;
  } else if (first.status !== "complete" && first.status !== "limited") {
    if (
      second.status === "limited" ||
      second.roots[0]!.children!.length > first.roots[0]!.children!.length
    ) {
      doc = second;
    }
  }
  const version = doc === second ? Wire.Version.V2 : Wire.Version.V1;
  const alternative = doc === second ? first : second;
  if (doc.status === "complete" && alternative.status === "limited") {
    doc.roots[0]!.name = `Possible Compact Binary v${version} (version unresolved)`;
    return limitedInterpretation(
      doc,
      "version-limited",
      `The v${version === Wire.Version.V1 ? Wire.Version.V2 : Wire.Version.V1} interpretation reached a parser limit and was not ruled out. Version remains unresolved.`,
    );
  }
  for (const [index, root] of doc.roots.entries()) {
    if (root.value?.kind === "object") {
      root.name = `Possible Compact Binary v${version} (${doc.status === "complete" ? "inferred" : "incomplete interpretation"})${doc.roots.length > 1 ? ` · Struct ${index + 1}` : ""}`;
    } else if (root.value?.kind === "array") {
      root.name = `Possible v${version} (${doc.status === "complete" ? "inferred" : "incomplete interpretation"}) · ${root.name.replace(/^Compact Binary v[12] · /, "")}`;
    }
  }
  doc.summary +=
    doc.status === "complete"
      ? ` No protocol/version header was captured; v${version} is inferred from structure.`
      : ` Neither raw version parsed completely; showing a possible v${version} interpretation.`;
  doc.diagnostics!.unshift({
    code: "inferred-version",
    message:
      "The Compact Binary version is inferred, not declared. Schema aliases such as enums, blobs and nullable values cannot be recovered.",
  });
  return doc;
}
