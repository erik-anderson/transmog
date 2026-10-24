import type {
  BodyReference,
  BytePage,
  HttpDecodeResult,
  Input,
  Step,
  VisualizationDocument,
} from "transmog:extensions/v1";
import { notice } from "./reader.js";

const maximumBytes = 2 * 1024 * 1024;
type Pending = { kind: "bytes" | "decoded"; token: string; count: number };

/** Only the selected side is read. No context, filesystem, network or live feed. */
export function visualizer(
  title: string,
  parse: (bytes: Uint8Array, sourceId: string, input?: Input) => VisualizationDocument,
) {
  let body: BodyReference | undefined;
  let bytes = new Uint8Array(0);
  let offset = 0;
  let sequence = 0;
  let pending: Pending | undefined;
  let metadata: Input | undefined;

  function show(status: "unavailable" | "limited" | "partial", message: string): Step {
    pending = undefined;
    return { kind: "document", document: notice(title, status, message) };
  }

  function take(token: string, kind: Pending["kind"]): Pending {
    if (!pending || pending.token !== token || pending.kind !== kind) {
      throw new Error("Expired binary visualizer continuation");
    }
    const result = pending;
    pending = undefined;
    return result;
  }

  function pump(): Step {
    if (!body) {
      return show(
        "unavailable",
        "The selected body is unavailable. Inspect the retained headers and Hex view.",
      );
    }
    if (offset < bytes.length) {
      const count = Math.min(65_536, bytes.length - offset);
      const token = `binary.${++sequence}`;
      pending = { kind: "bytes", token, count };
      return {
        kind: "needInput",
        token,
        reads: [{ sourceId: body.sourceId, offset: String(offset), maxBytes: count }],
      };
    }
    const document = parse(bytes, body.sourceId, metadata);
    if (body.availability !== "complete") {
      if (document.status !== "limited") {
        document.status = "partial";
      }
      document.diagnostics!.push({
        code: "incomplete-evidence",
        message:
          "The body is incomplete. Only retained bytes are shown; absent fields and message boundaries cannot be inferred.",
      });
    }
    bytes = new Uint8Array(0);
    return { kind: "document", document };
  }

  function begin(reference: BodyReference): Step {
    body = reference;
    if (!["complete", "capturing", "truncated"].includes(body.availability)) {
      return show(
        "unavailable",
        "Body bytes are unavailable. Inspect the retained headers and Hex view.",
      );
    }
    const length = BigInt(body.length);
    if (length > BigInt(maximumBytes)) {
      return show(
        "limited",
        "The body exceeds the visualizer's 2 MiB input limit. Inspect the retained bytes in Hex.",
      );
    }
    bytes = new Uint8Array(Number(length));
    offset = 0;
    return pump();
  }

  return {
    visualize(input: Input): Step {
      metadata = input;
      body = undefined;
      bytes = new Uint8Array(0);
      offset = 0;
      pending = undefined;
      const selected = input[input.direction];
      if (!selected?.body) {
        return pump();
      }
      body = selected.body;
      const codings = selected.headers.filter(
        (field) => field.name.toLowerCase() === "content-encoding",
      );
      const identity = codings.length === 1 ? codings[0] : undefined;
      if (
        codings.length === 0 ||
        (identity &&
          !identity.redacted &&
          identity.value &&
          identity.value.length <= 256 &&
          /^[\t ]*identity[\t ]*$/i.test(String.fromCharCode(...identity.value)))
      ) {
        return begin(body);
      }
      const token = `binary.${++sequence}`;
      pending = { kind: "decoded", token, count: 0 };
      return { kind: "needDecoded", sourceId: body.sourceId, token };
    },
    resumeDecoded(token: string, result: HttpDecodeResult): Step {
      take(token, "decoded");
      if (result.kind === "unavailable") {
        return show(
          result.reason === "limited"
            ? "limited"
            : result.reason === "incomplete"
              ? "partial"
              : "unavailable",
          "HTTP content decoding is unavailable (" +
            result.reason +
            "). Encoded bytes were not interpreted. Inspect the original body in Hex.",
        );
      }
      return begin(result.decoded.body);
    },
    resume(token: string, pages: readonly BytePage[]): Step {
      const request = take(token, "bytes");
      const page = pages[0];
      if (
        pages.length !== 1 ||
        !page ||
        page.sourceId !== body?.sourceId ||
        page.offset !== String(offset) ||
        page.bytes.length !== request.count
      ) {
        return show(
          "unavailable",
          "The requested body range is no longer available. Reopen the visualizer to retry.",
        );
      }
      bytes.set(page.bytes, offset);
      offset += page.bytes.length;
      return pump();
    },
  };
}
