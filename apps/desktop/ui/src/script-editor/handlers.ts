import type { ScriptLanguage } from "../models.js";

export const handlers = [
  {
    name: "onRequestHead",
    phase: "RequestHead",
    contract: "RequestHeadHandler",
    group: "Request",
    label: "Modify request headers",
    flag: "shouldModifyRequestHeaders",
    sample: 'return API.edit({ headers: [API.setHeader("User-Agent", "Transmog/1.0")] });',
    hint: "To try the header edit, allow user-agent in Writable headers.",
    setting: "scriptHeaders",
  },
  {
    name: "onRequestBody",
    phase: "RequestBody",
    contract: "RequestBodyHandler",
    group: "Request",
    label: "Modify request body",
    flag: "shouldModifyRequestBody",
    sample:
      'const text = API.readText(event.body, event.request.headers);\nreturn API.replaceText(text.replace("before", "after"));',
    hint: "Requires Read bounded bodies even with the switch off. Matching requests buffer and headers wait. Use text/plain. To try the edit, enable Replace bodies and allow content-type in Writable headers.",
    setting: "readBodies",
  },
  {
    name: "onResponseHead",
    phase: "ResponseHead",
    contract: "ResponseHeadHandler",
    group: "Response",
    label: "Modify response headers",
    flag: "shouldModifyResponseHeaders",
    sample: 'return API.edit({ headers: [API.setHeader("x-sample", "preview")] });',
    hint: "To try the header edit, allow x-sample in Writable headers.",
    setting: "scriptHeaders",
  },
  {
    name: "onResponseBody",
    phase: "ResponseBody",
    contract: "ResponseBodyHandler",
    group: "Response",
    label: "Modify response body",
    flag: "shouldModifyResponseBody",
    sample:
      "const original = API.readJson(event.body, event.response.headers);\nreturn API.replaceJson({ original, inspected: true });",
    hint: "Requires Read bounded bodies even with the switch off. Matching responses buffer and headers wait. Use application/json. To try the edit, enable Replace bodies and allow content-type in Writable headers.",
    setting: "readBodies",
  },
  {
    name: "onResponseChunk",
    phase: "ResponseChunk",
    contract: "ResponseChunkHandler",
    group: "Response",
    label: "Process response chunks",
    flag: "shouldReplaceStreamText",
    sample: 'return API.replaceTextChunk(event, "before", "after");',
    hint: "Requires Read bounded bodies even with the switch off. Use text/plain. To try the edit, enable Replace bodies. Content-Type is preserved. If onResponseBody also exists, it is the default unless onResponseHead selects streaming.",
    setting: "readBodies",
  },
  {
    name: "onCompleted",
    phase: "Completed",
    contract: "CompletedHandler",
    group: "Outcome",
    label: "Inspect completed exchanges",
    flag: "shouldLogCompletion",
    sample: "console.info(event.response.status);",
    hint: "Change the switch to true to log the final response status. Console output is temporary.",
    setting: "",
  },
  {
    name: "onFailed",
    phase: "Failed",
    contract: "FailedHandler",
    group: "Outcome",
    label: "Inspect failed exchanges",
    flag: "shouldLogFailure",
    sample: "console.warn(event.failure);",
    hint: "Change the switch to true to log the failure category. Console output is temporary.",
    setting: "",
  },
] as const;

export type HandlerName = (typeof handlers)[number]["name"];
export interface HandlerLocation {
  name: HandlerName;
  action: "Add" | "Go to" | "Review";
  start: number;
  end: number;
  reason: string;
}
export interface ScriptAnalysis {
  handlers: HandlerName[];
  locations: HandlerLocation[];
  bindings: string[];
  imports: Array<{ imported: string; local: string; typeOnly: boolean }>;
}

/** One appended edit keeps imports and the handler in the same undo transaction. */
export function handlerInsertion(
  source: string,
  language: ScriptLanguage,
  name: HandlerName,
  analysis: ScriptAnalysis,
  eol: string,
) {
  const handler = handlers.find((item) => item.name === name)!;
  const occupied = new Set(analysis.bindings);
  const locals = new Set(["event", handler.flag, "text", "original"]);
  for (const local of locals) {
    occupied.add(local);
  }
  const imports = analysis.imports.filter((item) => !locals.has(item.local));
  const unique = (base: string) => {
    let local = base;
    for (let suffix = 2; occupied.has(local); suffix++) {
      local = base + suffix;
    }
    occupied.add(local);
    return local;
  };
  let namespace = imports.find((item) => item.imported === "*" && !item.typeOnly);
  const missing: string[] = [];
  const reference = (symbol: string, typeOnly = false) => {
    if (namespace) {
      return namespace.local + "." + symbol;
    }
    const existing = imports.find(
      (item) => item.imported === symbol && (typeOnly || !item.typeOnly),
    );
    if (existing) {
      return existing.local;
    }
    const local = unique(symbol);
    missing.push(symbol === local ? symbol : symbol + " as " + local);
    return local;
  };
  // A namespace keeps a new script readable and supports API completion.
  let prefix = "";
  if (!analysis.imports.some((item) => !item.typeOnly)) {
    const local = unique("api");
    namespace = { imported: "*", local, typeOnly: false };
    prefix = "import * as " + local + ' from "transmog:api";' + eol + eol;
  }
  const apiNamespace = namespace;
  const ref = (symbol: string) =>
    apiNamespace ? apiNamespace.local + "." + symbol : reference(symbol);
  const sample = handler.sample.replace(/API\.(\w+)/g, (_, symbol: string) => ref(symbol));
  const pass = ref("pass");
  const result = handler.group === "Outcome" ? "TerminalResult" : handler.phase + "Result";
  const parameter =
    language === "typescript"
      ? "event: " +
        (apiNamespace
          ? apiNamespace.local + "." + handler.phase + "Event"
          : reference(handler.phase + "Event", true))
      : "event";
  const returns =
    language === "typescript"
      ? ": " + (apiNamespace ? apiNamespace.local + "." + result : reference(result, true))
      : "";
  if (missing.length) {
    prefix += "import { " + missing.join(", ") + ' } from "transmog:api";' + eol + eol;
  }
  const code = [
    "export function " + name + "(" + parameter + ")" + returns + " {",
    "  const " + handler.flag + " = false;",
    "  if (" + handler.flag + ") {",
    ...sample.split("\n").map((line) => "    " + line),
    "  }",
    "  return " + pass + "();",
    "}",
    "",
  ].join(eol);
  const separator = source.length
    ? source.endsWith(eol + eol)
      ? ""
      : source.endsWith(eol)
        ? eol
        : eol + eol
    : "";
  const text = separator + prefix + code;
  return { text, selectionOffset: source.length + text.indexOf(" = false;") + 3 };
}
