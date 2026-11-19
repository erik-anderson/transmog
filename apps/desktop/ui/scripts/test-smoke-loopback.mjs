import assert from "node:assert/strict";
import test from "node:test";
import { runInNewContext } from "node:vm";
import { smokeLoopback } from "./smoke-loopback.mjs";

async function observe({ completeAfter = 13_000, startAfter = 0, error = "" } = {}) {
  let now = 0;
  let modal = false;
  const filter = {
    checked: false,
    click() {
      this.checked = !this.checked;
    },
  };
  const manager = {
    loopbackBusy: false,
    loopbackCurrent: false,
    loopbackError: "",
    loopbackRows: [],
    loopbackSelected: 0,
    loopbackAddCount: 0,
    loopbackRemoveCount: 0,
    get loopbackSelectText() {
      return filter.checked ? "Select filtered" : "Select all";
    },
    exemptionDialog: {
      matches: () => modal,
      textContent:
        "Display Name Description Package AppContainer name AppContainer SID AppContainer user(s)",
    },
    selectMatching() {},
    clearSelection() {},
    async $whenRendered() {},
    querySelector: () => filter,
    querySelectorAll: () => [
      { textContent: "Add selected exemptions (0)", disabled: true },
      { textContent: "Remove selected exemptions (0)", disabled: true },
    ],
    close() {
      modal = false;
    },
  };
  const launch = {
    textContent: "Manage loopback exemptions…",
    focus() {},
    click() {
      modal = startAfter === 0;
      manager.loopbackBusy = startAfter === 0;
    },
  };
  const settings = {
    loopbackManager: manager,
    listenerOptions: { open: false },
    settingsSection: "original",
    async $whenRendered() {},
    showSettingsSection(value) {
      this.settingsSection = value;
    },
    querySelectorAll: () => [launch],
  };
  const shell = {
    settings,
    activeView: "traffic",
    shadowRoot: { querySelector: () => ({ click() {} }) },
  };
  const context = {
    document: { querySelector: () => shell },
    Date: { now: () => now },
    performance: { now: () => now },
    setTimeout(resolve, milliseconds) {
      now += milliseconds;
      if (now >= startAfter) {
        modal = true;
        manager.loopbackBusy = true;
      }
      if (now >= completeAfter) {
        manager.loopbackBusy = false;
        manager.loopbackCurrent = !error;
        manager.loopbackError = error;
      }
      resolve();
    },
  };
  let failure;
  let result;
  try {
    result = await smokeLoopback(undefined, (expression) => runInNewContext(expression, context));
  } catch (caught) {
    failure = caught;
  }
  assert.equal(modal, false, "The observation did not close its modal");
  assert.equal(settings.settingsSection, "original", "The observation did not restore the section");
  assert.equal(settings.listenerOptions.open, false);
  return { result, failure, elapsed: now };
}

test("native inventory may finish after the old twelve-second observation window", async () => {
  const result = await observe();
  assert.equal(result.failure, undefined, String(result.failure));
  assert.equal(result.result.enumerated, 0);
  assert.equal(result.result.readOnly, true);
  assert.ok(result.elapsed >= 13_000);
});

test("native enumeration errors still fail and restore the surface", async () => {
  const result = await observe({ completeAfter: 100, error: "Firewall unavailable" });
  assert.match(String(result.failure), /Firewall unavailable/);
});

test("deferred click delivery is observed before requiring current inventory", async () => {
  const result = await observe({ startAfter: 100, completeAfter: 200 });
  assert.equal(result.failure, undefined, String(result.failure));
  assert.equal(result.elapsed, 200);
});

test("an enumeration that never completes fails within the declared finite budget", async () => {
  const result = await observe({ completeAfter: Infinity });
  assert.ok(result.failure);
  assert.ok(result.elapsed >= 45_000 && result.elapsed <= 60_100, String(result.elapsed));
  assert.match(String(result.failure), /"busy":true/);
});
