import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { execFile, spawn } from "node:child_process";
import { setTimeout as delay } from "node:timers/promises";
import { promisify } from "node:util";
import { resolve } from "node:path";
const { chromium } = createRequire(
  new URL("../../../../e2e/playwright/package.json", import.meta.url),
)("playwright");
const argument = (name) => process.argv[process.argv.indexOf(name) + 1];
const browser = await chromium.connectOverCDP(`http://127.0.0.1:${argument("--port")}`);
const context = browser.contexts()[0],
  main = context.pages()[0];
const host = main.locator("app-shell converter-host");
const windows = [];
let restarted, secondBrowser;
try {
  const jwt = `${Buffer.from('{"typ":"JWT","alg":"HS256"}').toString("base64url")}.${Buffer.from('{"sub":"native-user","exp":9007199254740993.125}').toString("base64url")}.c2ln`;
  for (let index = 0; index < 4; index++) {
    await host.getByRole("button", { name: "Format converter", exact: true }).click();
    await host
      .getByRole("textbox", { name: "Converter input", exact: true })
      .fill(index === 0 ? `Bearer ${jwt}` : `native ${index}`);
    await host
      .getByLabel("Conversion", { exact: true })
      .selectOption(index === 0 ? "jwt-decode" : "base64-encode");
    const created = context.waitForEvent("page");
    await host.getByRole("button", { name: "Open in new window", exact: true }).click();
    const page = await created;
    windows.push(page);
    await page.getByRole("textbox", { name: "Converter input", exact: true }).waitFor();
    await page.waitForFunction(() => document.querySelector("converter-host")?.converterReady);
    assert.equal(
      await page.getByRole("textbox", { name: "Converter input", exact: true }).inputValue(),
      index === 0 ? `Bearer ${jwt}` : `native ${index}`,
    );
    const output = await page
      .getByRole("textbox", { name: "Converter output", exact: true })
      .inputValue();
    if (index === 0) {
      assert.equal(JSON.parse(output).payload.sub, "native-user");
      assert.equal(JSON.parse(output).verification, "Not checked");
      assert.ok(output.includes("9007199254740993.125"));
      await page.screenshot({ path: resolve(argument("--output"), "converter-jwt-native.png") });
      const input = page.getByRole("textbox", { name: "Converter input", exact: true });
      const ntlmBytes = Buffer.alloc(40);
      ntlmBytes.write("NTLMSSP\0", "ascii");
      ntlmBytes.writeUInt32LE(1, 8);
      const ntlm = ntlmBytes.toString("base64");
      for (const [operation, text, format] of [
        ["auth-decode", "Authorization: Basic dXNlcjpwYXNz", "Basic"],
        ["basic-decode", "dXNlcjpwYXNzOm1vcmU=", "Basic"],
        ["digest-parse", 'Authentication-Info: rspauth="secret", qop=auth', "Digest"],
        ["jws-decode", "eyJhbGciOiJub25lIn0._w.", "JWS"],
        ["jwe-inspect", "eyJhbGciOiJkaXIiLCJlbmMiOiJBMTI4R0NNIn0..aXY.Y2lwaGVy.dGFn", "JWE"],
        ["ntlm-decode", ntlm, "NTLM"],
        ["negotiate-decode", `Negotiate ${ntlm}`, "Negotiate"],
      ]) {
        await page.getByLabel("Conversion", { exact: true }).selectOption(operation);
        await input.fill(text);
        await page.waitForFunction(() => document.querySelector("converter-host").converterReady);
        const result = JSON.parse(
          await page.getByRole("textbox", { name: "Converter output", exact: true }).inputValue(),
        );
        assert.equal(result.format, format);
        if (operation === "jwe-inspect") assert.equal(result.status, "encrypted");
        if (operation === "jws-decode") assert.equal(result.decoded.payloadBytesBase64, "/w==");
      }
      await page.screenshot({
        path: resolve(argument("--output"), "converter-authentication-native.png"),
      });
      await page.getByLabel("Conversion", { exact: true }).selectOption("jwt-decode");
      await input.fill("incomplete.jwt");
      await page
        .getByRole("alert")
        .filter({ hasText: "Expected a complete compact JWT" })
        .waitFor();
      assert.equal(await input.inputValue(), "incomplete.jwt");
      assert.equal(
        await page.getByRole("button", { name: "Copy output", exact: true }).isEnabled(),
        false,
      );
      await input.fill(jwt);
      await page.waitForFunction(() => document.querySelector("converter-host").converterReady);
      const decodedInput = await page
        .getByRole("textbox", { name: "Converter output", exact: true })
        .inputValue();
      await page.getByRole("button", { name: "Use output as input", exact: true }).click();
      assert.equal(await page.getByLabel("Conversion", { exact: true }).inputValue(), "");
      assert.equal(await input.inputValue(), decodedInput);
      assert.equal(
        await page.getByRole("textbox", { name: "Converter output", exact: true }).inputValue(),
        "",
      );
      const pausedCreated = context.waitForEvent("page");
      await page.getByRole("button", { name: "Open in new window", exact: true }).click();
      const paused = await pausedCreated;
      await paused.getByRole("textbox", { name: "Converter input", exact: true }).waitFor();
      await paused.waitForFunction((expected) => {
        const converter = document.querySelector("converter-host");
        return (
          converter?.converterInput === expected &&
          converter.converterStatus === "Choose a conversion."
        );
      }, decodedInput);
      assert.equal(await paused.getByLabel("Conversion", { exact: true }).inputValue(), "");
      assert.equal(
        await paused.getByRole("textbox", { name: "Converter input", exact: true }).inputValue(),
        decodedInput,
      );
      assert.equal(
        await paused.getByRole("textbox", { name: "Converter output", exact: true }).inputValue(),
        "",
      );
      const pausedClosed = paused.waitForEvent("close");
      await paused
        .evaluate(() => window.__TAURI_INTERNALS__.invoke("close_converter_window"))
        .catch(() => undefined);
      await pausedClosed;
      await input.press("Control+z");
      assert.equal(await input.inputValue(), jwt);
      assert.equal(await page.getByLabel("Conversion", { exact: true }).inputValue(), "jwt-decode");
      await page.waitForFunction(() => document.querySelector("converter-host").converterReady);
    } else {
      assert.equal(output, Buffer.from(`native ${index}`).toString("base64"));
    }
    for (const command of [
      "start_proxy",
      "query_sessions",
      "change_response_transform",
      "record_frontend_diagnostic",
    ]) {
      assert.equal(
        await page.evaluate(async (command) => {
          try {
            await window.__TAURI_INTERNALS__.invoke(command, {});
            return false;
          } catch {
            return true;
          }
        }, command),
        true,
        `${command} was allowed in a converter window`,
      );
    }
    if (index === 0) {
      await main.bringToFront();
      assert.equal(await main.evaluate(() => document.hasFocus()), true);
      assert.equal(
        await page.evaluate(async () => {
          try {
            await window.__TAURI_INTERNALS__.invoke("acknowledge_converter_window");
            return false;
          } catch {
            return true;
          }
        }),
        true,
        "A completed handoff must not be acknowledged again",
      );
      assert.equal(
        await main.evaluate(() => document.hasFocus()),
        true,
        "A repeated handoff must not steal focus",
      );
    }
  }
  await windows[0].getByLabel("Conversion", { exact: true }).selectOption("base64-encode");
  await windows[0]
    .getByRole("textbox", { name: "Converter input", exact: true })
    .fill("independent");
  await windows[0].waitForFunction(
    () => document.querySelector("converter-host").converterOutput === "aW5kZXBlbmRlbnQ=",
  );
  assert.equal(
    await windows[1].getByRole("textbox", { name: "Converter input", exact: true }).inputValue(),
    "native 1",
  );
  const closed = windows[1].waitForEvent("close");
  await windows[1]
    .evaluate(() => window.__TAURI_INTERNALS__.invoke("close_converter_window"))
    .catch(() => undefined);
  await closed;
  await main.screenshot({ path: resolve(argument("--output"), "converter-main.png") });
  await windows[0].screenshot({ path: resolve(argument("--output"), "converter-native.png") });
  const originClosed = main.waitForEvent("close");
  await promisify(execFile)(
    "pwsh.exe",
    [
      "-NoProfile",
      "-NonInteractive",
      "-File",
      argument("--close-helper"),
      "-ProbeProcessId",
      argument("--process-id"),
      "-WindowTitle",
      "Transmog",
    ],
    { windowsHide: true },
  );
  await originClosed;
  const survivor = windows[0];
  await survivor
    .getByRole("textbox", { name: "Converter input", exact: true })
    .fill("after origin close");
  await survivor.waitForFunction(
    () => document.querySelector("converter-host").converterOutput === "YWZ0ZXIgb3JpZ2luIGNsb3Nl",
  );
  const created = context.waitForEvent("page");
  await survivor.getByRole("button", { name: "Open in new window", exact: true }).click();
  const another = await created;
  windows.push(another);
  await another.waitForFunction(() => document.querySelector("converter-host")?.converterReady);
  assert.equal(
    await another.getByRole("textbox", { name: "Converter input", exact: true }).inputValue(),
    "after origin close",
  );
  for (const page of windows.filter((page) => !page.isClosed()))
    await page
      .evaluate(() => window.__TAURI_INTERNALS__.invoke("close_converter_window"))
      .catch(() => undefined);
  await browser.close();
  for (let attempt = 0; attempt < 50; attempt++) {
    try {
      process.kill(Number(argument("--process-id")), 0);
    } catch {
      break;
    }
    await delay(100);
  }
  restarted = spawn(argument("--executable"), [], { windowsHide: true, stdio: "ignore" });
  for (let attempt = 0; attempt < 100; attempt++) {
    try {
      const response = await fetch(`http://127.0.0.1:${argument("--port")}/json/version`);
      if (response.ok) break;
    } catch {}
    if (restarted.exitCode !== null)
      throw new Error("Restarted converter probe exited during startup.");
    await delay(200);
  }
  secondBrowser = await chromium.connectOverCDP(`http://127.0.0.1:${argument("--port")}`);
  const fresh = secondBrowser.contexts()[0].pages()[0];
  await fresh.waitForFunction(() => {
    const shell = document.querySelector("app-shell");
    return (
      shell?.traffic?.isConnected &&
      shell.shadowRoot.querySelector("converter-host")?.trigger?.isConnected
    );
  });
  await fresh.getByRole("button", { name: "Format converter", exact: true }).click();
  assert.equal(
    await fresh.getByRole("textbox", { name: "Converter input", exact: true }).inputValue(),
    "",
  );
  assert.equal(
    await fresh.getByRole("textbox", { name: "Converter output", exact: true }).inputValue(),
    "",
  );
  assert.equal(await fresh.getByLabel("Conversion", { exact: true }).inputValue(), "");
  const freshClosed = fresh.waitForEvent("close");
  await promisify(execFile)(
    "pwsh.exe",
    [
      "-NoProfile",
      "-NonInteractive",
      "-File",
      argument("--close-helper"),
      "-ProbeProcessId",
      String(restarted.pid),
      "-WindowTitle",
      "Transmog",
    ],
    { windowsHide: true },
  );
  await freshClosed;
  process.stdout.write(
    "Native converters preserve independent drafts, survive the origin, create more windows, deny unrelated commands, and clear contents after restart.\n",
  );
} catch (error) {
  const live =
    secondBrowser?.contexts()[0]?.pages()[0] ?? windows.find((page) => !page.isClosed()) ?? main;
  await live
    .screenshot({ path: resolve(argument("--output"), "converter-native-failure.png") })
    .catch(() => undefined);
  throw error;
} finally {
  await secondBrowser?.close();
  await browser.close();
  if (restarted && restarted.exitCode === null) restarted.kill();
}
