import assert from "node:assert/strict";

export async function checkUpdateOwnership(page) {
  await page.goto("https://workspace.test/");
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell")?.updates &&
      !document.querySelector("app-shell").updates.updateBusy,
  );
  await page.evaluate(async () => {
    const shell = document.querySelector("app-shell"),
      fixture = globalThis.__workspaceFixture;
    fixture.update = { ...fixture.update, phase: "available", version: "0.2.0", scheduled: false };
    shell.openAbout();
    await shell.updates.check(true);
    const original = window.__TAURI_INTERNALS__.invoke;
    fixture.restoreUpdateInvoke = () => {
      window.__TAURI_INTERNALS__.invoke = original;
    };
    window.__TAURI_INTERNALS__.invoke = async (command, args) => {
      if (fixture.holdUpdateCommand === command) {
        fixture.holdUpdateCommand = null;
        await new Promise((resolve) => {
          fixture.finishUpdateCommand = resolve;
        });
      }
      if (command === "cancel_update" && fixture.rejectUpdateCancel) {
        throw new Error("Fixture update cancellation unavailable");
      }
      if (command === "download_update") fixture.updateDownloadProgress = args.progress;
      const hold = fixture.holdUpdateReply === command;
      if (hold) fixture.holdUpdateReply = null;
      const result = await original(command, args);
      if (hold)
        await new Promise((resolve) => {
          fixture.finishUpdateReply = resolve;
        });
      return result;
    };
  });
  try {
    // Cancel remains available during a download, but a repeated click must not
    // release the first cancellation's busy state or send another cancellation.
    await page.evaluate(() => {
      const fixture = globalThis.__workspaceFixture;
      fixture.deferUpdateDownload = true;
      fixture.holdUpdateReply = "cancel_update";
      fixture.finishUpdateReply = null;
      void document.querySelector("app-shell").updates.choose(true);
    });
    await page.waitForFunction(() => globalThis.__workspaceFixture.updateDownloadPending);
    await page.getByRole("button", { name: "Cancel update", exact: true }).click();
    await page.waitForFunction(() => globalThis.__workspaceFixture.finishUpdateReply);
    const repeated = await page.evaluate(async () => {
      const updates = document.querySelector("app-shell").updates;
      const before = globalThis.__workspaceFixture.calls.cancel_update;
      await updates.cancel();
      return {
        calls: globalThis.__workspaceFixture.calls.cancel_update - before,
        busy: updates.updateBusy,
      };
    });
    assert.deepEqual(
      repeated,
      { calls: 0, busy: true },
      "Repeated cancellation released owned update work",
    );
    await page.evaluate(() => globalThis.__workspaceFixture.finishUpdateReply());
    await page.waitForFunction(() => !document.querySelector("app-shell").updates.updateBusy);

    // Failure to request cancellation leaves the original operation monitored,
    // and the cancellation attempt revokes automatic installation consent.
    const installsBefore = await page.evaluate(
      () => globalThis.__workspaceFixture.updateInstallCalls ?? 0,
    );
    await page.evaluate(() => {
      const fixture = globalThis.__workspaceFixture;
      fixture.updateDownloadPending = false;
      fixture.rejectUpdateCancel = true;
      void document.querySelector("app-shell").updates.choose(false);
    });
    await page.waitForFunction(() => globalThis.__workspaceFixture.updateDownloadPending);
    await page.getByRole("button", { name: "Cancel update", exact: true }).click();
    await page.getByText("Fixture update cancellation unavailable", { exact: true }).waitFor();
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").updates.updateBusy),
      true,
      "Cancellation failure released an active download",
    );
    await page.evaluate(() => {
      const fixture = globalThis.__workspaceFixture;
      fixture.updateDownloadProgress.onmessage({
        ...fixture.update,
        phase: "downloading",
        downloadedBytes: 64,
        totalBytes: 128,
      });
    });
    await page.getByText("50% downloaded", { exact: true }).waitFor();
    await page.evaluate(() => globalThis.__workspaceFixture.releaseUpdateDownload());
    await page.waitForFunction(() => !document.querySelector("app-shell").updates.updateBusy);
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").updates.updateState.phase),
      "ready",
    );
    assert.equal(
      await page.evaluate(() => globalThis.__workspaceFixture.updateInstallCalls ?? 0),
      installsBefore,
      "An attempted cancellation still installed automatically",
    );
    await page.evaluate(async () => {
      globalThis.__workspaceFixture.rejectUpdateCancel = false;
      await document.querySelector("app-shell").updates.cancel();
    });

    // Native cancellation may acknowledge before the original download reply.
    // Keep submission locked until both have settled.
    await page.evaluate(() => {
      const fixture = globalThis.__workspaceFixture;
      fixture.updateDownloadPending = false;
      fixture.holdUpdateReply = "download_update";
      fixture.finishUpdateReply = null;
      void document.querySelector("app-shell").updates.choose(true);
    });
    await page.waitForFunction(() => globalThis.__workspaceFixture.updateDownloadPending);
    await page.evaluate(() => document.querySelector("app-shell").updates.cancel());
    await page.waitForFunction(() => globalThis.__workspaceFixture.finishUpdateReply);
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").updates.updateBusy),
      true,
      "Cancellation stopped monitoring an unfinished native download reply",
    );
    await page.evaluate(() => globalThis.__workspaceFixture.finishUpdateReply());
    await page.waitForFunction(() => !document.querySelector("app-shell").updates.updateBusy);

    // A preference snapshot produced before downloading must not restore the
    // available phase over that newer download.
    await page.evaluate(() => {
      const fixture = globalThis.__workspaceFixture,
        updates = document.querySelector("app-shell").updates;
      fixture.holdUpdateReply = "set_automatic_update_checks";
      fixture.finishUpdateReply = null;
      void updates.setAutomaticChecks({ currentTarget: { checked: false } });
    });
    await page.waitForFunction(() => globalThis.__workspaceFixture.finishUpdateReply);
    await page.evaluate(() => {
      globalThis.__workspaceFixture.updateDownloadPending = false;
      void document.querySelector("app-shell").updates.choose(true);
    });
    await page.waitForFunction(() => globalThis.__workspaceFixture.updateDownloadPending);
    await page.evaluate(() => globalThis.__workspaceFixture.finishUpdateReply());
    await page.waitForFunction(
      () => !document.querySelector("app-shell").updates.updatePreferenceBusy,
    );
    assert.deepEqual(
      await page.evaluate(() => {
        const updates = document.querySelector("app-shell").updates;
        return {
          phase: updates.updateState.phase,
          automatic: updates.updateState.automaticChecksEnabled,
          busy: updates.updateBusy,
        };
      }),
      { phase: "downloading", automatic: false, busy: true },
      "A preference reply replaced newer download state",
    );
    await page.evaluate(() => {
      const fixture = globalThis.__workspaceFixture;
      fixture.updateDownloadProgress.onmessage({
        ...fixture.update,
        phase: "downloading",
        automaticChecksEnabled: true,
        downloadedBytes: 96,
        totalBytes: 128,
      });
    });
    await page.getByText("75% downloaded", { exact: true }).waitFor();
    assert.equal(
      await page.evaluate(
        () => document.querySelector("app-shell").updates.updateState.automaticChecksEnabled,
      ),
      false,
      "Old progress restored an obsolete automatic-check preference",
    );
    await page.evaluate(() => document.querySelector("app-shell").updates.cancel());

    // The preference command itself can also be pending when another command
    // snapshots the old value; completion must invalidate that older snapshot.
    await page.evaluate(async () => {
      const fixture = globalThis.__workspaceFixture,
        updates = document.querySelector("app-shell").updates;
      await updates.setAutomaticChecks({ currentTarget: { checked: true } });
      fixture.holdUpdateCommand = "set_automatic_update_checks";
      fixture.finishUpdateCommand = null;
      void updates.setAutomaticChecks({ currentTarget: { checked: false } });
    });
    await page.waitForFunction(() => globalThis.__workspaceFixture.finishUpdateCommand);
    await page.evaluate(() => {
      const fixture = globalThis.__workspaceFixture;
      fixture.deferUpdateDownload = false;
      fixture.holdUpdateReply = "download_update";
      fixture.finishUpdateReply = null;
      void document.querySelector("app-shell").updates.choose(true);
    });
    await page.waitForFunction(() => globalThis.__workspaceFixture.finishUpdateReply);
    await page.evaluate(() => globalThis.__workspaceFixture.finishUpdateCommand());
    await page.waitForFunction(
      () => !document.querySelector("app-shell").updates.updatePreferenceBusy,
    );
    await page.evaluate(() => globalThis.__workspaceFixture.finishUpdateReply());
    await page.waitForFunction(() => !document.querySelector("app-shell").updates.updateBusy);
    assert.equal(
      await page.evaluate(
        () => document.querySelector("app-shell").updates.updateState.automaticChecksEnabled,
      ),
      false,
      "A snapshot taken during a preference save overwrote its completion",
    );
    await page.evaluate(() => document.querySelector("app-shell").updates.cancel());

    // Conversely, a download reply captured before a preference edit must not
    // re-enable automatic checking after that edit succeeds.
    await page.evaluate(() => {
      const fixture = globalThis.__workspaceFixture,
        updates = document.querySelector("app-shell").updates;
      fixture.deferUpdateDownload = false;
      fixture.holdUpdateReply = "download_update";
      fixture.finishUpdateReply = null;
      fixture.update.automaticChecksEnabled = true;
      void updates.choose(true);
    });
    await page.waitForFunction(() => globalThis.__workspaceFixture.finishUpdateReply);
    await page.evaluate(() =>
      document
        .querySelector("app-shell")
        .updates.setAutomaticChecks({ currentTarget: { checked: false } }),
    );
    await page.evaluate(() => globalThis.__workspaceFixture.finishUpdateReply());
    await page.waitForFunction(() => !document.querySelector("app-shell").updates.updateBusy);
    assert.equal(
      await page.evaluate(
        () => document.querySelector("app-shell").updates.updateState.automaticChecksEnabled,
      ),
      false,
      "A download reply restored an obsolete preference",
    );
    await page.evaluate(() => document.querySelector("app-shell").updates.cancel());
  } finally {
    await page.evaluate(async () => {
      const fixture = globalThis.__workspaceFixture;
      fixture.finishUpdateCommand?.();
      fixture.finishUpdateReply?.();
      fixture.releaseUpdateDownload?.();
      fixture.restoreUpdateInvoke();
      fixture.deferUpdateDownload = false;
      await document
        .querySelector("app-shell")
        .updates.setAutomaticChecks({ currentTarget: { checked: true } });
    });
  }
}
