import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { referenceCases } from "../../../../scripts/binary-reference-fixtures.mjs";

/** Render real parser output through the production host template under Trusted Types. */
export async function checkBinaryVisualizers(page, root) {
  const binary = resolve(root, "../../../extensions/binary");
  execFileSync(process.execPath, [resolve(binary, "build.mjs")], { windowsHide: true });
  const text = Array.from(Buffer.from("hello <script>inert</script>"));
  const fixtures = [
    ["protobuf", [8, 150, 1, 18, text.length, ...text, 26, 3, 8, 150, 1]],
    ["bond", [0x29, 3, 51, 46, 48, 0]],
    ["protobuf", [8, 1, 18, 5, 97]],
    ["protobuf", []],
    ...referenceCases
      .filter((item) => !item.name.endsWith("-right"))
      .map((item) => [
        item.name.split("-")[0],
        Array.from(Buffer.from(item.bodyHex, "hex")),
        item.contentType,
      ]),
    [
      "bond",
      Array.from({ length: 50_000 }, () => [0x23, 7, 0]).flat(),
      "application/bond-compact-binary",
      true,
    ],
  ];
  for (const [index, [kind, bytes, contentType, ranges]] of fixtures.entries()) {
    const parser = await import(pathToFileURL(resolve(binary, `dist/runtime/${kind}.js`)).href);
    const body = { sourceId: "body", length: String(bytes.length), availability: "complete" };
    let step = parser.visualize({
      instanceId: "browser",
      exchangeId: "first",
      requestNumber: "1",
      generation: "1",
      direction: "response",
      response: {
        evidenceId: "first",
        headers: contentType
          ? [
              {
                index: 0,
                name: "content-type",
                redacted: false,
                value: Array.from(Buffer.from(contentType)),
              },
            ]
          : [],
        body,
      },
    });
    while (step.kind === "needInput") {
      step = parser.resume(
        step.token,
        step.reads.map((read) => ({
          sourceId: "body",
          offset: read.offset,
          bytes: bytes.slice(Number(read.offset), Number(read.offset) + read.maxBytes),
        })),
      );
    }
    assert.equal(step.kind, "document");
    const document = step.document;
    await page.goto("https://workspace.test/");
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.evaluate(
      ({ document: parsedDocument, kind, length }) => {
        const s = globalThis.__workspaceFixture;
        const workspace = document.querySelector("app-shell").traffic;
        workspace.preferences = { ...workspace.preferences, layout: "stacked", listSplit: 30 };
        s.visualizerChoices = {
          body: [
            {
              extensionId: "net.transmog.binary",
              visualizerId: kind + "-response",
              name: parsedDocument.title,
            },
          ],
        };
        s.visualizerDocument = parsedDocument;
        s.visualizerLiveAvailable = false;
        s.visualizerSources = [
          {
            id: "body",
            name: "Original response body",
            length: String(length),
            availability: "complete",
            sessionId: "first",
            boundary: "client-response",
          },
        ];
      },
      { document, kind, length: bytes.length },
    );
    const traffic = page.locator("traffic-workspace");
    await traffic.locator('tr[data-session-id="first"]').click();
    const response = traffic.locator('message-inspector[side="response"]');
    await response.getByRole("button", { name: "Body", exact: true }).click();
    await response
      .getByRole("combobox", { name: "Body viewer", exact: true })
      .selectOption({ label: document.title });
    const viewer = response.locator("extension-visualizer");
    await viewer
      .locator(".visualizer-heading > strong")
      .filter({ hasText: document.title })
      .waitFor();
    assert.equal(await viewer.locator(".status-badge").textContent(), document.status);
    await viewer.evaluate(async (element) => {
      const walk = (node) => [node, ...(node.children ?? []).flatMap(walk)];
      for (const node of element.view.document.roots.flatMap(walk))
        if (node.children?.length) await element.toggleNode(node.id);
    });
    if (index === 0) {
      await viewer.getByRole("tree").getByText("Possible UTF-8 text", { exact: true }).waitFor();
      assert.equal(await viewer.locator("script,img").count(), 0);
      const tree = viewer.getByRole("tree");
      await tree.focus();
      await page.keyboard.press("ArrowDown");
      await page.keyboard.press("Enter");
      await viewer.getByRole("region", { name: "Selected structure", exact: true }).waitFor();
      await page.keyboard.press("Escape");
    }
    if (index === 1)
      await viewer
        .getByRole("tree")
        .getByText("Possible Compact Binary v1 (inferred)", { exact: true })
        .waitFor();
    if (index === 2)
      await viewer.getByRole("tree").getByText("Unparsed bytes", { exact: true }).waitFor();
    if (ranges) {
      const walk = (node) => [node, ...(node.children ?? []).flatMap(walk)];
      let current = document;
      for (const offset of [149997, 0]) {
        for (let count = 0; count < 20; count++) {
          const all = current.roots.flatMap(walk);
          const field = all.find(
            (node) => node.name === "Field 1 · uint8" && node.spans[0].offset === String(offset),
          );
          if (field) {
            assert.equal(field.value.value, "7");
            await viewer.evaluate((element, id) => element.selectNode(id, false), field.id);
            break;
          }
          const pending = all.find(
            (node) =>
              node.childrenToken &&
              node.spans.some(
                (span) =>
                  Number(span.offset) <= offset &&
                  Number(span.offset) + Number(span.length) > offset,
              ),
          );
          assert.ok(pending, "No range reaches the requested Bond field");
          current = parser.expand(pending.childrenToken).document;
          await page.evaluate((doc) => {
            globalThis.__workspaceFixture.nextExpandedDocument = doc;
          }, current);
          const evicted = await viewer.evaluate(async (element, id) => {
            const selected = element.visualizerRows.find((row) => row.selected)?.id;
            await element.toggleNode(id);
            const walk = (node) => [node, ...(node.children ?? []).flatMap(walk)];
            return (
              selected &&
              !element.view.document.roots.flatMap(walk).some((node) => node.id === selected) &&
              selected !== "$document"
            );
          }, pending.id);
          if (evicted && offset === 0) {
            assert.equal(
              await viewer.evaluate(
                (element) => element.visualizerRows.find((row) => row.selected)?.id,
              ),
              pending.id,
            );
            assert.equal(
              await viewer.locator(".visualizer-node").isVisible(),
              false,
              "Paging reopened closed details",
            );
          }
          assert.ok(
            (await viewer.getByRole("treeitem").count()) < 100,
            "Large Bond display stopped virtualizing rows",
          );
          assert.ok(count < 19, "Range expansion made no progress");
        }
      }
    }
    if (kind === "cbor" && document.roots[0]?.value?.kind === "object") {
      await viewer.evaluate((element) => element.selectNode(element.view.document.roots[0].id));
      await viewer.getByRole("combobox", { name: "Structure view" }).selectOption("table");
      const headings = await viewer.locator(".visualizer-table thead th").allTextContents();
      assert.equal(
        headings.filter((name) => name === "Value").length,
        1,
        "Map tables mixed a redundant container Value column with the actual Value field",
      );
      await viewer.getByRole("button", { name: "Close structure details" }).click();
    }
    if (index < 2 || index >= 4) {
      for (const [width, height] of [
        [1440, 900],
        [960, 720],
        [800, 600],
      ]) {
        await page.setViewportSize({ width, height });
        assert.equal(await viewer.locator(".visualizer-heading > strong").isVisible(), true);
        await page.screenshot({
          path: resolve(
            root,
            `../../../target/ui-check/binary-${kind}${ranges ? "-ranges" : ""}-${width}.png`,
          ),
        });
      }
    }
    await viewer.getByRole("button", { name: "About this visualizer", exact: true }).click();
    await viewer.getByRole("dialog", { name: "About this visualizer", exact: true }).waitFor();
    await page.keyboard.press("Escape");
    assert.equal(
      await page.evaluate(() => globalThis.__workspaceFixture.visualizerStarts.at(-1).extensionId),
      "net.transmog.binary",
    );
  }
}
