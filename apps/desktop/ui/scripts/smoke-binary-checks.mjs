import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { smokeBinary } from "./smoke-binary.mjs";

const { chromium } = createRequire(
  new URL("../../../../e2e/playwright/package.json", import.meta.url),
)("playwright");
const { values } = parseArgs({
  options: { port: { type: "string" }, output: { type: "string" } },
});
assert.ok(values.port && values.output, "--port and --output are required");
const output = resolve(values.output);
const browser = await chromium.connectOverCDP("http://127.0.0.1:" + values.port);
try {
  const context = browser.contexts()[0];
  const page = context.pages().find((page) => page.url().includes("transmog-ui"));
  assert.ok(page, "Transmog application page is missing");
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.waitForFunction(() => document.querySelector("app-shell")?.traffic?.isConnected);
  const cdp = await context.newCDPSession(page);
  const result = await smokeBinary(
    (method, params) => cdp.send(method, params),
    (expression) => page.evaluate(expression),
    join(output, "binary.png"),
    join(output, "binary-fixture.saz"),
  );
  assert.deepEqual(errors, []);
  await writeFile(join(output, "binary-result.json"), JSON.stringify(result, null, 2) + "\n");
  process.stdout.write(
    "Packaged binary formats, deferred BOND ranges and native rendering verified.\n",
  );
} finally {
  await browser.close();
}
