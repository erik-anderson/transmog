import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";

/** Exercise the compiled authoring controls and actual loopback replay in WebView2. */
export async function smokeComposerAuthoring(viewer, composer, url, screenshot) {
  const text = '{"message":"café"}\n';
  const headers = "Content-Type: application/json\nX-Repeat: first\nx-repeat: second\nX-Empty: ";
  await viewer.evaluate(`(() => {
    const workspace = ${composer};
    workspace.showComposerPane('request');
    const field = (name, value) => {
      const input = workspace.composerForm.elements.namedItem(name);
      input.value = value;
      input.dispatchEvent(new Event('input', { bubbles: true }));
    };
    field('method', 'POST');
    field('url', ${JSON.stringify(url)});
    workspace.headerEditor.value = ${JSON.stringify(headers)};
    workspace.headerEditor.toggleMode();
    if (workspace.headerEditor.headerEditorMode === 'text') workspace.headerEditor.toggleMode();
    workspace.useEmptyBody();
    const view = workspace.composerForm.elements.namedItem('bodyView');
    view.value = 'text'; view.dispatchEvent(new Event('change', { bubbles: true }));
    field('body', ${JSON.stringify(text)});
    view.value = 'hex'; view.dispatchEvent(new Event('change', { bubbles: true }));
    view.value = 'text'; view.dispatchEvent(new Event('change', { bubbles: true }));
    workspace.composerForm.elements.namedItem('nonIdempotent').checked = true;
  })()`);
  assert.equal(await viewer.evaluate(`${composer}.input('body').value`), text);
  assert.equal(await viewer.evaluate(`${composer}.composerBodyError`), "");
  assert.deepEqual(await viewer.evaluate(`${composer}.headerEditor.getHeaders().slice(1)`), [
    { name: "X-Repeat", value: "first" },
    { name: "x-repeat", value: "second" },
    { name: "X-Empty", value: "" },
  ]);
  await viewer.evaluate(`(async () => {
    const workspace = ${composer};
    await workspace.executeComposer(new Event('submit', { cancelable: true }));
  })()`);
  const result = await viewer.evaluate(`({
    error: ${composer}.composerError,
    status: ${composer}.composerResult?.status,
    body: ${composer}.composerResult?.body,
    matches: ${composer}.resultMatchesDraft
  })`);
  assert.equal(result.error, "");
  assert.equal(result.status, 200);
  const echoed = JSON.parse(result.body);
  assert.deepEqual(echoed.bytes, Array.from(Buffer.from(text)));
  assert.deepEqual(echoed.repeated, ["first", "second"]);
  assert.equal(echoed.empty, "");
  assert.equal(result.matches, true);
  await viewer.evaluate(`(() => {
    const workspace = ${composer};
    workspace.showComposerPane('request');
    const value = workspace.headerEditor.querySelector('textarea[aria-label="Header value 2"]');
    value.value = 'edited'; value.dispatchEvent(new Event('input', { bubbles: true }));
  })()`);
  assert.equal(await viewer.evaluate(`${composer}.resultMatchesDraft`), false);
  for (const [width, height] of [
    [1440, 900],
    [960, 720],
    [800, 600],
  ]) {
    await viewer.call("Emulation.setDeviceMetricsOverride", {
      width,
      height,
      deviceScaleFactor: 1,
      mobile: false,
    });
    const bounds = await viewer.evaluate(`(() => {
      const workspace = ${composer};
      const form = workspace.composerForm;
      form.scrollTop = 0;
      const send = form.querySelector('button[type="submit"]');
      const rect = send.getBoundingClientRect();
      return { overflow: form.scrollWidth > form.clientWidth + 2, top: rect.top, bottom: rect.bottom, width: rect.width, height: rect.height, viewport: innerHeight };
    })()`);
    assert.equal(bounds.overflow, false, `Composer overflows horizontally at ${width}px`);
    assert.ok(
      bounds.top >= 0 && bounds.bottom <= bounds.viewport && bounds.width > 0 && bounds.height > 0,
      `Send is unreachable at ${width}px`,
    );
    if (screenshot) {
      const image = await viewer.call("Page.captureScreenshot", { format: "png" });
      await writeFile(
        screenshot.replace(".png", `-authoring-${width}.png`),
        Buffer.from(image.data, "base64"),
      );
    }
  }
  await viewer.call("Emulation.clearDeviceMetricsOverride");
  process.stdout.write(
    "Native header rows, duplicate values, Text/Hex replay and compact authoring verified.\n",
  );
}
