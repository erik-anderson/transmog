import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkDebuggerProperties(page, workspace, root) {
  const prompt = workspace.getByRole("textbox", { name: "Immediate expression", exact: true });
  const showSample = async () => {
    await workspace.getByRole("button", { name: "Clear output", exact: true }).click();
    await prompt.fill("sample");
    await workspace.getByRole("button", { name: "Run", exact: true }).click();
    const result = workspace
      .locator(".command")
      .filter({ hasText: "› sample" })
      .locator("debug-value");
    await result.getByRole("button", { name: /Result/u }).click();
    return result;
  };
  try {
    await page.evaluate(() => {
      const inventory = {};
      inventory[2] = Array.from({ length: 150 }, (_, index) => {
        inventory[1000 + index] = [
          {
            name: "index",
            value: { kind: "number", display: String(index), handle: null, truncated: false },
          },
        ];
        return {
          name: "item" + index,
          value: { kind: "object", display: "[Object]", handle: 1000 + index, truncated: false },
        };
      });
      inventory[1149].push({
        name: "message",
        value: {
          kind: "string",
          display: "first line\nsecond line\n" + "x".repeat(900),
          handle: null,
          truncated: false,
        },
      });
      globalThis.__workspaceFixture.workspaceDebug.propertyInventory = inventory;
      globalThis.__workspaceFixture.workspaceDebug.rejectPropertyRead = 100;
    });
    const result = await showSample();
    await result.getByRole("button", { name: /item0$/u }).waitFor();
    await workspace.evaluate((element) => {
      element.consoleOutput.scrollTop = element.consoleOutput.scrollHeight;
    });
    await result.getByText("Property read failed", { exact: true }).waitFor();
    const failedReads = await page.evaluate(
      () =>
        globalThis.__workspaceFixture.workspaceDebug.actions.filter(
          (action) =>
            action.kind === "preview-properties" && action.handle === 2 && action.offset === 100,
        ).length,
    );
    await workspace.evaluate((element) => element.refresh());
    await page.waitForTimeout(350);
    assert.equal(
      await page.evaluate(
        () =>
          globalThis.__workspaceFixture.workspaceDebug.actions.filter(
            (action) =>
              action.kind === "preview-properties" && action.handle === 2 && action.offset === 100,
          ).length,
      ),
      failedReads,
      "A failed automatic read must wait for explicit retry",
    );
    await page.evaluate(() => {
      delete globalThis.__workspaceFixture.workspaceDebug.rejectPropertyRead;
    });
    await result.getByRole("button", { name: "Show more properties", exact: true }).click();
    await result.getByRole("button", { name: /item149$/u }).waitFor();
    assert.equal(
      await result.getByRole("button", { name: /item0$/u }).count(),
      1,
      "Loading more must retain earlier rows",
    );
    assert.equal(
      await workspace.getByRole("button", { name: "Previous properties", exact: true }).count(),
      0,
    );
    assert.equal(
      await workspace.getByRole("button", { name: "Next properties", exact: true }).count(),
      0,
    );
    await result.getByRole("button", { name: /item149$/u }).click();
    await result.getByText("149", { exact: true }).waitFor();
    const message = result.getByTitle("first line\nsecond line\n" + "x".repeat(900), {
      exact: true,
    });
    await message.waitFor();
    assert.ok(
      (await message.textContent()).includes("\\n"),
      "String previews must preserve line breaks as visible escapes",
    );
    assert.ok(
      (await message.boundingBox()).height < 30,
      "Long multiline values must keep one compact property row",
    );
    await result.getByRole("button", { name: /item149$/u }).click();
    await page.waitForFunction(() =>
      globalThis.__workspaceFixture.workspaceDebug.actions.some(
        (action) => action.kind === "release" && action.handles.includes(1149),
      ),
    );
    await result.getByRole("button", { name: /item0$/u }).scrollIntoViewIfNeeded();
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check", "debugger-scroll-properties.png"),
    });
    await page.evaluate(() => {
      globalThis.__workspaceFixture.workspaceDebug.propertyInventory[2] = Array.from(
        { length: 5000 },
        (_, index) => ({
          name: "item" + index,
          value: { kind: "number", display: String(index), handle: null, truncated: false },
        }),
      );
    });
    const large = await showSample();
    await large.getByRole("button", { name: /Properties 4901–5000/u }).click();
    await large.getByText("4999", { exact: true }).waitFor();
    assert.equal(
      await large.locator("tr").count(),
      150,
      "Only the expanded range should render property rows",
    );
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check", "debugger-property-ranges.png"),
    });
  } catch (error) {
    process.stderr.write(
      JSON.stringify(
        await workspace.evaluate((element) => ({
          scroll: {
            top: element.consoleOutput.scrollTop,
            height: element.consoleOutput.clientHeight,
            total: element.consoleOutput.scrollHeight,
          },
          values: [...element.consoleOutput.querySelectorAll("debug-value")].map((value) => ({
            rows: value.value?.rows.length,
            loading: value.value?.loading,
            issue: value.value?.issue,
            observer: !!value.moreObserver,
            more: [...value.shadowRoot.querySelectorAll("[data-more]")].map((node) => ({
              key: node.getAttribute("data-more"),
              top: node.getBoundingClientRect().top,
              height: node.getBoundingClientRect().height,
            })),
          })),
          actions: globalThis.__workspaceFixture.workspaceDebug.actions
            .filter((action) => ["preview-properties", "property"].includes(action.kind))
            .slice(-8),
        })),
        null,
        2,
      ) + "\n",
    );
    throw error;
  } finally {
    await page.evaluate(() => {
      delete globalThis.__workspaceFixture.workspaceDebug.propertyInventory;
      delete globalThis.__workspaceFixture.workspaceDebug.rejectPropertyRead;
    });
    await workspace.getByRole("button", { name: "Clear output", exact: true }).click();
  }
}
