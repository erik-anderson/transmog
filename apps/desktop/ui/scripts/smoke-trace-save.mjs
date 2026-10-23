import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";

// Temporarily display a pending save in the isolated WebView2 profile. Backend
// progress and actual trace roundtrips have their own Rust regression checks.
export async function smokeTraceSave(client, screenshotPath) {
  const traffic = 'document.querySelector("app-shell").traffic';
  await client.evaluate(`(()=>{
    const workspace=${traffic};
    globalThis.__traceSaveProbe={workspace,active:workspace.getRootNode().activeElement,state:{savingTrace:workspace.savingTrace,saveTraceStatus:workspace.saveTraceStatus,saveTraceOpen:workspace.saveTraceOpen,saveTraceTotal:workspace.saveTraceTotal,saveTraceCompleted:workspace.saveTraceCompleted}};
    workspace.savingTrace=true;workspace.saveTraceTotal=4;workspace.saveTraceCompleted=2;workspace.saveTraceStatus='Creating SAZ archive · 2 of 4 entries…';workspace.showSaveTrace();workspace.$flushUpdates();
  })()`);
  try {
    for (const [name, width, height] of [
      ["wide", 1440, 900],
      ["small", 800, 600],
    ]) {
      await client.call("Emulation.setDeviceMetricsOverride", {
        width,
        height,
        deviceScaleFactor: 1,
        mobile: false,
      });
      const layout = await client.evaluate(`(()=>{
        const workspace=${traffic};workspace.$flushUpdates();
        const dialog=workspace.saveTraceDialog,progress=dialog.querySelector('progress'),close=dialog.querySelector('footer button:last-child');
        const bounds=dialog.getBoundingClientRect(),button=close.getBoundingClientRect();
        close.focus();
        return {value:progress.value,max:progress.max,visible:bounds.x>=0&&bounds.right<=innerWidth&&bounds.y>=0&&bounds.bottom<=innerHeight,reachable:button.bottom<=innerHeight&&workspace.getRootNode().elementFromPoint(button.x+button.width/2,button.y+button.height/2)===close,focus:workspace.getRootNode().activeElement===close};
      })()`);
      assert.deepEqual(
        layout,
        { value: 2, max: 4, visible: true, reachable: true, focus: true },
        "Native save progress or Close is unavailable",
      );
      const accessibility = await client.call("Accessibility.getFullAXTree");
      assert.ok(
        accessibility.nodes.some(
          (node) => node.role?.value === "progressbar" && node.name?.value === "Saving trace",
        ),
        "Native progress bar has no accessible name",
      );
      if (screenshotPath) {
        const screenshot = await client.call("Page.captureScreenshot", { format: "png" });
        await writeFile(
          screenshotPath.replace(".png", `-trace-save-${name}.png`),
          Buffer.from(screenshot.data, "base64"),
        );
      }
    }
    await client.evaluate(
      `void(globalThis.__traceSaveClosed=new Promise(resolve=>${traffic}.saveTraceDialog.addEventListener('close',()=>resolve(true),{once:true})))`,
    );
    await client.call("Input.dispatchKeyEvent", {
      type: "keyDown",
      key: "Escape",
      code: "Escape",
      windowsVirtualKeyCode: 27,
    });
    await client.call("Input.dispatchKeyEvent", {
      type: "keyUp",
      key: "Escape",
      code: "Escape",
      windowsVirtualKeyCode: 27,
    });
    await client.evaluate("globalThis.__traceSaveClosed");
    assert.equal(
      await client.evaluate(
        `(()=>{const workspace=${traffic};workspace.$flushUpdates();return !workspace.saveTraceDialog.open&&workspace.savingTrace&&!!workspace.querySelector('.trace-import-status progress[aria-label="Saving trace"]');})()`,
      ),
      true,
      "Escape lost background save progress",
    );
    return { progress: true, accessible: true, compact: true, background: true };
  } finally {
    await client.call("Emulation.clearDeviceMetricsOverride");
    await client.evaluate(
      `(()=>{const probe=globalThis.__traceSaveProbe;probe.workspace.saveTraceDialog.close();Object.assign(probe.workspace,probe.state);probe.workspace.$flushUpdates();probe.active?.focus({preventScroll:true});delete globalThis.__traceSaveProbe;delete globalThis.__traceSaveClosed;})()`,
    );
  }
}
