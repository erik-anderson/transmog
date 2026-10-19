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
      if(original.warning||original.entries.length!==1||original.entries[0].catalog.manifest.id!=='test.development')throw new Error('Native extension fixture was not loaded');
      const root=editor.shadowRoot??editor,entry=root.querySelector('article');
      entry.querySelector('summary').click();
      const enabled=entry.querySelector('input[type="checkbox"]');enabled.focus();enabled.click();
      await wait(()=>!editor.extensionsBusy&&!editor.extensionsEntries[0].enabled);
      const disabled=await window.__TAURI_INTERNALS__.invoke('extension_status');
      if(disabled.entries[0].catalog.enabled)throw new Error('Extension disable was not persisted');
      enabled.click();await wait(()=>!editor.extensionsBusy&&editor.extensionsEntries[0].enabled);
      if(!entry.querySelector('details').open)throw new Error('Changing extension enable state collapsed declarations');
      const reload=root.querySelector('input[aria-label="Hot reload Development visualizer fixture"]');reload.click();
      await wait(()=>!editor.extensionsBusy&&editor.extensionsEntries[0].hotReload);
      const status=await window.__TAURI_INTERNALS__.invoke('extension_status');
      if(!status.entries[0].hotReload)throw new Error('Developer hot reload was not persisted');
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
    await evaluate(`(async()=>{const editor=document.querySelector('app-shell').settings.extensionsEditor,root=editor.shadowRoot??editor;root.querySelector('button[aria-label="Uninstall Development visualizer fixture"]').click();const deadline=Date.now()+10000;while(editor.extensionsBusy){if(Date.now()>deadline)throw new Error('Native extension removal timed out');await new Promise(resolve=>setTimeout(resolve,25));}if(editor.extensionsError||editor.extensionsEntries.length)throw new Error('Native extension removal failed');})()`);
    return {...result,native,removed:true};
  } finally {
    await call('Emulation.clearDeviceMetricsOverride');
    await evaluate(`(()=>{const saved=globalThis.__extensionProbe;if(!saved)return;const shell=document.querySelector('app-shell');shell.settings.showSettingsSection(saved.section);shell.shadowRoot.querySelector('a[data-view="'+saved.view+'"]').click();delete globalThis.__extensionProbe;})()`);
  }
}
