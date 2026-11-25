import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { build, transformSync } from "esbuild";
// The worker re-exports this parser. Its underlying module also runs in Node.
import { typescript as parser } from "monaco-editor/languages/features/typescript/lib/typescriptServices.js";
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
const {
  analyzeScript: analyzeWithParser,
  handlerInsertion,
  handlers,
} = await import(
  "data:text/javascript;base64," + Buffer.from(compiled.outputFiles[0].text).toString("base64")
);
const compiler = fileURLToPath(new URL("bin/tsc", import.meta.resolve("typescript/package.json")));
const declarationsPath = fileURLToPath(
  new URL("../../../../crates/proxy-script/generated/transmog-script.d.ts", import.meta.url),
);
const help = JSON.parse(
  await readFile(new URL("../../../../crates/proxy-script/help.json", import.meta.url), "utf8"),
);

function analyzeScript(source, language) {
  return analyzeWithParser(parser, source, language);
}

function executeSample(source, loader, output = console) {
  const { code } = transformSync(source, { loader, format: "cjs", target: "es2022" });
  const module = { exports: {} };
  new Function("module", "require", "console", code)(
    module,
    (specifier) => {
      assert.equal(specifier, "transmog:api");
      return api;
    },
    output,
  );
  return module.exports;
}

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
    assert.equal(analyzeScript(source, "typescript").locations[0].action, "Add");
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
    const location = analyzeScript(source, "typescript").locations[0];
    assert.equal(location.action, "Review", source);
    assert.ok(location.reason);
  }
  const source = "// 🌍\r\nexport function onRequestHead() { return {}; }";
  const location = analyzeScript(source, "typescript").locations[0];
  assert.equal(source.slice(location.start, location.end), "onRequestHead");
  assert.equal(
    analyzeScript(
      "export function onRequestHead(): unknown; export function onRequestHead() { return {}; }",
      "typescript",
    ).locations[0].action,
    "Go to",
  );
  assert.equal(
    analyzeScript("if (false) { var onRequestHead; }", "javascript").locations[0].action,
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
        handlerInsertion(source, language, name, analyzeScript(source, language), "\n").text;
      for (const enabled of [false, true]) {
        const text = enabled ? inserted.replace(" = false;", " = true;") : inserted;
        const exported = executeSample(text, language === "typescript" ? "ts" : "js");
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
    async (t) => {
      let source = '"use strict";\r\n// keep this comment';
      for (const handler of handlers)
        source += handlerInsertion(
          source,
          language,
          handler.name,
          analyzeScript(source, language),
          "\r\n",
        ).text;
      assert.equal(source.startsWith('"use strict";\r\n// keep this comment'), true);
      assert.equal((source.match(/import \* as api /g) ?? []).length, 1);
      assert.equal((source.match(/ = false;/g) ?? []).length, 7);
      assert.equal(analyzeScript(source, language).handlers.length, 7);
      const directory = await mkdtemp(join(tmpdir(), "transmog-handler-"));
      t.after(() => rm(directory, { recursive: true, force: true }));
      const loader = language === "typescript" ? "ts" : "js";
      const name = `generated.${loader}`;
      const compilerOptions = {
        strict: true,
        noEmit: true,
        allowJs: true,
        checkJs: false,
        lib: ["ESNext"],
        types: [],
        target: "ESNext",
        module: "ESNext",
        moduleResolution: "Bundler",
        skipLibCheck: true,
      };
      await writeFile(join(directory, name), source);
      await writeFile(
        join(directory, "tsconfig.json"),
        JSON.stringify({ compilerOptions, files: [name, declarationsPath] }),
      );
      const result = spawnSync(
        process.execPath,
        [compiler, "--project", directory, "--pretty", "false"],
        {
          encoding: "utf8",
          timeout: 30_000,
          windowsHide: true,
        },
      );
      assert.ifError(result.error);
      assert.equal(result.status, 0, result.stdout + result.stderr);
      const logs = [];
      const output = { info: (value) => logs.push(value), warn: (value) => logs.push(value) };
      const off = executeSample(source, loader, output);
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
      const on = executeSample(source.replaceAll(" = false;", " = true;"), loader, output);
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
      analyzeScript(source, "typescript"),
      "\n",
    );
    assert.equal(
      (source + insertion.text).slice(insertion.selectionOffset, insertion.selectionOffset + 5),
      "false",
    );
    assert.equal(analyzeScript(source + insertion.text, "typescript").handlers[0], "onRequestHead");
    if (source.includes("next")) assert.match(insertion.text, /return next\(\)/);
    if (source.includes("type \*")) assert.match(insertion.text, /import \* as api2/);
    if (source.includes("traffic")) assert.match(insertion.text, /traffic.setHeader/);
  }
});
