import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";

export async function smokeScriptHelp(call, evaluate, screenshotPath) {
  const editor = "document.querySelector('app-shell').automation.querySelector('script-editor')";
  const wait = async (expression) => {
    const deadline = Date.now() + 20000;
    while (!(await evaluate(expression))) {
      if (Date.now() > deadline) throw new Error("Native script help timed out: " + expression);
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
  };
  const prior = await evaluate(
    `(()=>{const shell=document.querySelector('app-shell');return {view:shell.activeView,section:shell.automation.automationSection};})()`,
  );
  try {
    await evaluate(
      `(async()=>{const shell=document.querySelector('app-shell');shell.shadowRoot.querySelector('a[data-view="automation"]').click();await shell.automation.showAutomationSection('scripts');shell.automation.$flushUpdates();})()`,
    );
    await wait(`Boolean(${editor}?.scriptReady)`);
    const search = await evaluate(
      `(()=>{const editor=${editor};editor.scriptHelpButton.click();const search=(value)=>{editor.scriptHelpSearch.value=value;editor.scriptHelpSearch.dispatchEvent(new Event('input',{bubbles:true}));editor.$flushUpdates();};search('Complete examples');editor.scriptHelpContent.scrollTop=editor.scriptHelpContent.scrollHeight;const scrolled=editor.scriptHelpContent.scrollTop>0;search('request body');return {scrolled,top:editor.scriptHelpContent.scrollTop,content:editor.scriptHelpContent.textContent};})()`,
    );
    assert.equal(search.scrolled, true);
    assert.equal(search.top, 0);
    assert.match(search.content, /Filters, buffering and limits/);
    const contract = await evaluate(
      `(()=>{const editor=${editor};editor.scriptHelpButton.click();editor.scriptHelpSearch.value='onFailed';editor.scriptHelpSearch.dispatchEvent(new Event('input',{bubbles:true}));editor.$flushUpdates();return {revision:editor.apiRevision,failure:editor.scriptTestFailure.value,scope:editor.draftScope,modal:editor.scriptHelpDialog.matches(':modal'),content:editor.scriptHelpDialog.textContent};})()`,
    );
    assert.equal(contract.revision, 2);
    assert.equal(contract.failure, "Upstream");
    assert.equal(contract.modal, true);
    assert.match(contract.content, /onFailed/);
    if (screenshotPath)
      await writeFile(
        screenshotPath.replace(/\.png$/i, "-script-help.png"),
        Buffer.from(
          (await call("Page.captureScreenshot", { format: "png", captureBeyondViewport: false }))
            .data,
          "base64",
        ),
      );
    for (const type of ["keyDown", "keyUp"])
      await call("Input.dispatchKeyEvent", {
        type,
        key: "Escape",
        code: "Escape",
        windowsVirtualKeyCode: 27,
      });
    await wait(`!${editor}.scriptHelpDialog.open && ${editor}.scriptHelpButton.matches(':focus')`);
    return {
      revision: contract.revision,
      scope: contract.scope,
      search: true,
      searchScrollReset: true,
      keyboardClose: true,
    };
  } finally {
    await evaluate(
      `(async()=>{const shell=document.querySelector('app-shell');shell.automation.querySelector('script-editor')?.closeScriptHelp?.();await shell.automation.showAutomationSection(${JSON.stringify(prior.section)});shell.shadowRoot.querySelector('a[data-view="'+${JSON.stringify(prior.view)}+'"]').click();})()`,
    );
  }
}
