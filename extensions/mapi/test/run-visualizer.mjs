// Developer-only host-continuation harness; production isolation is AppContainer.
import vm from "node:vm";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { createInterface } from "node:readline";

const context = vm.createContext({}, { codeGeneration: { strings: false, wasm: false } }),
  modules = new Map(),
  root = new URL("../dist/runtime/", import.meta.url);
vm.runInContext("globalThis.Function=undefined;globalThis.eval=undefined;", context);
async function module(url) {
  if (!url.href.startsWith(root.href) || !url.pathname.endsWith(".js"))
    throw new Error("Fixture module escaped the extension");
  if (!modules.has(url.href))
    modules.set(
      url.href,
      readFile(fileURLToPath(url), "utf8").then(
        (source) => new vm.SourceTextModule(source, { context, identifier: url.href }),
      ),
    );
  return modules.get(url.href);
}
const entry = await module(new URL((process.argv[2] ?? "body") + ".js", root));
await entry.link((specifier, owner) => module(new URL(specifier, owner.identifier)));
await entry.evaluate({ timeout: 15000 });
context.handlers = entry.namespace;
function freeze(value) {
  if (value && typeof value === "object" && !Object.isFrozen(value)) {
    Object.freeze(value);
    for (const child of Object.values(value)) freeze(child);
  }
  return value;
}
function call(name, ...args) {
  context.method = name;
  context.args = freeze(args);
  return vm.runInContext("handlers[method](...args)", context, { timeout: 2000 });
}
for await (const line of createInterface({ input: process.stdin, crlfDelay: Infinity })) {
  try {
    const fixture = JSON.parse(line),
      sources = new Map();
    let reads = 0,
      contextReads = 0,
      decodedReads = 0,
      expansions = 0;
    function input(message, index, direction) {
      const order = BigInt(message.order ?? index + 1) + 9007199254740993n;
      const result = {
        instanceId: "test",
        exchangeId: "exchange." + order,
        requestNumber: String(order),
        generation: "1",
        direction,
      };
      for (const side of ["request", "response"]) {
        const fields =
          side === "request"
            ? {
                "content-type": "application/mapi-http",
                "x-requesttype": message.requestType,
                "x-clientinfo": "synthetic-client",
              }
            : { "content-type": "application/mapi-http", "x-responsecode": "0" };
        Object.assign(fields, message[side + "Headers"] ?? {});
        const headers = Object.entries(fields).map(([name, value], index) => ({
          index,
          name,
          redacted: value === null,
          value: value === null ? null : Array.from(Buffer.from(value, "utf8")),
        }));
        result[side] = {
          evidenceId: side + "." + order,
          headers,
          ...(side === "request" ? { method: "POST", path: "/mapi/emsmdb" } : { status: 200 }),
        };
        const bytes = Array.from(Buffer.from(message[side] ?? "", "base64"));
        if (bytes.length || message[side + "Availability"]) {
          const sourceId = side + "." + order + "." + (message.revision ?? 0);
          sources.set(sourceId, bytes);
          result[side].body = {
            sourceId,
            length: String(message[side + "Length"] ?? bytes.length),
            availability: message[side + "Availability"] ?? "complete",
          };
        }
      }
      return result;
    }
    const related = (fixture.context ?? [])
      .map((message, index) => {
        const value = input(message, index, fixture.message.direction);
        return { order: value.requestNumber, input: value };
      })
      .sort((a, b) => (BigInt(a.order) < BigInt(b.order) ? -1 : 1));
    const selected = input(fixture.message, related.length, fixture.message.direction);
    let step = call("visualize", selected),
      cursor = 0;
    function finish() {
      for (let guard = 0; guard < 1024; guard++) {
        if (step.kind === "needInput") {
          reads++;
          step = call(
            "resume",
            step.token,
            step.reads.map((read) => ({
              sourceId: read.sourceId,
              offset: read.offset,
              bytes: sources
                .get(read.sourceId)
                .slice(Number(read.offset), Number(read.offset) + read.maxBytes),
            })),
          );
        } else if (step.kind === "needContext") {
          contextReads++;
          const page = related.slice(cursor, cursor + step.request.limit);
          cursor += page.length;
          step = call("resumeContext", step.token, {
            anchorOrder: selected.requestNumber,
            exchanges: page,
            status: cursor < related.length ? "more" : "complete",
            nextCursor: cursor < related.length ? "cursor." + cursor : null,
          });
        } else if (step.kind === "needDecoded") {
          decodedReads++;
          if (fixture.decodedFailure)
            step = call("resumeDecoded", step.token, {
              kind: "unavailable",
              reason: fixture.decodedFailure,
            });
          else {
            const bytes = Array.from(Buffer.from(fixture.decodedBytes ?? "", "base64")),
              id = "decoded-http";
            sources.set(id, bytes);
            step = call("resumeDecoded", step.token, {
              kind: "ready",
              decoded: {
                body: { sourceId: id, length: String(bytes.length), availability: "complete" },
                parent: {
                  sourceId: step.sourceId,
                  offset: "0",
                  length: String(sources.get(step.sourceId).length),
                },
                codings: ["gzip"],
              },
            });
          }
        } else if (step.kind === "document") {
          function token(nodes) {
            for (const node of nodes) {
              if (node.childrenToken) return node.childrenToken;
              const result = token(node.children ?? []);
              if (result) return result;
            }
          }
          const next = fixture.expand === false ? null : token(step.document.roots);
          if (!next) return step.document;
          if (++expansions > 512) throw new Error("Unbounded expansion");
          step = call("expand", next);
        } else throw new Error("Invalid visualizer step");
      }
      throw new Error("Unbounded continuation");
    }
    let document = finish();
    for (const update of fixture.updates ?? []) {
      const value = input(update, related.length, fixture.message.direction);
      step = call(
        "onTraffic",
        [
          {
            sequence: "1",
            exchangeId: value.exchangeId,
            direction: fixture.message.direction,
            phase: "bodyCompleted",
            revision: String(update.revision ?? 1),
            input: value,
          },
        ],
        { missedHints: "0" },
      );
      document = finish();
    }
    if (fixture.missedHints) {
      step = call("onTraffic", [], { missedHints: "1" });
      document = finish();
    }
    console.log(
      JSON.stringify({
        document,
        reads,
        contextReads,
        decodedReads,
        expansions,
        parserLoaded: [...modules.keys()].some((url) => url.endsWith("/body.js")),
      }),
    );
  } catch (error) {
    console.log(JSON.stringify({ failure: error.name, message: error.message }));
  } finally {
    delete context.args;
  }
}
