import {smokeVisualizers} from './smoke-visualizers.mjs';
import {writeFile} from 'node:fs/promises';

// The native harness seeds this unpacked fixture in its owned profile. Its code
// throws if evaluated; managing declarations must never start that code.
export async function smokeExtensions(call,evaluate,screenshotPath,visualizerTracePath) {
  try {
    const result=await evaluate(`(async()=>{
      const shell=document.querySelector('app-shell'),settings=shell.settings;
      globalThis.__extensionProbe={view:shell.activeView,section:settings.settingsSection};
      shell.shadowRoot.querySelector('a[data-view="settings"]').click();settings.showSettingsSection('extensions');
      const editor=settings.extensionsEditor;
      const wait=async(condition)=>{const deadline=Date.now()+10000;while(!condition()){if(Date.now()>deadline)throw new Error('Extensions did not settle');await new Promise(resolve=>setTimeout(resolve,25));}};
      await wait(()=>editor.extensionsLoaded&&!editor.extensionsBusy);
      const original=await window.__TAURI_INTERNALS__.invoke('extension_status');
      const development=original.entries.find(entry=>entry.catalog.manifest.id==='test.development'),bundled=original.entries.find(entry=>entry.catalog.manifest.id==='com.transmog.mapi');
      if(original.warning||!development||!bundled||bundled.catalog.origin!=='bundled')throw new Error('Native extension fixture and bundled MAPI were not loaded');
      const root=editor.shadowRoot??editor,entry=[...root.querySelectorAll('article')].find(entry=>entry.getAttribute('aria-label')==='Development visualizer fixture');
      const bundledRow=[...root.querySelectorAll('article')].find(entry=>entry.getAttribute('aria-label')==='MAPI/HTTP');
      if(!bundledRow||[...bundledRow.querySelectorAll('button')].some(button=>!button.hidden&&/Uninstall|Package|Reload/.test(button.textContent)))throw new Error('Bundled MAPI exposes development or file-removal controls');
      const bundledDeclarations=bundledRow.querySelector('.extension-declarations');bundledDeclarations.querySelector('summary').click();
      const matching=[...bundledDeclarations.querySelectorAll('.extension-matches')];
      if(!matching.length||matching.some(details=>details.open))throw new Error('MAPI matching rules did not use compact disclosures');
      bundledDeclarations.querySelector('summary').click();

      const packageButton=entry.querySelector('button[aria-label="Package Development visualizer fixture"]');
      if(!packageButton||packageButton.disabled)throw new Error('Unpacked extension has no available packaging action');
      entry.querySelector('button[aria-label="Reload active visualizers for Development visualizer fixture"]').click();
      await wait(()=>!editor.extensionsBusy);
      if(!editor.extensionsText.startsWith('No active visualizers'))throw new Error('Settings reload did not stay idle without active instances');
      entry.querySelector('summary').click();
      const columns=[...entry.querySelectorAll('summary')].find(summary=>summary.textContent==='Traffic columns and header rules');if(!columns)throw new Error('Static column declarations missing');columns.click();
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
    if(!result.headers||!result.body)throw new Error('Native declared-input details are incomplete');
    if(screenshotPath)await writeFile(screenshotPath.replace(/\.png$/i,'-extensions-wide.png'),Buffer.from((await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:false})).data,'base64'));
    await call('Emulation.setDeviceMetricsOverride',{width:800,height:600,deviceScaleFactor:1,mobile:false});
    const layout=await evaluate(`(()=>{const editor=document.querySelector('app-shell').settings.extensionsEditor,root=editor.shadowRoot??editor;return {fits:editor.scrollWidth<=editor.clientWidth+1,button:Array.from(root.querySelectorAll('button')).some(button=>button.textContent==='Uninstall'&&!button.hidden&&!button.disabled)};})()`);
    if(!layout.fits||!layout.button)throw new Error('Native Extensions controls are clipped');
    if(screenshotPath)await writeFile(screenshotPath.replace(/\.png$/i,'-extensions-compact.png'),Buffer.from((await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:false})).data,'base64'));
    const native=visualizerTracePath?await smokeVisualizers(call,evaluate,screenshotPath,visualizerTracePath):null;
    await evaluate(`(async()=>{const editor=document.querySelector('app-shell').settings.extensionsEditor,root=editor.shadowRoot??editor;root.querySelector('button[aria-label="Uninstall Development visualizer fixture"]').click();const deadline=Date.now()+10000;while(editor.extensionsBusy){if(Date.now()>deadline)throw new Error('Native extension removal timed out');await new Promise(resolve=>setTimeout(resolve,25));}if(editor.extensionsError||editor.extensionsEntries.some(entry=>entry.id==='test.development')||!editor.extensionsEntries.some(entry=>entry.id==='com.transmog.mapi'))throw new Error('Native extension removal failed');})()`);
    return {...result,native,removed:true};
  } finally {
    await call('Emulation.clearDeviceMetricsOverride');
    await evaluate(`(()=>{const saved=globalThis.__extensionProbe;if(!saved)return;const shell=document.querySelector('app-shell');shell.settings.showSettingsSection(saved.section);shell.shadowRoot.querySelector('a[data-view="'+saved.view+'"]').click();delete globalThis.__extensionProbe;})()`);
  }
}
