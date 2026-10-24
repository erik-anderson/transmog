import assert from "node:assert/strict";
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
