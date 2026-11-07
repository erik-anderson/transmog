import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

export async function checkExtensionDeclarations(page, root) {
  const panel = page.locator("extension-settings");
  const previous = await page.evaluate(() =>
    structuredClone(globalThis.__workspaceFixture.extensionStatus),
  );
  const entries = await Promise.all(
    ["binary", "mapi"].map(async (name) => ({
      catalog: {
        manifest: JSON.parse(
          await readFile(resolve(root, `../../../extensions/${name}/manifest.json`), "utf8"),
        ),
        enabled: true,
        origin: "bundled",
        digest: "b".repeat(64),
      },
      directory: null,
      hotReload: false,
    })),
  );
  const differing = structuredClone(entries[0]);
  differing.catalog.manifest.id = "test.different-conditions";
  differing.catalog.manifest.name = "Direction-specific conditions";
  differing.catalog.manifest.visualizers = differing.catalog.manifest.visualizers.slice(0, 2);
  for (const [index, value] of differing.catalog.manifest.visualizers.entries()) {
    value.matches = [
      {
        direction: index === 0 ? "request" : "response",
        contentTypes: ["application/example"],
        statuses: index ? [500] : [],
      },
    ];
  }
  const single = structuredClone(entries[0]);
  single.catalog.manifest.id = "test.single-viewer";
  single.catalog.manifest.name = "Single request viewer";
  single.catalog.manifest.visualizers = single.catalog.manifest.visualizers.slice(0, 1);
  single.catalog.manifest.visualizers[0].inputs.response.body = "optional";
  try {
    await page.evaluate(
      (additions) => {
        const fixture = globalThis.__workspaceFixture;
        fixture.extensionStatus.entries.push(...additions);
        fixture.emitNative("extensions-changed");
        globalThis.__extensionDebugTargets = [];
        globalThis.__extensionClipboard = Object.getOwnPropertyDescriptor(navigator, "clipboard");
        Object.defineProperty(navigator, "clipboard", {
          configurable: true,
          value: {
            writeText: async (text) => {
              globalThis.__extensionCopied = text;
            },
          },
        });
        globalThis.__extensionDebugListener = (event) => {
          event.stopPropagation();
          globalThis.__extensionDebugTargets.push(event.detail);
        };
        document
          .querySelector("app-shell")
          .settings.extensionsEditor.addEventListener(
            "extension-debug-request",
            globalThis.__extensionDebugListener,
          );
      },
      [...entries, differing, single],
    );
    const binary = panel.getByRole("article", { name: "Binary formats", exact: true });
    const mapi = panel.getByRole("article", { name: "MAPI/HTTP", exact: true });
    await binary.getByText("Visualizers and declared inputs", { exact: true }).click();
    await mapi.getByText("Visualizers and declared inputs", { exact: true }).click();
    assert.equal(await binary.locator(".extension-contribution").count(), 5);
    const mapiViewers = mapi
      .locator(".extension-declarations")
      .first()
      .locator(".extension-contribution");
    assert.equal(await mapiViewers.count(), 2);
    assert.equal(await binary.locator(".extension-input-details[open]").count(), 0);
    await binary
      .getByText(
        "Request and response · application/protobuf · application/x-protobuf · application/vnd.google.protobuf",
        { exact: true },
      )
      .waitFor();
    const protobuf = binary.locator(".extension-contribution").first();
    const declarations = protobuf.locator(".extension-input-details");
    const summary = declarations.locator("summary");
    await summary.focus();
    await page.keyboard.press("Enter");
    await protobuf
      .getByText("Request viewer: Required · HTTP decoding on demand", { exact: false })
      .waitFor();
    assert.deepEqual(
      await declarations.locator(".extension-input-value").allTextContents(),
      [
        "Request viewer: content-type, content-encoding",
        "Request viewer: Required · HTTP decoding on demand",
        "Response viewer: content-type, content-encoding",
        "Response viewer: Required · HTTP decoding on demand",
      ],
      "Empty opposite-message declarations were not omitted",
    );
    for (const direction of ["Request", "Response"]) {
      await protobuf
        .getByRole("button", { name: `Debug Protobuf · ${direction} in Traffic`, exact: true })
        .click();
    }
    assert.deepEqual(await page.evaluate(() => globalThis.__extensionDebugTargets), [
      { extensionId: "net.transmog.binary", visualizerId: "protobuf-request" },
      { extensionId: "net.transmog.binary", visualizerId: "protobuf-response" },
    ]);
    const callCount = await page.evaluate(
      () => globalThis.__workspaceFixture.calls.extension_status,
    );
    await summary.focus();
    const scrollTop = await page
      .locator("settings-workspace .settings-content")
      .evaluate((element) => element.scrollTop);
    await page.evaluate(() => {
      const fixture = globalThis.__workspaceFixture;
      const binary = fixture.extensionStatus.entries.find(
        (entry) => entry.catalog.manifest.id === "net.transmog.binary",
      ).catalog.manifest;
      [binary.visualizers[0], binary.visualizers[1]] = [
        binary.visualizers[1],
        binary.visualizers[0],
      ];
      fixture.emitNative("extensions-changed");
    });
    await page.waitForFunction(
      (before) =>
        globalThis.__workspaceFixture.calls.extension_status > before &&
        !document.querySelector("app-shell").settings.extensionsEditor.extensionsBusy,
      callCount,
    );
    assert.equal(
      await declarations.evaluate((element) => element.open),
      true,
      "A refresh collapsed full declarations",
    );
    assert.equal(
      await summary.evaluate((element) => element === element.getRootNode().activeElement),
      true,
      "A refresh lost declaration focus",
    );
    assert.equal(
      await page
        .locator("settings-workspace .settings-content")
        .evaluate((element) => element.scrollTop),
      scrollTop,
      "A refresh moved the settings scroll position",
    );
    await page.keyboard.press("Space");
    assert.equal(await declarations.evaluate((element) => element.open), false);

    const body = mapiViewers.first();
    const rules = body.locator(".extension-matches");
    assert.equal(await rules.evaluate((element) => element.open), false);
    assert.equal(
      await rules.locator("summary").innerText(),
      "Request and response · application/mapi-http · 23 matching rules",
    );
    await rules.locator("summary").focus();
    await page.keyboard.press("Enter");
    assert.equal(await rules.locator("p").count(), 23);
    await rules
      .getByText(
        "Request and response · application/mapi-http · request x-requesttype: equals Connect",
        { exact: true },
      )
      .waitFor();
    await page.keyboard.press("Space");
    assert.equal(await rules.evaluate((element) => element.open), false);
    await body.getByText("Full declarations", { exact: true }).click();
    await body
      .getByText("content-type, x-requesttype, x-clientinfo, content-encoding", { exact: true })
      .waitFor();
    await body
      .getByText("Response viewer: Optional · HTTP decoding on demand", { exact: false })
      .waitFor();
    const requestBody = body.locator(".extension-input-details dd").nth(1);
    assert.equal(
      await requestBody.textContent(),
      "Request viewer: Required · HTTP decoding on demand\nResponse viewer: Optional · HTTP decoding on demand",
    );
    await requestBody.focus();
    await page.keyboard.press("Shift+F10");
    await page
      .locator("inspection-copy-menu")
      .getByRole("menuitem", { name: "Copy value", exact: true })
      .click();
    assert.equal(
      await page.evaluate(() => globalThis.__extensionCopied),
      "Request viewer: Required · HTTP decoding on demand\\nResponse viewer: Optional · HTTP decoding on demand",
      "Copied direction-specific declarations ran together",
    );
    await body.locator(".extension-input-details").scrollIntoViewIfNeeded();
    assert.equal(
      await body
        .getByText(
          "Up to 64 earlier exchanges from the same capture, origin, and conversation; request x-clientinfo",
          { exact: true },
        )
        .count(),
      1,
    );
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/extensions-grouped-details.png"),
    });
    await body.getByText("Full declarations", { exact: true }).click();

    const custom = panel.getByRole("article", {
      name: "Direction-specific conditions",
      exact: true,
    });
    await custom.getByText("Visualizers and declared inputs", { exact: true }).click();
    assert.equal(await custom.locator(".extension-contribution").count(), 1);
    await custom.getByText("Request · application/example", { exact: true }).waitFor();
    await custom.getByText("Response · application/example · 500", { exact: true }).waitFor();

    const singleEntry = panel.getByRole("article", { name: "Single request viewer", exact: true });
    await singleEntry.getByText("Visualizers and declared inputs", { exact: true }).click();
    await singleEntry.getByText("Full declarations", { exact: true }).click();
    const pairedBody = singleEntry.getByText("Optional", { exact: true });
    await pairedBody.focus();
    await page.evaluate(() => {
      const fixture = globalThis.__workspaceFixture;
      const entry = fixture.extensionStatus.entries.find(
        (entry) => entry.catalog.manifest.id === "test.single-viewer",
      );
      entry.catalog.manifest.visualizers[0].inputs.response.headers = ["x-paired-context"];
      fixture.emitNative("extensions-changed");
    });
    await singleEntry.getByText("x-paired-context", { exact: true }).waitFor();
    assert.equal(
      await pairedBody.evaluate((element) => element === element.getRootNode().activeElement),
      true,
      "Adding a paired input moved focus to another declaration",
    );
    const pairedHeader = singleEntry.getByText("x-paired-context", { exact: true });
    await pairedHeader.focus();
    await page.evaluate(() => {
      const fixture = globalThis.__workspaceFixture;
      const entry = fixture.extensionStatus.entries.find(
        (entry) => entry.catalog.manifest.id === "test.single-viewer",
      );
      entry.catalog.manifest.visualizers[0].inputs.response.headers = [];
      fixture.emitNative("extensions-changed");
    });
    await pairedHeader.waitFor({ state: "detached" });
    assert.equal(
      await singleEntry
        .getByText("Full declarations", { exact: true })
        .evaluate((element) => element === element.getRootNode().activeElement),
      true,
      "Removing the focused paired input did not return focus to its declarations",
    );

    for (const [width, height] of [
      [1440, 900],
      [960, 720],
      [800, 600],
    ]) {
      await page.setViewportSize({ width, height });
      await binary
        .getByRole("heading", { name: "Binary formats", exact: true })
        .scrollIntoViewIfNeeded();
      assert.equal(
        await panel.evaluate((element) => element.scrollWidth <= element.clientWidth + 1),
        true,
        "Grouped declarations overflowed settings",
      );
      await page.screenshot({
        path: resolve(root, `../../../target/ui-check/extensions-grouped-${width}.png`),
      });
      await summary.click();
      await protobuf
        .getByText("Response viewer: Required · HTTP decoding on demand", { exact: true })
        .waitFor();
      await declarations.scrollIntoViewIfNeeded();
      assert.equal(
        await declarations.evaluate((element) => element.scrollWidth <= element.clientWidth + 1),
        true,
        "Full declarations overflowed settings",
      );
      await page.screenshot({
        path: resolve(root, `../../../target/ui-check/extensions-grouped-details-${width}.png`),
      });
      await summary.click();
    }
    await page.emulateMedia({ colorScheme: "dark" });
    await summary.click();
    await declarations.scrollIntoViewIfNeeded();
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/extensions-grouped-dark.png"),
    });
    await page.emulateMedia({ forcedColors: "active" });
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/extensions-grouped-forced-colors.png"),
    });
    await page.emulateMedia({ forcedColors: "none" });
    const zoom = await page.evaluate(() => {
      const previousZoom = document.documentElement.style.zoom;
      document.documentElement.style.zoom = "1.5";
      return previousZoom;
    });
    try {
      const debug = protobuf.getByRole("button", {
        name: "Debug Protobuf · Response in Traffic",
        exact: true,
      });
      await debug.focus();
      assert.equal(
        await declarations.evaluate((element) => element.scrollWidth <= element.clientWidth + 1),
        true,
        "Zoomed full declarations overflowed settings",
      );
      const bounds = await debug.boundingBox();
      assert.ok(
        bounds && bounds.x >= 0 && bounds.x + bounds.width <= 800,
        "Zoomed debug actions were clipped",
      );
      await page.screenshot({
        path: resolve(root, "../../../target/ui-check/extensions-grouped-zoom.png"),
      });
    } finally {
      await page.evaluate((previousZoom) => {
        document.documentElement.style.zoom = previousZoom;
      }, zoom);
    }
  } finally {
    await page.emulateMedia({ forcedColors: "none", colorScheme: "light" });
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.evaluate((status) => {
      document
        .querySelector("app-shell")
        .settings.extensionsEditor.removeEventListener(
          "extension-debug-request",
          globalThis.__extensionDebugListener,
        );
      delete globalThis.__extensionDebugListener;
      delete globalThis.__extensionDebugTargets;
      if (globalThis.__extensionClipboard)
        Object.defineProperty(navigator, "clipboard", globalThis.__extensionClipboard);
      else delete navigator.clipboard;
      delete globalThis.__extensionClipboard;
      delete globalThis.__extensionCopied;
      globalThis.__workspaceFixture.extensionStatus = status;
      globalThis.__workspaceFixture.emitNative("extensions-changed");
    }, previous);
    await page.waitForFunction(
      () => !document.querySelector("app-shell").settings.extensionsEditor.extensionsBusy,
    );
  }
}
