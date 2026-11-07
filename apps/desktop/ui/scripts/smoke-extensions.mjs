import { smokeVisualizers } from "./smoke-visualizers.mjs";
import { writeFile } from "node:fs/promises";

async function smokeGroupedDeclarations(call, evaluate, screenshotPath) {
  try {
    await evaluate(`(()=>{
    const settings=document.querySelector('app-shell').settings,editor=settings.extensionsEditor,root=editor.shadowRoot??editor;
    const scroller=(settings.shadowRoot??settings).querySelector('.settings-content');
    globalThis.__extensionDeclarationsProbe={details:[...root.querySelectorAll('details')].map(element=>({element,open:element.open})),scroller,scrollTop:scroller.scrollTop};
    for(const item of globalThis.__extensionDeclarationsProbe.details)if(item.element.open)item.element.querySelector('summary').click();
    const entry=[...root.querySelectorAll('article')].find(element=>element.getAttribute('aria-label')==='Binary formats');
    if(!entry)throw new Error('Bundled Binary formats were not loaded');
    entry.querySelector('.extension-declarations > summary').click();
    const rows=entry.querySelectorAll('.extension-contribution');
    if(rows.length!==5||!rows[0].textContent.includes('Request and response'))throw new Error('Native binary request and response viewers were not grouped');
    const inputs=[...rows[0].querySelectorAll('.extension-input-value')].map(element=>element.textContent);
    if(inputs.length!==4||inputs.some(value=>value.includes('No headers')||value.includes('No body access')))throw new Error('Native empty opposite-message declarations were not omitted');
  })()`);
    for (const [width, height] of [
      [1440, 900],
      [800, 600],
    ]) {
      await call("Emulation.setDeviceMetricsOverride", {
        width,
        height,
        deviceScaleFactor: 1,
        mobile: false,
      });
      for (const expanded of [false, true]) {
        const fits = await evaluate(`(()=>{
          const editor=document.querySelector('app-shell').settings.extensionsEditor,root=editor.shadowRoot??editor;
          const entry=[...root.querySelectorAll('article')].find(element=>element.getAttribute('aria-label')==='Binary formats');
          const details=entry.querySelector('.extension-input-details');
          if(details.open!==${expanded})details.querySelector('summary').click();
          (${expanded}?details:entry.querySelector('h3')).scrollIntoView({block:'start'});
          return editor.scrollWidth<=editor.clientWidth+1&&details.scrollWidth<=details.clientWidth+1;
        })()`);
        if (!fits) throw new Error("Native grouped declarations overflowed the settings pane");
        if (screenshotPath) {
          await writeFile(
            screenshotPath.replace(
              /\.png$/i,
              `-extensions-grouped-${expanded ? "details-" : ""}${width}.png`,
            ),
            Buffer.from(
              (
                await call("Page.captureScreenshot", {
                  format: "png",
                  captureBeyondViewport: false,
                })
              ).data,
              "base64",
            ),
          );
        }
      }
    }
  } finally {
    await call("Emulation.clearDeviceMetricsOverride");
    await evaluate(`(()=>{
      const saved=globalThis.__extensionDeclarationsProbe;if(!saved)return;
      for(const item of saved.details)if(item.element.open!==item.open)item.element.querySelector('summary').click();
      saved.scroller.scrollTop=saved.scrollTop;delete globalThis.__extensionDeclarationsProbe;
    })()`);
  }
}

// The native harness seeds this unpacked fixture in its owned profile. Its code
// throws if evaluated; managing declarations must never start that code.
export async function smokeExtensions(call, evaluate, screenshotPath, visualizerTracePath) {
  try {
    const result = await evaluate(`(async()=>{
      const shell=document.querySelector('app-shell'),settings=shell.settings;
      globalThis.__extensionProbe={view:shell.activeView,section:settings.settingsSection};
      shell.shadowRoot.querySelector('a[data-view="settings"]').click();settings.showSettingsSection('extensions');
      const editor=settings.extensionsEditor;
      const wait=async(condition)=>{const deadline=Date.now()+10000;while(!condition()){if(Date.now()>deadline)throw new Error('Extensions did not settle');await new Promise(resolve=>setTimeout(resolve,25));}};
      await wait(()=>editor.extensionsLoaded&&!editor.extensionsBusy);
      const original=await window.__TAURI_INTERNALS__.invoke('extension_status');
      const development=original.entries.find(entry=>entry.catalog.manifest.id==='test.development'),bundled=original.entries.find(entry=>entry.catalog.manifest.id==='net.transmog.mapi');
      if(original.warning||!development||!bundled||bundled.catalog.origin!=='bundled')throw new Error('Native extension fixture and bundled MAPI were not loaded');
      const root=editor.shadowRoot??editor,entry=[...root.querySelectorAll('article')].find(entry=>entry.getAttribute('aria-label')==='Development visualizer fixture');
      const bundledRow=[...root.querySelectorAll('article')].find(entry=>entry.getAttribute('aria-label')==='MAPI/HTTP');
      if(!bundledRow||[...bundledRow.querySelectorAll('button')].some(button=>!button.hidden&&/Uninstall|Package|Reload/.test(button.textContent)))throw new Error('Bundled MAPI exposes development or file-removal controls');
      const bundledDeclarations=bundledRow.querySelector('.extension-declarations');bundledDeclarations.querySelector('summary').click();
      const bodyRows=[...bundledDeclarations.querySelectorAll('.extension-contribution')];
      if(bodyRows.length!==2)throw new Error('MAPI request and response viewers were not grouped');
      const matching=[...bundledDeclarations.querySelectorAll('.extension-matches')];
      if(!matching.length||matching.some(details=>details.open))throw new Error('MAPI matching rules did not use compact disclosures');
      if(!matching[0].querySelector('summary').textContent.includes('Request and response')||matching[0].querySelectorAll('p').length!==23)throw new Error('MAPI matching conditions were not combined faithfully');
      const inputDetails=bodyRows[0].querySelector('.extension-input-details');
      if(inputDetails.open)throw new Error('Full declarations opened by default');
      inputDetails.querySelector('summary').click();
      if(!inputDetails.textContent.includes('Request viewer: Required')||!inputDetails.textContent.includes('Response viewer: Optional'))throw new Error('MAPI direction-specific body access was lost');
      if(!inputDetails.querySelectorAll('dd')[1].textContent.includes('on demand\\nResponse viewer: Optional'))throw new Error('Native direction-specific declarations lost their line boundary');
      if(inputDetails.querySelectorAll('button').length!==2)throw new Error('MAPI direction-specific debug actions were lost');
      inputDetails.querySelector('summary').click();
      bundledDeclarations.querySelector('summary').click();

      const packageButton=entry.querySelector('button[aria-label="Package Development visualizer fixture"]');
      if(!packageButton||packageButton.disabled)throw new Error('Unpacked extension has no available packaging action');
      entry.querySelector('button[aria-label="Reload active visualizers for Development visualizer fixture"]').click();
      await wait(()=>!editor.extensionsBusy);
      if(!editor.extensionsText.startsWith('No active visualizers'))throw new Error('Settings reload did not stay idle without active instances');
      entry.querySelector('summary').click();
      const columns=[...entry.querySelectorAll('summary')].find(summary=>summary.textContent==='Traffic columns and header rules');if(!columns)throw new Error('Static column declarations missing');columns.click();
      entry.querySelector('.extension-input-details > summary').click();
      if(!entry.textContent.includes('Synthetic protocol')||!entry.textContent.includes('request x-example: equals Native header evidence'))throw new Error('Static header rules were not disclosed');
      const enabled=entry.querySelector('input[type="checkbox"]');enabled.focus();enabled.click();
      await wait(()=>!editor.extensionsBusy&&!editor.extensionsEntries.find(entry=>entry.id==='test.development').enabled);
      const disabled=await window.__TAURI_INTERNALS__.invoke('extension_status');
      if(disabled.entries.find(entry=>entry.catalog.manifest.id==='test.development').catalog.enabled)throw new Error('Extension disable was not persisted');
      enabled.click();await wait(()=>!editor.extensionsBusy&&editor.extensionsEntries.find(entry=>entry.id==='test.development').enabled);
      if(!entry.querySelector('details').open)throw new Error('Changing extension enable state collapsed declarations');
      const reload=root.querySelector('input[aria-label="Hot reload Development visualizer fixture"]');reload.click();
      await wait(()=>!editor.extensionsBusy&&editor.extensionsEntries.find(entry=>entry.id==='test.development').hotReload);
      const status=await window.__TAURI_INTERNALS__.invoke('extension_status');
      if(!status.entries.find(entry=>entry.catalog.manifest.id==='test.development').hotReload)throw new Error('Developer hot reload was not persisted');
      if(editor.extensionsError)throw new Error(editor.extensionsError);
      return {entries:editor.extensionsEntries.length,headers:root.textContent.includes('x-example'),body:root.textContent.includes('No body access')};
    })()`);
    if (!result.headers || !result.body)
      throw new Error("Native declared-input details are incomplete");
    await smokeGroupedDeclarations(call, evaluate, screenshotPath);
    if (screenshotPath)
      await writeFile(
        screenshotPath.replace(/\.png$/i, "-extensions-wide.png"),
        Buffer.from(
          (await call("Page.captureScreenshot", { format: "png", captureBeyondViewport: false }))
            .data,
          "base64",
        ),
      );
    await call("Emulation.setDeviceMetricsOverride", {
      width: 800,
      height: 600,
      deviceScaleFactor: 1,
      mobile: false,
    });
    const layout = await evaluate(
      `(()=>{const editor=document.querySelector('app-shell').settings.extensionsEditor,root=editor.shadowRoot??editor;return {fits:editor.scrollWidth<=editor.clientWidth+1,button:Array.from(root.querySelectorAll('button')).some(button=>button.textContent==='Uninstall'&&!button.hidden&&!button.disabled)};})()`,
    );
    if (!layout.fits || !layout.button) throw new Error("Native Extensions controls are clipped");
    if (screenshotPath)
      await writeFile(
        screenshotPath.replace(/\.png$/i, "-extensions-compact.png"),
        Buffer.from(
          (await call("Page.captureScreenshot", { format: "png", captureBeyondViewport: false }))
            .data,
          "base64",
        ),
      );
    const native = visualizerTracePath
      ? await smokeVisualizers(call, evaluate, screenshotPath, visualizerTracePath)
      : null;
    await evaluate(
      `(async()=>{const editor=document.querySelector('app-shell').settings.extensionsEditor,root=editor.shadowRoot??editor;root.querySelector('button[aria-label="Uninstall Development visualizer fixture"]').click();const deadline=Date.now()+10000;while(editor.extensionsBusy){if(Date.now()>deadline)throw new Error('Native extension removal timed out');await new Promise(resolve=>setTimeout(resolve,25));}if(editor.extensionsError||editor.extensionsEntries.some(entry=>entry.id==='test.development')||!editor.extensionsEntries.some(entry=>entry.id==='net.transmog.mapi'))throw new Error('Native extension removal failed');})()`,
    );
    return { ...result, native, removed: true };
  } finally {
    await call("Emulation.clearDeviceMetricsOverride");
    await evaluate(
      `(()=>{const saved=globalThis.__extensionProbe;if(!saved)return;const shell=document.querySelector('app-shell');shell.settings.showSettingsSection(saved.section);shell.shadowRoot.querySelector('a[data-view="'+saved.view+'"]').click();delete globalThis.__extensionProbe;})()`,
    );
  }
}
