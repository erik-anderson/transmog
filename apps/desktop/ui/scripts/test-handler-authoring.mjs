import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import ts from "../../../../node_modules/typescript/lib/typescript.js";
import * as api from "../../../../crates/script-host/src/api.mjs";

const compiled = await build({
  stdin: {
    contents:
      'export * from "./src/script-editor/analyze.ts"; export * from "./src/script-editor/handlers.ts";',
    resolveDir: fileURLToPath(new URL("../", import.meta.url)),
    loader: "ts",
  },
  bundle: true,
  platform: "node",
  format: "esm",
  write: false,
});
const { analyzeScript, handlerInsertion, handlers } = await import(
  "data:text/javascript;base64," + Buffer.from(compiled.outputFiles[0].text).toString("base64")
);
const declarationsPath = fileURLToPath(
  new URL("../../../../crates/proxy-script/generated/transmog-script.d.ts", import.meta.url),
);
const help = JSON.parse(
  await readFile(new URL("../../../../crates/proxy-script/help.json", import.meta.url), "utf8"),
);

test("the menu covers exactly the runtime's documented phases", () => {
  assert.deepEqual(
    handlers.map((item) => item.name),
    help.sections.flatMap((section) => section.phases.map((phase) => phase.handler)),
  );
});

test("source inspection ignores text and nested declarations and identifies repair locations", () => {
  for (const source of [
    "// export function onRequestHead() {}",
    'const text="export function onRequestHead() {}";',
    "function outer() { function onRequestHead() {} }",
  ]) {
    assert.equal(analyzeScript(ts, source, "typescript").locations[0].action, "Add");
  }
  for (const source of [
    "function onRequestHead() {}",
    "export const onRequestHead = () => {};",
    "export function* onRequestHead() {}",
    "export async function onRequestHead() {}",
    "export function onRequestHead() {",
    "export function onRequestHead() {}\nexport function onRequestHead() {}",
    "const local=()=>{}; export {local as onRequestHead};",
    'export * as onRequestHead from "transmog:api";',
  ]) {
    const location = analyzeScript(ts, source, "typescript").locations[0];
    assert.equal(location.action, "Review", source);
    assert.ok(location.reason);
  }
  const source = "// 🌍\r\nexport function onRequestHead() { return {}; }";
  const location = analyzeScript(ts, source, "typescript").locations[0];
  assert.equal(source.slice(location.start, location.end), "onRequestHead");
  assert.equal(
    analyzeScript(
      ts,
      "export function onRequestHead(): unknown; export function onRequestHead() { return {}; }",
      "typescript",
    ).locations[0].action,
    "Go to",
  );
  assert.equal(
    analyzeScript(ts, "if (false) { var onRequestHead; }", "javascript").locations[0].action,
    "Review",
  );
});

test("generated handlers avoid aliases shadowed by their local variables", () => {
  const body = { bytes: api.utf8Encode('{"before":true}') };
  for (const language of ["typescript", "javascript"]) {
    for (const [source, name] of [
      ['import {pass as event} from "transmog:api";', "onRequestHead"],
      ['import * as event from "transmog:api";', "onRequestHead"],
      ['import {edit as shouldModifyRequestHeaders} from "transmog:api";', "onRequestHead"],
      ['import * as text from "transmog:api";', "onRequestBody"],
      ['import {readJson as original} from "transmog:api";', "onResponseBody"],
      ['import * as original from "transmog:api";', "onResponseBody"],
    ]) {
      const inserted =
        source +
        handlerInsertion(source, language, name, analyzeScript(ts, source, language), "\n").text;
      for (const enabled of [false, true]) {
        const text = enabled ? inserted.replace(" = false;", " = true;") : inserted;
        const javascript = ts.transpileModule(text, {
          compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
        }).outputText;
        const exported = {};
        new Function("exports", "require", javascript)(exported, () => api);
        const result = exported[name]({
          request: { headers: [] },
          response: { headers: [] },
          body,
        });
        assert.equal(result.decision.action, enabled ? "edit" : "continue", text);
      }
    }
  }
});

for (const language of ["typescript", "javascript"]) {
  test(
    language + " samples compile and only run example work when their switches are enabled",
    () => {
      let source = '"use strict";\r\n// keep this comment';
      for (const handler of handlers)
        source += handlerInsertion(
          source,
          language,
          handler.name,
          analyzeScript(ts, source, language),
          "\r\n",
        ).text;
      assert.equal(source.startsWith('"use strict";\r\n// keep this comment'), true);
      assert.equal((source.match(/import \* as api /g) ?? []).length, 1);
      assert.equal((source.match(/ = false;/g) ?? []).length, 7);
      assert.equal(analyzeScript(ts, source, language).handlers.length, 7);
      const options = {
        strict: true,
        noEmit: true,
        allowJs: true,
        checkJs: false,
        lib: ["lib.esnext.d.ts"],
        types: [],
        target: ts.ScriptTarget.ESNext,
        module: ts.ModuleKind.ESNext,
        skipLibCheck: true,
      };
      const host = ts.createCompilerHost(options),
        get = host.getSourceFile.bind(host),
        name = "generated." + (language === "typescript" ? "ts" : "js");
      host.getSourceFile = (file, ...args) =>
        file === name
          ? ts.createSourceFile(
              file,
              source,
              ts.ScriptTarget.Latest,
              true,
              language === "typescript" ? ts.ScriptKind.TS : ts.ScriptKind.JS,
            )
          : get(file, ...args);
      const program = ts.createProgram([name, declarationsPath], options, host);
      assert.deepEqual(
        ts
          .getPreEmitDiagnostics(program)
          .map((item) => ts.flattenDiagnosticMessageText(item.messageText, " ")),
        [],
      );
      const logs = [];
      function execute(text) {
        const javascript = ts.transpileModule(text, {
          compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
        }).outputText;
        const exports = {};
        new Function("exports", "require", "console", javascript)(exports, () => api, {
          info: (value) => logs.push(value),
          warn: (value) => logs.push(value),
        });
        return exports;
      }
      const off = execute(source);
      for (const handler of handlers)
        assert.deepEqual(
          off[handler.name](
            new Proxy(
              {},
              {
                get() {
                  throw new Error("Sample read input while its switch was off");
                },
              },
            ),
          ),
          api.pass(),
        );
      assert.deepEqual(logs, []);
      const on = execute(source.replaceAll(" = false;", " = true;"));
      assert.deepEqual(on.onRequestHead({}).decision.edits.headers, [
        api.setHeader("User-Agent", "Transmog/1.0"),
      ]);
      assert.deepEqual(on.onResponseHead({}).decision.edits.headers, [
        api.setHeader("x-sample", "preview"),
      ]);
      assert.equal(
        api.utf8Decode(
          on.onRequestBody({ request: { headers: [] }, body: { bytes: api.utf8Encode("before") } })
            .decision.edits.bytes,
        ),
        "after",
      );
      assert.deepEqual(
        JSON.parse(
          api.utf8Decode(
            on.onResponseBody({
              response: { headers: [] },
              body: { bytes: api.utf8Encode('{"a":1}') },
            }).decision.edits.bytes,
          ),
        ),
        { original: { a: 1 }, inspected: true },
      );
      assert.equal(
        on.onResponseChunk({ bytes: api.utf8Encode("be"), end: false }).decision.action,
        "need-more",
      );
      assert.equal(
        api.utf8Decode(
          on.onResponseChunk({ bytes: api.utf8Encode("before"), end: true }).decision.bytes,
        ),
        "after",
      );
      assert.deepEqual(on.onCompleted({ response: { status: 200 } }), api.pass());
      assert.deepEqual(on.onFailed({ failure: "Upstream" }), api.pass());
      assert.deepEqual(logs, [200, "Upstream"]);
    },
  );
}

test("insertions reuse aliases and distinguish type-only imports from runtime helpers", () => {
  for (const source of [
    'import {pass as next} from "transmog:api";\nconst edit = 1;',
    'import type * as api from "transmog:api";',
    'import * as traffic from "transmog:api";',
  ]) {
    const insertion = handlerInsertion(
      source,
      "typescript",
      "onRequestHead",
      analyzeScript(ts, source, "typescript"),
      "\n",
    );
    assert.equal(
      (source + insertion.text).slice(insertion.selectionOffset, insertion.selectionOffset + 5),
      "false",
    );
    assert.equal(
      analyzeScript(ts, source + insertion.text, "typescript").handlers[0],
      "onRequestHead",
    );
    if (source.includes("next")) assert.match(insertion.text, /return next\(\)/);
    if (source.includes("type \*")) assert.match(insertion.text, /import \* as api2/);
    if (source.includes("traffic")) assert.match(insertion.text, /traffic.setHeader/);
  }
});
