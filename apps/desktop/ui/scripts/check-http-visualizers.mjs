import assert from "node:assert/strict";

export async function checkHttpVisualizers(page, visualizer, selector, tree) {
  const saved = await page.evaluate(() => {
    const s = globalThis.__workspaceFixture;
    return {
      visualizerDocument: structuredClone(s.visualizerDocument),
      visualizerSources: structuredClone(s.visualizerSources),
    };
  });
  await selector.selectOption("auto");
  try {
    await page.evaluate(() => {
      const s = globalThis.__workspaceFixture;
      s.visualizerSources.push({
        id: "decoded-http",
        name: "HTTP decoded response",
        length: "16",
        availability: "complete",
        sessionId: "first",
        boundary: "client-response",
        httpDecoded: { parent: { sourceId: "body", offset: "0", length: "32" }, codings: ["gzip"] },
      });
      s.visualizerDocument.roots[0].spans = [
        { sourceId: "decoded-http", offset: "1", length: "4" },
      ];
    });
    await selector.selectOption({ label: "Protocol structure" });
    await visualizer.locator(".visualizer-heading > strong").waitFor();
    await tree.focus();
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
    if (!(await visualizer.locator(".visualizer-ranges").evaluate((element) => element.open)))
      await visualizer.getByText("Source ranges (1)", { exact: true }).click();
    await visualizer
      .getByRole("button", { name: "HTTP decoded response · offset 1 · 4 bytes", exact: true })
      .click();
    await visualizer
      .locator(".visualizer-bytes .visualizer-format")
      .filter({ hasText: "HTTP content decoding (gzip) · enclosing parent regions only" })
      .waitFor();
    await visualizer
      .locator(".visualizer-bytes .visualizer-format")
      .filter({ hasText: "Parents: Original response body, offset 0, 32 bytes" })
      .waitFor();
    await visualizer
      .locator("hex-viewer")
      .getByText("4 bytes selected", { exact: false })
      .waitFor();
    assert.deepEqual(
      await visualizer
        .locator("hex-viewer")
        .evaluate((element) => ({ first: element.rangeStart, last: element.rangeEnd })),
      { first: "1", last: "4" },
    );
    await visualizer
      .getByRole("combobox", { name: "Visualization byte source", exact: true })
      .selectOption("body");
    assert.equal(
      await visualizer
        .locator(".visualizer-bytes .visualizer-format")
        .textContent()
        .then((text) => text.includes("HTTP content decoding")),
      false,
      "Original encoded bytes acquired a decoded label",
    );
  } finally {
    await selector.selectOption("auto");
    await page.evaluate((saved) => Object.assign(globalThis.__workspaceFixture, saved), saved);
    await selector.selectOption({ label: "Protocol structure" });
    await visualizer.locator(".visualizer-heading > strong").waitFor();
  }
}
