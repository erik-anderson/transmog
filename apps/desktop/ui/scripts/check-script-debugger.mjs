import assert from "node:assert/strict";
import { resolve } from "node:path";
import { writeFile } from "node:fs/promises";

export async function checkScriptDebugger(page, root) {
  const editor = page.locator("script-editor");
  await page.waitForFunction(
    () => document.querySelector("app-shell").shadowRoot.querySelector("script-editor").scriptReady,
  );
  await editor.locator(".debug-options>summary").click();
  await editor.getByRole("textbox", { name: "Breakpoints before initialization" }).fill("2, 200");
  await editor.locator(".monaco-editor-host").first().scrollIntoViewIfNeeded();
  const gutter = await editor.evaluate((element) => {
    const source = element.sourceEditor;
    const bounds = source.getDomNode().getBoundingClientRect();
    const layout = source.getLayoutInfo();
    const line = source.getScrolledVisiblePosition({ lineNumber: 1, column: 1 });
    return {
      x: bounds.left + layout.glyphMarginLeft + layout.glyphMarginWidth / 2,
      y: bounds.top + line.top + line.height / 2,
    };
  });
  await page.mouse.click(gutter.x, gutter.y);
  assert.equal(
    await editor.getByRole("textbox", { name: "Breakpoints before initialization" }).inputValue(),
    "1, 2, 200",
  );
  await page.mouse.click(gutter.x, gutter.y);
  assert.equal(
    await editor.getByRole("textbox", { name: "Breakpoints before initialization" }).inputValue(),
    "2, 200",
  );
  await editor.locator(".script-breakpoint").first().waitFor({ state: "visible" });
  await editor.getByRole("button", { name: "Debug test", exact: true }).click();
  const panel = editor.locator("script-debugger");
  const waitPaused = async () => {
    await page.waitForFunction(
      () => document.querySelector("app-shell").shadowRoot.querySelector("script-debugger")?.paused,
    );
    await panel.getByRole("cell", { name: "7", exact: true }).waitFor();
  };
  await waitPaused();
  assert.equal(
    (await page.evaluate(() => globalThis.__workspaceFixture.debugTestRequest)).configuration
      .breakpoints.length,
    2,
  );
  assert.equal(await panel.locator("img,script").count(), 0);
  assert.ok(
    await panel
      .locator(".debugger")
      .evaluate(
        (element) =>
          element.matches(":modal") && element.contains(element.getRootNode().activeElement),
      ),
  );
  // Local scope loads without a scope click. Object expansion retains the parent rows.
  await panel.getByRole("button", { name: "object", exact: true }).click();
  await panel.getByRole("cell", { name: "key-99", exact: true }).waitFor();
  assert.equal(await panel.getByRole("cell", { name: "7", exact: true }).count(), 1);
  await panel.getByRole("button", { name: "Next properties", exact: true }).click();
  await panel.getByRole("cell", { name: "key-104", exact: true }).waitFor();
  await panel.getByRole("button", { name: "Previous properties", exact: true }).click();
  await panel.getByRole("cell", { name: "key-99", exact: true }).waitFor();
  await panel.getByRole("button", { name: "Toggle breakpoint at line 1", exact: true }).click();
  assert.equal(
    await panel
      .getByRole("button", { name: "Toggle breakpoint at line 1" })
      .getAttribute("aria-pressed"),
    "true",
  );
  // Native byte limits can shorten a page before its 100-property ceiling.
  await page.evaluate(() => {
    globalThis.__workspaceFixture.debugPropertyPageSize = 37;
  });
  await panel.getByRole("button", { name: "local · nested", exact: true }).click();
  // key-36 also exists on the previous 100-property page. Wait for that page to
  // be replaced before treating it as evidence that the shorter page is ready.
  await panel.getByRole("cell", { name: "key-99", exact: true }).waitFor({ state: "detached" });
  await panel.getByRole("cell", { name: "key-36", exact: true }).waitFor();
  await panel.getByRole("button", { name: "Next properties", exact: true }).click();
  await panel.getByRole("cell", { name: "key-73", exact: true }).waitFor();
  await panel.getByRole("button", { name: "Next properties", exact: true }).click();
  await panel.getByRole("cell", { name: "key-104", exact: true }).waitFor();
  await panel.getByRole("button", { name: "Previous properties", exact: true }).click();
  await panel.getByRole("cell", { name: "key-73", exact: true }).waitFor();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.debugPropertyPageSize = 100;
  });
  await panel.getByRole("button", { name: "local · nested", exact: true }).click();
  await panel.getByRole("cell", { name: "key-99", exact: true }).waitFor();
  await panel
    .getByRole("combobox", { name: "Pause on exceptions", exact: true })
    .selectOption("all");

  await page.evaluate(() => {
    globalThis.__workspaceFixture.debugAckDelay = 250;
    globalThis.__workspaceFixture.debugLongSource = true;
    globalThis.__workspaceFixture.debugSourceLine = 100;
  });
  for (const name of ["Step over", "Step into", "Step out"]) {
    await panel.getByRole("button", { name: new RegExp(`^${name}`) }).click();
    await waitPaused();
    // Expansion is restored using new pause handles; no stale table or manual scope click.
    await panel.getByRole("cell", { name: "key-99", exact: true }).waitFor();
  }
  // Function keys continue to work while a native select has focus.
  await panel.getByRole("combobox", { name: "Pause on exceptions", exact: true }).focus();
  await page.keyboard.press("F10");
  await waitPaused();
  await panel.getByRole("cell", { name: "key-99", exact: true }).waitFor();
  assert.deepEqual(
    (await page.evaluate(() => globalThis.__workspaceFixture.debugCommands))
      .filter((value) => value.kind === "resume")
      .map((value) => value.action),
    ["over", "into", "out", "over"],
  );

  for (const [name, width, height] of [
    ["wide", 1440, 900],
    ["compact", 960, 720],
    ["small", 800, 600],
  ]) {
    await page.setViewportSize({ width, height });
    await page.mouse.move(width - 50, 30);
    await page.waitForFunction(
      (element) => {
        const line = element.querySelector(".current").getBoundingClientRect();
        const top = element.getBoundingClientRect().top + element.clientTop;
        return line.top >= top && line.bottom <= top + element.clientHeight;
      },
      await panel.locator(".source-code").elementHandle(),
    );
    assert.ok(
      await panel.locator(".debugger").evaluate((element) => {
        const rect = element.getBoundingClientRect();
        const controls = element.querySelector(".debug-controls").getBoundingClientRect();
        const source = element.querySelector(".source-code");
        const code = source.querySelector(".current").getBoundingClientRect();
        const viewport = source.getBoundingClientRect();
        return (
          rect.top >= 0 &&
          rect.bottom <= innerHeight + 1 &&
          controls.bottom < innerHeight &&
          element.scrollWidth <= element.clientWidth + 1 &&
          code.top >= viewport.top &&
          code.bottom <= viewport.bottom
        );
      }),
      "Paused statement or controls outside the debugger viewport",
    );
    await page.screenshot({
      path: resolve(root, `../../../target/ui-check/script-debugger-${name}.png`),
    });
    const valuesTab = panel.getByRole("button", { name: "Stack and variables", exact: true });
    if (await valuesTab.isVisible()) {
      await valuesTab.click();
      assert.ok(
        await panel
          .locator(".property-page")
          .evaluate(
            (element) =>
              element.getBoundingClientRect().bottom <= innerHeight && element.clientHeight >= 100,
          ),
      );
      await page.screenshot({
        path: resolve(root, `../../../target/ui-check/script-debugger-${name}-variables.png`),
      });
    }
  }
  await page.setViewportSize({ width: 1440, height: 900 });
  const divider = panel.getByRole("separator", {
    name: "Resize source and variables",
    exact: true,
  });
  for (let index = 0; index < 20; index++) await divider.press("ArrowRight");
  assert.equal(await divider.getAttribute("aria-valuenow"), "85");
  assert.ok(
    await panel
      .locator(".debug-grid")
      .evaluate(
        (element) =>
          element.querySelector(".debug-source").getBoundingClientRect().width /
            element.getBoundingClientRect().width >
          0.8,
      ),
  );
  await divider.press("Home");
  assert.equal(await divider.getAttribute("aria-valuenow"), "60");
  const resume = panel.getByRole("button", { name: "Continue (F8)", exact: true });
  await resume.hover();
  const tooltip = panel.getByRole("tooltip").filter({ hasText: /^Continue F8/ });
  await tooltip.waitFor({ state: "visible" });
  assert.equal(
    await tooltip
      .locator("kbd")
      .first()
      .evaluate((element) => getComputedStyle(element).color),
    await tooltip.evaluate((element) => getComputedStyle(element).color),
  );
  await resume.focus();
  await page.mouse.move(1400, 10);
  await tooltip.waitFor({ state: "visible" });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/script-debugger-tooltip.png"),
  });
  for (const colorScheme of ["light", "dark"]) {
    await page.emulateMedia({ colorScheme, reducedMotion: "reduce" });
    await page.waitForFunction(
      (scheme) => getComputedStyle(document.querySelector("app-shell")).colorScheme === scheme,
      colorScheme,
    );
    assert.ok(
      await tooltip.evaluate((element) => {
        const linear = (color) =>
          color
            .match(/[\d.]+/g)
            .slice(0, 3)
            .map(Number)
            .map((value) => value / 255)
            .map((value) => (value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4));
        const luminance = (color) => {
          const [r, g, b] = linear(color);
          return 0.2126 * r + 0.7152 * g + 0.0722 * b;
        };
        const style = getComputedStyle(element);
        const values = [luminance(style.color), luminance(style.backgroundColor)].sort(
          (a, b) => b - a,
        );
        return (values[0] + 0.05) / (values[1] + 0.05) >= 4.5;
      }),
      `Insufficient ${colorScheme} shortcut tooltip contrast`,
    );
    const colors = await resume.evaluate((element) => {
      const style = getComputedStyle(element),
        icon = element.querySelector("debug-icon").shadowRoot.querySelector("svg");
      const luminance = (color) => {
        const [r, g, b] = color
          .match(/[\d.]+/g)
          .slice(0, 3)
          .map(Number)
          .map((value) => value / 255)
          .map((value) => (value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4));
        return 0.2126 * r + 0.7152 * g + 0.0722 * b;
      };
      const levels = [luminance(style.color), luminance(style.backgroundColor)].sort(
        (a, b) => b - a,
      );
      return {
        disabled: element.disabled,
        foreground: style.color,
        background: style.backgroundColor,
        opacity: style.opacity,
        icon: getComputedStyle(icon).stroke,
        contrast: (levels[0] + 0.05) / (levels[1] + 0.05),
        html: element.outerHTML,
      };
    });
    assert.ok(colors.contrast >= 4.5, `Insufficient ${colorScheme} Continue icon contrast`);
    await writeFile(
      resolve(root, `../../../target/ui-check/debugger-${colorScheme}-colors.json`),
      JSON.stringify(colors, null, 2),
    );
    await page.screenshot({
      path: resolve(root, `../../../target/ui-check/script-debugger-${colorScheme}.png`),
    });
  }
  await page.emulateMedia({ colorScheme: "light", reducedMotion: "no-preference" });
  await page.emulateMedia({ forcedColors: "active" });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/script-debugger-forced-colors.png"),
  });
  await page.emulateMedia({ forcedColors: "none" });

  await panel.getByRole("button", { name: "Return to editor", exact: true }).click();
  await editor.getByRole("button", { name: "Show debugger", exact: true }).waitFor();
  assert.ok(
    await editor
      .getByRole("button", { name: "Show debugger", exact: true })
      .evaluate((element) => element === element.getRootNode().activeElement),
  );
  assert.equal(await panel.evaluate((element) => element.paused), true);
  await editor.evaluate((element) => element.markScriptChanged());
  await editor.getByRole("button", { name: "Show debugger", exact: true }).click();
  await panel.getByText(/Draft edits are newer/).waitFor();
  await page.keyboard.press("F8");
  await panel.getByText("Callback completed", { exact: true }).waitFor();
  assert.equal(await resume.isDisabled(), true);
  // Disabled controls still explain themselves on hover.
  await resume.locator("..").hover();
  await tooltip.waitFor({ state: "visible" });
  await panel
    .getByRole("button", { name: "Restart with current draft and fixture", exact: true })
    .click();
  await waitPaused();
  const configuration = (await page.evaluate(() => globalThis.__workspaceFixture.debugTestRequest))
    .configuration;
  assert.equal(configuration.exceptions, "all");
  assert.deepEqual(
    configuration.breakpoints.map((value) => value.location.line).sort((a, b) => a - b),
    [1, 2, 200],
  );
  await panel.locator(".debugger").focus();
  await page.keyboard.press("Control+Backslash");
  await panel.getByText("Callback completed", { exact: true }).waitFor();
  await panel.getByRole("button", { name: "Close debugger", exact: true }).click();
  await panel.waitFor({ state: "detached" });

  await page.evaluate(() => {
    globalThis.__workspaceFixture.debugInitializerScope = true;
  });
  await editor.getByRole("button", { name: "Debug test", exact: true }).click();
  await panel.getByRole("cell", { name: "41", exact: true }).waitFor();
  await panel.getByRole("button", { name: "Step over (F10)", exact: true }).click();
  await waitPaused();
  assert.equal(
    await panel
      .getByRole("button", { name: "local · nested", exact: true })
      .getAttribute("aria-pressed"),
    "true",
  );
  await panel.getByRole("button", { name: "Stop and close debugger", exact: true }).click();
  await panel.waitFor({ state: "detached" });
  await page.evaluate(() => {
    globalThis.__workspaceFixture.debugInitializerScope = false;
  });

  await editor.getByRole("button", { name: "Save & enable", exact: true }).click();
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("script-editor").scriptActive,
  );
  await editor.locator(".debug-live>summary").click();
  await editor.getByRole("button", { name: "Review applied revision", exact: true }).click();
  const reservation = editor.getByRole("region", { name: "Live debugger reservation" });
  await reservation
    .getByRole("textbox", { name: "Host (blank matches all)", exact: true })
    .fill("example.test");
  await reservation.getByRole("button", { name: "Arm for 60 seconds", exact: true }).click();
  await panel
    .getByText("Armed · waiting for the next matching exchange", { exact: true })
    .waitFor();
  assert.equal(await panel.getByRole("button", { name: /^Restart/ }).count(), 0);
  assert.equal(
    (await page.evaluate(() => globalThis.__workspaceFixture.liveDebugRequest)).filter.host,
    "example.test",
  );
  await page.evaluate(() => globalThis.__workspaceFixture.claimDebugExchange());
  await waitPaused();
  await panel.getByText(/selected-live-exchange/).waitFor();
  await editor.evaluate((element) => element.markScriptChanged());
  await panel.getByText(/Applying a revision ends this live debugger/).waitFor();
  await panel.getByRole("button", { name: "Stop and close debugger", exact: true }).click();
  await panel.waitFor({ state: "detached" });
  assert.ok(await page.evaluate(() => globalThis.__workspaceFixture.debugClosedSession));
}
