import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { createContext, runInContext } from "node:vm";
import test from "node:test";
import { shippedCargoMetadata } from "./build-credits.mjs";
import { smokeBuildInfo } from "./smoke-build-info.mjs";

const repository = fileURLToPath(new URL("../../../../", import.meta.url));
const npm = JSON.parse(await readFile(new URL("../package-lock.json", import.meta.url), "utf8"));

function supportReport(target) {
  const { packages } = shippedCargoMetadata(repository, target);
  return {
    build: {
      identity: {
        target,
        profile: "release",
        rustc: "rustc fixture",
        sourceCommit: null,
        sourceDirty: null,
      },
      dependencies: [
        ...packages.map(({ name, version }) => ({ name, version })),
        ...["@microsoft/webui-framework", "@tauri-apps/api"].map((name) => ({
          name,
          version: npm.packages["node_modules/" + name].version,
        })),
      ],
      bundledSources: [
        "BoringSSL",
        "Chromium certificate policy",
        "Chromium root store",
        "Chromium Public Suffix List",
      ].map((name) => ({ name })),
    },
    runtime: { webviewVersion: "fixture" },
  };
}

async function runSmoke(report) {
  const details = { open: false, querySelector: () => text };
  const text = { textContent: JSON.stringify(report), scrollIntoView() {}, focus() {} };
  let focusRestored = false;
  const settings = {
    ready: Promise.resolve(),
    settingsSection: "general",
    supportDetails: JSON.stringify(report),
    showSettingsSection(section) {
      this.settingsSection = section;
    },
    async refreshDiagnostics() {},
    async $whenRendered() {},
    querySelector: () => ({ nextElementSibling: details }),
  };
  const shell = {
    settings,
    activeView: "traffic",
    shadowRoot: {
      activeElement: {
        isConnected: true,
        focus() {
          focusRestored = true;
        },
      },
      querySelector: (selector) => ({
        click() {
          shell.activeView = /data-view="([^"]+)"/.exec(selector)[1];
        },
      }),
    },
  };
  const context = createContext({ document: { querySelector: () => shell } });
  try {
    return await smokeBuildInfo(undefined, (source) => runInContext(source, context));
  } finally {
    assert.equal(shell.activeView, "traffic");
    assert.equal(settings.settingsSection, "general");
    assert.equal(details.open, false);
    assert.equal(focusRestored, true);
    assert.equal(context.__buildInfoProbe, undefined);
  }
}

for (const target of ["x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc"]) {
  test(`${target} smoke accepts the shipped WebUI runtime and rejects compiler substitution`, async () => {
    const report = supportReport(target);
    assert.ok(!report.build.dependencies.some(({ name }) => name === "microsoft-webui"));
    assert.equal((await runSmoke(report)).identity.target, target);
    for (const name of [
      "tauri",
      "microsoft-webui-handler",
      "@microsoft/webui-framework",
      "@tauri-apps/api",
      "boring",
      "v8",
      "swc_ecma_parser",
    ]) {
      const missing = structuredClone(report);
      missing.build.dependencies = missing.build.dependencies.filter((item) => item.name !== name);
      if (name === "microsoft-webui-handler")
        missing.build.dependencies.push({ name: "microsoft-webui", version: "3.0.4" });
      await assert.rejects(runSmoke(missing), { message: "Missing built dependency " + name });
      const unversioned = structuredClone(report);
      unversioned.build.dependencies.find((item) => item.name === name).version = "";
      await assert.rejects(runSmoke(unversioned), { message: "Missing built dependency " + name });
    }
  });
}
