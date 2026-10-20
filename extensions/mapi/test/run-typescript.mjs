// Developer-only fixture runner. The real extension runs in the native sandbox.
import vm from "node:vm";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { createInterface } from "node:readline";

const context = vm.createContext({}, { codeGeneration: { strings: false, wasm: false } });
const modules = new Map();
const root = new URL("../dist/", import.meta.url);

async function loadModule(url) {
  if (!url.href.startsWith(root.href) || !url.pathname.endsWith(".js")) {
    throw new Error("Fixture module escaped the extension");
  }
  if (!modules.has(url.href)) {
    modules.set(
      url.href,
      new vm.SourceTextModule(await readFile(fileURLToPath(url), "utf8"), {
        context,
        identifier: url.href,
      }),
    );
  }
  return modules.get(url.href);
}

const entry = await loadModule(new URL("decode.js", root));
await entry.link((specifier, owner) => loadModule(new URL(specifier, owner.identifier)));
await entry.evaluate({ timeout: 2_000 });
context.decode = entry.namespace.decode;

for await (const line of createInterface({ input: process.stdin, crlfDelay: Infinity })) {
  try {
    const fixture = JSON.parse(line);
    const messages = [...(fixture.context ?? []), fixture.message].map((message, index) => {
      const id = message.order || index + 1;
      return {
        id,
        requestType: message.requestType,
        ...Object.fromEntries(
          ["request", "response"].map((side) => [
            side,
            {
              bytes: Array.from(Buffer.from(message[side] ?? "", "base64")),
              sourceId: `body.${id}.${side}`,
            },
          ]),
        ),
      };
    });
    context.messages = messages;
    context.selectedId = messages.at(-1).id;
    context.direction = fixture.message.direction;
    context.options = Object.fromEntries(
      ["type", "arguments", "offset", "count", "prepareRequest"]
        .filter((name) => fixture[name] !== undefined)
        .map((name) => [name, fixture[name]]),
    );
    const result = vm.runInContext("decode(messages, selectedId, direction, options)", context, {
      timeout: 2_000,
    });
    console.log(JSON.stringify(result));
  } catch (error) {
    console.log(JSON.stringify({ failure: error.name, message: error.message }));
  } finally {
    delete context.messages;
    delete context.options;
  }
}
