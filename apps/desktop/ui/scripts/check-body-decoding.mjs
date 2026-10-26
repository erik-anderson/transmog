import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { deflateRawSync } from "node:zlib";

export async function checkBodyDecoding(page) {
  await page.goto("https://workspace.test/");
  const decoded = [0, 0x41, 0xff, 0x20];
  const encoded = [...deflateRawSync(Uint8Array.from(decoded))];
  await page.evaluate(
    ({ decoded, encoded }) => {
      const state = globalThis.__workspaceFixture;
      state.hexBytes = encoded;
      state.bodyOverride = {
        mediaType: "application/octet-stream",
        contentCodings: ["deflate"],
        availability: "complete",
        observedBytes: encoded.length,
        retainedBytes: encoded.length,
      };
      const invoke = window.__TAURI_INTERNALS__.invoke;
      window.__TAURI_INTERNALS__.invoke = async (command, args) => {
        const result = await invoke(command, args);
        if (command !== "inspect_body" || !args.request.decodeContent) return result;
        return {
          ...result,
          decoded: true,
          displayBytes: decoded.length,
          bytesBase64: btoa(String.fromCharCode(...decoded)),
        };
      };
    },
    { decoded, encoded },
  );
  try {
    await page.locator('tr[data-session-id="first"]').click();
    const inspector = page.locator('traffic-workspace message-inspector[side="response"]');
    const viewer = inspector.getByLabel("Body viewer", { exact: true });
    const decode = inspector.getByLabel("Decode", { exact: true });
    const firstByte = inspector.locator('.hex-cell[data-byte="0"][data-column="hex"]');
    const decodedByte = decoded[0].toString(16).padStart(2, "0").toUpperCase();
    const encodedByte = encoded[0].toString(16).padStart(2, "0").toUpperCase();
    await firstByte.filter({ hasText: decodedByte }).waitFor();
    await inspector.locator(".body-options > summary").click();
    for (const keyboard of [false, true]) {
      assert.equal(await decode.isChecked(), true);
      if (keyboard) {
        await decode.focus();
        await decode.press("Space");
      } else await decode.uncheck();
      await firstByte.filter({ hasText: encodedByte }).waitFor();
      assert.equal(await viewer.inputValue(), "auto");
      assert.equal(
        await decode.isEnabled(),
        true,
        "Auto must allow decoding raw binary bytes again",
      );
      assert.equal(await decode.isChecked(), false);
      if (keyboard) {
        assert.equal(
          await decode.evaluate((element) => element.getRootNode().activeElement === element),
          true,
          "Loading original bytes lost checkbox focus",
        );
        await decode.press("Space");
      } else await decode.check();
      await firstByte.filter({ hasText: decodedByte }).waitFor();
      assert.equal(await decode.isChecked(), true);
    }
    // Explicit raw Hex and Metadata retain their existing decoding semantics.
    for (const representation of ["bytes", "metadata"]) {
      await viewer.selectOption(representation);
      assert.equal(await inspector.evaluate((element) => element.viewer), representation);
      assert.equal(await decode.isEnabled(), false);
      assert.equal(await decode.isChecked(), false);
    }
  } finally {
    await page.goto("https://workspace.test/");
  }
}

export async function checkXssiJson(page, root) {
  const viewport = page.viewportSize();
  await page.goto("https://workspace.test/");
  // Rust inspector tests cover parsing. These responses exercise the production
  // template's separation of formatted evidence, the notice and original bytes.
  const original = ')]}\'\n{"ok":true}';
  const formatted = '{\n  "ok": true\n}';
  const warning = "XSSI prefix omitted from this formatted view; the original body is unchanged.";
  await page.evaluate(
    ({ original, formatted, warning }) => {
      const bytes = [...new TextEncoder().encode(original)];
      globalThis.__workspaceFixture.hexBytes = bytes;
      globalThis.__workspaceFixture.bodyOverride = {
        mediaType: "application/json",
        contentCodings: [],
        availability: "complete",
        retainedBytes: bytes.length,
        observedBytes: bytes.length,
      };
      const invoke = window.__TAURI_INTERNALS__.invoke;
      window.__TAURI_INTERNALS__.invoke = async (command, args) => {
        const result = await invoke(command, args);
        if (command !== "inspect_body") return result;
        const json = ["auto", "formatted"].includes(args.request.representation);
        if (!json && args.request.representation !== "original-text") return result;
        return {
          ...result,
          representation: json ? "formatted-json" : "original-text",
          display: json ? formatted : original,
          displayBytes: bytes.length,
          warning: json ? warning : null,
        };
      };
    },
    { original, formatted, warning },
  );
  try {
    await page.locator('tr[data-session-id="first"]').click();
    const inspector = page.locator('traffic-workspace message-inspector[side="response"]');
    const viewer = inspector.getByLabel("Body viewer", { exact: true });
    const preview = inspector.locator(".body-preview");
    const notice = inspector.locator(".body-notice");
    await notice.getByText(warning, { exact: true }).waitFor();
    assert.equal(await preview.textContent(), formatted);
    await mkdir(resolve(root, "../../../target/ui-check"), { recursive: true });
    for (const [width, height] of [
      [1440, 900],
      [960, 720],
      [800, 600],
    ]) {
      await page.setViewportSize({ width, height });
      assert.equal(await viewer.isVisible(), true);
      assert.equal(await notice.isVisible(), true);
      await page.screenshot({
        path: resolve(root, `../../../target/ui-check/xssi-json-${width}.png`),
      });
    }
    await preview.focus();
    await preview.press("Shift+F10");
    await page.getByRole("menuitem", { name: "Copy body preview", exact: true }).click();
    // Windows normalizes clipboard line endings to CRLF.
    const copied = await page.evaluate(() => navigator.clipboard.readText());
    assert.equal(copied.replaceAll("\r\n", "\n"), formatted);
    await viewer.selectOption("original-text");
    await preview.getByText(original, { exact: true }).waitFor();
    assert.equal(await preview.textContent(), original);
    assert.equal(await notice.count(), 0);
    await viewer.selectOption("formatted");
    await notice.getByText(warning, { exact: true }).waitFor();
    assert.equal(await preview.textContent(), formatted);
    await viewer.focus();
    await viewer.press("Home");
    await viewer.press("Enter");
    await page.waitForFunction(
      () => document.querySelector("app-shell").traffic.responseMessage.viewer === "auto",
    );
    await notice.getByText(warning, { exact: true }).waitFor();
    await viewer.selectOption("bytes");
    await inspector.locator('.hex-cell[data-byte="0"][data-column="hex"]').waitFor();
    assert.equal(
      await inspector.locator('.hex-cell[data-byte="0"][data-column="hex"]').textContent(),
      "29",
    );
    assert.equal(await notice.count(), 0);
  } finally {
    await page.setViewportSize(viewport);
    await page.goto("https://workspace.test/");
  }
}
