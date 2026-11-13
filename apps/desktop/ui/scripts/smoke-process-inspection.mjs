import { writeFile } from "node:fs/promises";

// Native command and WebView2 coverage in the harness-owned profile. The proxy
// uses manual routing throughout; this probe never installs a certificate.
export async function smokeProcessInspection(call, evaluate, screenshotPath) {
  try {
    await evaluate(`(async()=>{
      const shell=document.querySelector('app-shell'),settings=shell.settings;await settings.ready;
      const original=await window.__TAURI_INTERNALS__.invoke('product_state');
      const bootstrap=await window.__TAURI_INTERNALS__.invoke('desktop_bootstrap');
      if((await window.__TAURI_INTERNALS__.invoke('app_status')).lifecycle!=='stopped')throw new Error('Process probe requires a stopped proxy');
      globalThis.__processInspectionProbe={original,bootstrap,view:shell.activeView,section:settings.settingsSection};
      const manual=structuredClone(original);manual.preferences.configureSystemProxy=false;
      await window.__TAURI_INTERNALS__.invoke('save_product_state',{productState:manual});await settings.loadSettings();
      shell.shadowRoot.querySelector('a[data-view="settings"]').click();settings.showSettingsSection('preferences');await settings.$whenRendered();
      const panel=settings.processInspectionEditor,mode=panel.querySelector('select');
      if(mode.closest('details'))throw new Error('Process selection is hidden in advanced settings');
      if(panel.processMode!=='all')throw new Error('Process selection did not default to All processes');
      mode.value='only';mode.dispatchEvent(new Event('change'));await panel.$whenRendered();
      await settings.saveSettings(new Event('submit',{cancelable:true}));
      if(!panel.processError.includes('at least one'))throw new Error('Empty process filter was accepted');
      await panel.refreshProcesses();
      // Validation focuses the input, which may already own an inventory lookup.
      // refreshProcesses deliberately coalesces that request; await its outcome.
      const inventoryDeadline=performance.now()+15000;
      while(panel.processLoading&&performance.now()<inventoryDeadline)await new Promise(resolve=>setTimeout(resolve,25));
      const inventory={loading:panel.processLoading,status:panel.processSuggestionStatus,count:panel.processSuggestions.length};
      if(panel.processLoading)throw new Error('Native process inventory did not finish: '+JSON.stringify(inventory));
      if(!panel.processSuggestions.some(name=>name.toLowerCase().includes('transmog')))throw new Error('Native process inventory omitted the app: '+JSON.stringify(inventory));
      panel.processNameInput.scrollIntoView({block:'center'});panel.processNameInput.focus();
    })()`);
    await call("Input.insertText", { text: "Transmog" });
    await call("Input.dispatchKeyEvent", {
      type: "keyDown",
      key: "Enter",
      code: "Enter",
      windowsVirtualKeyCode: 13,
    });
    await call("Input.dispatchKeyEvent", {
      type: "keyUp",
      key: "Enter",
      code: "Enter",
      windowsVirtualKeyCode: 13,
    });
    const result = await evaluate(`(async()=>{
      const shell=document.querySelector('app-shell'),settings=shell.settings,panel=settings.processInspectionEditor;
      if(panel.processNames.join()!=='transmog.exe')throw new Error('Keyboard Add did not normalize the executable name');
      panel.processNameInput.value='TRANSMOG.EXE';panel.addName();
      if(!panel.processError.includes('already')||panel.processNames.length!==1)throw new Error('Process list did not reject duplicates');
      panel.processNameInput.value='';await settings.saveSettings(new Event('submit',{cancelable:true}));
      if(settings.settingsError)throw new Error(settings.settingsError);
      await settings.startHttpsTunnels();
      const status=await window.__TAURI_INTERNALS__.invoke('app_status'),saved=await window.__TAURI_INTERNALS__.invoke('product_state'),after=await window.__TAURI_INTERNALS__.invoke('desktop_bootstrap');
      const probe=globalThis.__processInspectionProbe;
      if(status.lifecycle!=='running'||status.httpsDecryption!==false||status.processInspection.mode!=='only'||saved.preferences.decryptHttps||saved.preferences.configureSystemProxy)throw new Error('Certificate-free process-filtered startup failed: '+settings.proxyText);
      for(const key of ['caFilesPresent','caFilesExist','ownedCaSha256','ownedCaTrusted'])if(after[key]!==probe.bootstrap[key])throw new Error('Certificate-free start changed '+key);
      const captureBefore=await window.__TAURI_INTERNALS__.invoke('capture_status');
      if(captureBefore.state!=='active'){
        await settings.stopProxy();
        const stopDeadline=performance.now()+15000;
        while((await window.__TAURI_INTERNALS__.invoke('app_status')).lifecycle!=='stopped'){
          if(performance.now()>stopDeadline)throw new Error('Initial tunnel run did not stop before recording setup');
          await new Promise(resolve=>setTimeout(resolve,25));
        }
        const path=probe.bootstrap.diagnosticsPath+'.live-https-'+Date.now()+'.tmcap';
        await window.__TAURI_INTERNALS__.invoke('start_capture',{request:{path,password:null,retainBodySamples:false,redactSensitiveHeaders:false,includeNetworkContext:false}});
        probe.recordingStarted=true;
      }
      const capture=await window.__TAURI_INTERNALS__.invoke('capture_status');
      const recordingRun=await window.__TAURI_INTERNALS__.invoke('app_status');await settings.refreshStatus();
      const checkLive=async(enabled)=>{const live=await window.__TAURI_INTERNALS__.invoke('app_status'),recording=await window.__TAURI_INTERNALS__.invoke('capture_status');if(live.lifecycle!=='running'||live.listener!==recordingRun.listener||live.httpsDecryption!==enabled||recording.state!=='active'||recording.path!==capture.path)throw new Error('Saving HTTPS mode restarted the listener or recording: '+JSON.stringify({live,recording}));};
      const decrypt=settings.settingsForm.elements.namedItem('decryptHttps');
      const changeDecrypt=value=>{decrypt.checked=value;decrypt.dispatchEvent(new Event('change',{bubbles:true}));};
      if(!probe.bootstrap.caFilesPresent){
        panel.processNameInput.value='native-draft.exe';panel.addName();
        changeDecrypt(true);await settings.saveSettings(new Event('submit',{cancelable:true}));
        if(!settings.settingsError||!settings.httpsEnableError||!decrypt.checked||!panel.processNames.includes('native-draft.exe'))throw new Error('Enabling without a certificate lost its actionable draft');
        if((await window.__TAURI_INTERNALS__.invoke('product_state')).preferences.decryptHttps)throw new Error('Missing-certificate enable persisted before validation');
        await checkLive(false);
        const action=[...settings.querySelectorAll('.settings-save button')].find(button=>button.textContent.trim()==='Interception certificate…');
        action.click();await settings.$whenRendered();
        if(!settings.interceptionSummary.closest('details').open||settings.getRootNode().activeElement!==settings.interceptionSummary)throw new Error('Missing-certificate recovery did not focus certificate settings');
        await settings.backToPreferences();
        if(!decrypt.checked||!panel.processNames.includes('native-draft.exe'))throw new Error('Certificate navigation discarded the failed-enable draft');
        await window.__TAURI_INTERNALS__.invoke('create_ca',{request:{certificatePath:probe.bootstrap.caCertificatePath,privateKeyPath:probe.bootstrap.caPrivateKeyPath,commonName:'Transmog native live HTTPS fixture',validityDays:2}});
        const generated=await window.__TAURI_INTERNALS__.invoke('desktop_bootstrap');
        if(generated.ownedCaTrusted!==probe.bootstrap.ownedCaTrusted)throw new Error('Live HTTPS fixture changed certificate trust');
        probe.missingCertificateDraftPreserved=true;
      }
      changeDecrypt(true);await settings.saveSettings(new Event('submit',{cancelable:true}));if(settings.settingsError)throw new Error(settings.settingsError);await checkLive(true);
      changeDecrypt(false);await settings.saveSettings(new Event('submit',{cancelable:true}));if(settings.settingsError)throw new Error(settings.settingsError);await checkLive(false);
      settings.certificateSettingsButton.click();await settings.$whenRendered();
      if(!settings.interceptionSummary.closest('details').open||settings.getRootNode().activeElement!==settings.interceptionSummary)throw new Error('Certificate settings were unavailable while HTTPS decryption was off');
      await settings.backToPreferences();if(settings.getRootNode().activeElement!==settings.certificateSettingsButton)throw new Error('Certificate settings did not restore their opener');
      changeDecrypt(true);await settings.saveSettings(new Event('submit',{cancelable:true}));if(settings.settingsError)throw new Error(settings.settingsError);await checkLive(true);
      const includeBefore=panel.processNames.join(),mode=panel.querySelector('select');mode.value='except';mode.dispatchEvent(new Event('change'));await panel.$whenRendered();
      panel.processNameInput.value='service.helper.exe';panel.addName();
      mode.value='only';mode.dispatchEvent(new Event('change'));await panel.$whenRendered();if(panel.processNames.join()!==includeBefore)throw new Error('Mode switch lost the include draft');
      mode.value='except';mode.dispatchEvent(new Event('change'));await panel.$whenRendered();
      await settings.saveSettings(new Event('submit',{cancelable:true}));
      const updated=await window.__TAURI_INTERNALS__.invoke('app_status');
      if(updated.processInspection.mode!=='except'||updated.processInspection.exclude.join()!=='service.helper.exe'||!updated.processInspection.include.includes('transmog.exe')||updated.httpsDecryption!==true)throw new Error('Live process policy or effective HTTPS status did not update');
      const remove=panel.querySelector('.process-names button');remove.click();await panel.$whenRendered();
      if(panel.getRootNode().activeElement!==panel.processNameInput)throw new Error('Removing a process lost keyboard focus');
      panel.processNameInput.value='service.helper.exe';panel.addName();
      panel.querySelector('details').open=true;mode.scrollIntoView({block:'center'});
      return {manualRouting:true,nativeInventory:true,keyboardAdd:true,emptyValidation:true,deduplicated:true,separateLists:true,livePolicy:true,effectiveHttpsTunnels:true,certificateStatePreserved:true,removeFocus:true,liveHttpsToggle:true,listenerPreserved:true,capturePreserved:true,certificateSettingsDiscoverable:true,missingCertificateDraftPreserved:probe.missingCertificateDraftPreserved??false};
    })()`);
    for (const [width, height, suffix, forced] of [
      [1440, 900, "wide", false],
      [960, 720, "compact", false],
      [960, 720, "forced-colors", true],
    ]) {
      await call("Emulation.setDeviceMetricsOverride", {
        width,
        height,
        deviceScaleFactor: 1,
        mobile: false,
      });
      await call("Emulation.setEmulatedMedia", {
        features: forced ? [{ name: "forced-colors", value: "active" }] : [],
      });
      await evaluate(
        `(()=>{const panel=document.querySelector('app-shell').settings.processInspectionEditor;panel.querySelector('select').scrollIntoView({block:'center'});panel.processNameInput.focus();const box=panel.processNameInput.getBoundingClientRect();if(panel.getRootNode().elementFromPoint(box.x+box.width/2,box.y+box.height/2)!==panel.processNameInput)throw new Error('Process input is covered');if(panel.scrollWidth>panel.clientWidth+1)throw new Error('Process controls overflow');})()`,
      );
      if (screenshotPath)
        await writeFile(
          screenshotPath.replace(/\.png$/i, `-process-inspection-${suffix}.png`),
          Buffer.from(
            (await call("Page.captureScreenshot", { format: "png", captureBeyondViewport: false }))
              .data,
            "base64",
          ),
        );
    }
    return result;
  } finally {
    await call("Emulation.setEmulatedMedia", { features: [] });
    await call("Emulation.clearDeviceMetricsOverride");
    await evaluate(`(async()=>{const probe=globalThis.__processInspectionProbe;if(!probe)return;const shell=document.querySelector('app-shell'),settings=shell.settings;
      if(probe.recordingStarted&&(await window.__TAURI_INTERNALS__.invoke('capture_status')).state==='active')await window.__TAURI_INTERNALS__.invoke('stop_capture');
      await window.__TAURI_INTERNALS__.invoke('stop_application');
      const latest=await window.__TAURI_INTERNALS__.invoke('product_state');
      await window.__TAURI_INTERNALS__.invoke('save_product_state',{productState:probe.original,expectedProcessInspection:latest.preferences.processInspection});await settings.loadSettings();await settings.refreshStatus();settings.showSettingsSection(probe.section);shell.shadowRoot.querySelector('a[data-view="'+probe.view+'"]').click();delete globalThis.__processInspectionProbe;
    })()`);
  }
}
