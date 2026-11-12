import assert from "node:assert/strict";
import { resolve } from "node:path";
import { waitForBodySource } from "./check-body-source.mjs";

async function inspectorGeometry(inspector) {
  return inspector.evaluate((element) => {
    const toolbar = element.querySelector(".body-toolbar"),
      facts = element.querySelector(".body-facts"),
      body = element.querySelector(".body-scroll"),
      toolbarBox = toolbar.getBoundingClientRect(),
      factsBox = facts.getBoundingClientRect(),
      controls = [...toolbar.children].filter(
        (control) => control !== facts && control.getBoundingClientRect().width,
      );
    return {
      overflow: toolbar.scrollWidth > toolbar.clientWidth + 1,
      factsShareRow: controls.every((control) => {
        const box = control.getBoundingClientRect();
        return box.top < factsBox.bottom && box.bottom > factsBox.top;
      }),
      factsWidth: factsBox.width,
      toolbarWidth: toolbarBox.width,
      bodyHeight: body.getBoundingClientRect().height,
      controlsInside: controls.every((control) => {
        const box = control.getBoundingClientRect();
        return box.left >= toolbarBox.left && box.right <= toolbarBox.right + 1;
      }),
    };
  });
}

export async function checkInspectorLayout(page, root) {
  const viewport = page.viewportSize();
  await page.goto("https://workspace.test/");
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.evaluate(() => {
    globalThis.__workspaceFixture.bodyOverride = { mediaType: "application/json" };
  });
  await page.locator('tr[data-session-id="first"]').click();
  const traffic = page.locator("traffic-workspace"),
    inspector = traffic.locator('message-inspector[side="response"]'),
    facts = inspector.locator(".body-facts"),
    factsSummary = facts.locator("summary"),
    viewer = inspector.getByRole("combobox", { name: "Body viewer", exact: true });
  await traffic.getByRole("button", { name: "Expand inspector", exact: true }).click();
  await inspector.getByRole("button", { name: "Body", exact: true }).click();
  await waitForBodySource(inspector);
  await factsSummary.getByText("Auto · JSON", { exact: true }).waitFor();
  await inspector.getByRole("group", { name: "JSON body view", exact: true }).waitFor();
  const evidence = await facts.textContent();

  // Hosts may already label the direction; hiding its repeated visual title
  // must retain an accessible region name and leave the current control intact.
  assert.equal(await inspector.locator(".message-tabs > strong").isVisible(), true);
  await viewer.focus();
  await inspector.evaluate((element) => (element.hideHeading = true));
  assert.equal(await inspector.locator(".message-tabs > strong").isVisible(), false);
  assert.equal(
    await inspector.getByRole("region", { name: "Response", exact: true }).isVisible(),
    true,
  );
  assert.equal(await viewer.evaluate((control) => control.matches(":focus")), true);
  await inspector.evaluate((element) => (element.hideHeading = false));

  const wide = await inspectorGeometry(inspector);
  assert.equal(wide.overflow, false, "Body controls overflowed the wide inspector");
  assert.equal(wide.controlsInside, true, "A primary body control escaped the inspector");
  assert.equal(wide.factsShareRow, true, "Wide body metadata forced another toolbar band");
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/inspector-wide.png") });

  await factsSummary.focus();
  await factsSummary.press("Space");
  await facts.locator("p").waitFor({ state: "visible" });
  const expanded = await inspectorGeometry(inspector);
  assert.ok(
    expanded.factsWidth > expanded.toolbarWidth * 0.9,
    "Expanded retention evidence did not have its own full-width row",
  );
  assert.equal(await facts.textContent(), evidence, "Expanding metadata changed its evidence");
  await factsSummary.press("Space");
  assert.equal(await factsSummary.evaluate((control) => control.matches(":focus")), true);

  await page.setViewportSize({ width: 800, height: 600 });
  const responseSwitch = traffic.getByRole("radio", { name: "Response", exact: true });
  if (await responseSwitch.isVisible()) await responseSwitch.check();
  const compact = await inspectorGeometry(inspector);
  assert.equal(compact.overflow, false, "Body toolbar overflowed the compact inspector");
  assert.equal(compact.controlsInside, true, "Compact layout clipped a primary body control");
  assert.ok(compact.bodyHeight >= 48, "Compact toolbar left no readable body surface");
  assert.equal(await facts.textContent(), evidence, "Resizing changed retention evidence");
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/inspector-small.png") });

  const previousZoom = await page.evaluate(() => {
    const previous = document.documentElement.style.zoom;
    document.documentElement.style.zoom = "1.5";
    return previous;
  });
  try {
    await page.emulateMedia({ forcedColors: "active" });
    const zoomed = await inspectorGeometry(inspector);
    assert.equal(zoomed.overflow, false, "Zoomed body toolbar overflowed horizontally");
    assert.equal(zoomed.controlsInside, true, "Zoom clipped a primary body control");
    await viewer.focus();
    assert.equal(await viewer.evaluate((control) => control.matches(":focus-visible")), true);
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/inspector-zoom-forced-colors.png"),
    });
    await inspector.locator(".body-options > summary").click();
    await factsSummary.click();
    const save = inspector.getByRole("button", { name: "Save as…", exact: true });
    await save.hover();
    assert.equal(
      await save.evaluate((control) => {
        const box = control.getBoundingClientRect(),
          viewport = control.closest(".message-content").getBoundingClientRect();
        return box.top >= viewport.top && box.bottom <= viewport.bottom + 1;
      }),
      true,
      "Expanded options and evidence made Save as unreachable",
    );
    await facts.locator("p").hover();
    assert.equal(await facts.textContent(), evidence);
    await factsSummary.click();
    await inspector.locator(".body-options > summary").click();
    await inspector.getByRole("button", { name: "Headers", exact: true }).click();
    await inspector.getByRole("button", { name: "Copy headers", exact: true }).waitFor();
    await inspector.getByRole("button", { name: "Body", exact: true }).click();
    assert.equal(await viewer.inputValue(), "auto", "Switching modes reset the body viewer");
    assert.equal(await facts.textContent(), evidence, "Mode switching changed retention evidence");
  } finally {
    await page.evaluate((zoom) => (document.documentElement.style.zoom = zoom), previousZoom);
    await page.emulateMedia({ forcedColors: "none" });
    await page.setViewportSize(viewport);
  }
}
