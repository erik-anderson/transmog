import {writeFile} from 'node:fs/promises';

// Exercise the Windows command boundary in the harness-owned profile, preserving
// the original settings and avoiding changes to host proxy routing or CA trust.
export async function smokeNetworkSettings(call,evaluate,screenshotPath) {
  try {
    const result=await evaluate(`(async()=>{
    const shell=document.querySelector('app-shell'),settings=shell.shadowRoot.querySelector('settings-workspace');
    await settings.ready;
    const original=await window.__TAURI_INTERNALS__.invoke('product_state');
    const bootstrap=await window.__TAURI_INTERNALS__.invoke('desktop_bootstrap');
    globalThis.__networkSettingsProbe={original,view:shell.activeView,section:settings.settingsSection};
    shell.shadowRoot.querySelector('a[data-view="settings"]').click();settings.showSettingsSection('preferences');
    const manual=structuredClone(original);manual.preferences.configureSystemProxy=false;
    await window.__TAURI_INTERNALS__.invoke('save_product_state',{productState:manual});await settings.loadSettings();
    const disclosure=settings.querySelector('.preferences-form .connection-advanced');
    globalThis.__networkSettingsProbe.open=disclosure.open;
    if(disclosure.open)throw new Error('Advanced network settings opened by default');
    if(settings.settingsForm.elements.namedItem('decryptHttps').closest('details'))throw new Error('HTTPS decryption remains advanced');
    if(original.preferences.allowRemoteClients||original.preferences.proxyPort!==null||!original.preferences.reuseServerConnections)throw new Error('Unsafe network defaults');
    disclosure.open=true;
    for(const name of ['decryptHttps','isolateOutbound','outboundHttp1','outboundHttp2','outboundHttp3','reuseServerConnections']) {
      const input=settings.settingsForm.elements.namedItem(name);
      if(!input.checked)throw new Error(name+' was not enabled by default');
      input.scrollIntoView({block:'center'});input.focus();
      const box=input.getBoundingClientRect();
      if(settings.getRootNode().activeElement!==input||settings.getRootNode().elementFromPoint(box.x+box.width/2,box.y+box.height/2)!==input)throw new Error(name+' is not reachable');
      if(name!=='outboundHttp1')input.checked=false;
      input.dispatchEvent(new Event('change',{bubbles:true}));
    }
    await settings.saveSettings(new Event('submit',{cancelable:true}));
    const saved=await window.__TAURI_INTERNALS__.invoke('product_state');
    if(saved.preferences.decryptHttps||saved.preferences.isolateOutboundConnections||saved.preferences.configureSystemProxy||JSON.stringify(saved.preferences.allowedOutboundProtocols)!==JSON.stringify({http1:true,http2:false,http3:false}))throw new Error('Network settings did not persist');
    await settings.startProxy();
    const status=await window.__TAURI_INTERNALS__.invoke('app_status');
    if(status.lifecycle!=='running'||status.hostRestorePending)throw new Error('Opaque mode could not start without certificate setup: '+settings.proxyText);
    const after=await window.__TAURI_INTERNALS__.invoke('desktop_bootstrap');
    if(after.ownedCaSha256!==bootstrap.ownedCaSha256||after.caFilesExist!==bootstrap.caFilesExist||after.ownedCaTrusted!==bootstrap.ownedCaTrusted)throw new Error('Opaque mode changed certificate files or trust');
    if(!status.listener.startsWith('127.0.0.1:'))throw new Error('Default listener is remotely accessible');
    await settings.stopProxy();
    const port=Number(status.listener.split(':').at(-1));
    const form=settings.settingsForm;
    form.elements.namedItem('allowRemoteClients').checked=true;
    form.elements.namedItem('customProxyPort').checked=true;settings.updateProxyPortMode();settings.$flushUpdates();
    form.elements.namedItem('proxyPort').value=String(port);
    await settings.saveSettings(new Event('submit',{cancelable:true}));
    await settings.startProxy();
    const remote=await window.__TAURI_INTERNALS__.invoke('app_status');
    if(remote.listener!=='0.0.0.0:'+port)throw new Error('Remote opt-in or desired port was not applied: '+settings.proxyText);
    if((await window.__TAURI_INTERNALS__.invoke('product_state')).preferences.reuseServerConnections)throw new Error('Pooling choice was not saved');
    await settings.stopProxy();
    if((await window.__TAURI_INTERNALS__.invoke('app_status')).lifecycle!=='stopped')throw new Error('Opaque proxy did not stop');
    settings.$flushUpdates();disclosure.scrollIntoView({block:'start'});
    return {settingsPersisted:true,inputsReachable:true,opaqueStartWithoutCertificate:true,certificateStatePreserved:true,remoteOptIn:true,persistentPort:true,reuseDisabled:true,decryptionProminent:true};
  })()`);
    if(screenshotPath) {
      await writeFile(screenshotPath.replace(/\.png$/i,'-network-settings.png'),Buffer.from((await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:false})).data,'base64'));
      await call('Emulation.setDeviceMetricsOverride',{width:760,height:520,deviceScaleFactor:1,mobile:false});
      await evaluate(`(()=>{const settings=document.querySelector('app-shell').shadowRoot.querySelector('settings-workspace');const input=settings.settingsForm.elements.namedItem('outboundHttp3');input.scrollIntoView({block:'center'});input.focus();const box=input.getBoundingClientRect();if(settings.getRootNode().elementFromPoint(box.x+box.width/2,box.y+box.height/2)!==input)throw new Error('Narrow protocol control is covered');})()`);
      await writeFile(screenshotPath.replace(/\.png$/i,'-network-settings-compact.png'),Buffer.from((await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:false})).data,'base64'));
    }
    return result;
  } finally {
    await call('Emulation.clearDeviceMetricsOverride');
    await evaluate(`(async()=>{const probe=globalThis.__networkSettingsProbe;if(!probe)return;const shell=document.querySelector('app-shell'),settings=shell.shadowRoot.querySelector('settings-workspace');if((await window.__TAURI_INTERNALS__.invoke('app_status')).lifecycle!=='stopped')await settings.stopProxy();await window.__TAURI_INTERNALS__.invoke('save_product_state',{productState:probe.original});await settings.loadSettings();settings.querySelector('.preferences-form .connection-advanced').open=probe.open;settings.showSettingsSection(probe.section);shell.shadowRoot.querySelector('a[data-view="'+probe.view+'"]').click();settings.$flushUpdates();shell.$flushUpdates();delete globalThis.__networkSettingsProbe;})()`);
  }
}
