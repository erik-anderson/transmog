import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { after, before, test } from "node:test";
import { build } from "esbuild";
import { monacoCompletionLifecycle } from "./monaco-completion-lifecycle.mjs";

const { chromium } = createRequire(
  new URL("../../../../e2e/playwright/package.json", import.meta.url),
)("playwright");
let browser, page;
before(async () => {
  const result = await build({
    absWorkingDir: fileURLToPath(new URL("../", import.meta.url)),
    stdin: {
      contents: `
        import { CompletionAdapter } from "./node_modules/monaco-editor/esm/vs/languages/features/common/lspLanguageFeatures.js";
        import { editor, Uri, Position, languages } from "monaco-editor/editor/editor.api";
        globalThis.completionFixture = { CompletionAdapter, editor, Uri, Position, languages };
      `,
      resolveDir: fileURLToPath(new URL("../", import.meta.url)),
    },
    bundle: true,
    write: false,
    format: "iife",
    loader: { ".css": "empty", ".ttf": "dataurl" },
    plugins: [monacoCompletionLifecycle()],
  });
  browser = await chromium.launch({ headless: true });
  page = await browser.newPage();
  await page.addScriptTag({ content: result.outputFiles[0].text });
});
after(async () => browser?.close());

for (const stage of ["worker startup", "completion response"]) {
  for (const action of ["dispose", "cancel"]) {
    test(`${action} during ${stage} rejects the obsolete completion without reading the model`, async () => {
      const result = await page.evaluate(
        async ({ stage, action }) => {
          const { CompletionAdapter, editor, Uri, Position } = globalThis.completionFixture;
          const model = editor.createModel(
            "value",
            "json",
            Uri.parse("inmemory://completion/test.json"),
          );
          const token = { isCancellationRequested: false };
          let resume,
            entered,
            calls = 0,
            reads = 0;
          const ready = new Promise((resolve) => (entered = resolve));
          const gate = new Promise((resolve) => (resume = resolve));
          const word = model.getWordUntilPosition.bind(model);
          model.getWordUntilPosition = (...args) => {
            reads++;
            return word(...args);
          };
          const worker = {
            async doComplete() {
              calls++;
              if (stage === "completion response") {
                entered();
                await gate;
              }
              return { items: [{ label: "value", kind: 6 }] };
            },
          };
          const adapter = new CompletionAdapter(async () => {
            if (stage === "worker startup") {
              entered();
              await gate;
            }
            return worker;
          }, []);
          try {
            const pending = adapter.provideCompletionItems(model, new Position(1, 6), {}, token);
            await ready;
            if (action === "dispose") model.dispose();
            else token.isCancellationRequested = true;
            resume();
            const completion = await pending;
            return { empty: completion === undefined, reads, calls };
          } finally {
            model.dispose();
          }
        },
        { stage, action },
      );
      assert.deepEqual(result, {
        empty: true,
        reads: 0,
        calls: stage === "worker startup" ? 0 : 1,
      });
    });
  }
}

test("a live request retains completion text, kind and replacement range", async () => {
  const result = await page.evaluate(async () => {
    const { CompletionAdapter, editor, Uri, Position, languages } = globalThis.completionFixture;
    const model = editor.createModel("value", "json", Uri.parse("inmemory://completion/live.json"));
    const adapter = new CompletionAdapter(
      async () => ({
        doComplete: async () => ({ items: [{ label: "value", insertText: "valid", kind: 6 }] }),
      }),
      [],
    );
    try {
      const result = await adapter.provideCompletionItems(
        model,
        new Position(1, 6),
        {},
        {
          isCancellationRequested: false,
        },
      );
      const suggestion = result.suggestions[0];
      return {
        label: suggestion.label,
        text: suggestion.insertText,
        kindMatches: suggestion.kind === languages.CompletionItemKind.Variable,
        range: suggestion.range,
      };
    } finally {
      model.dispose();
    }
  });
  assert.deepEqual(result, {
    label: "value",
    text: "valid",
    kindMatches: true,
    range: { startLineNumber: 1, startColumn: 1, endLineNumber: 1, endColumn: 6 },
  });
});
