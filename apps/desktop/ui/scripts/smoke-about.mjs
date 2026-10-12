import {writeFile} from 'node:fs/promises';

export async function smokeAbout(call,evaluate,screenshotPath) {
  try {
    const result=await evaluate(`(async()=>{
      const shell=document.querySelector('app-shell'),settings=shell.settings;
      globalThis.__aboutProbe={view:shell.activeView,section:settings.settingsSection};
      shell.openAbout();await new Promise(resolve=>setTimeout(resolve,20));
      const status=await window.__TAURI_INTERNALS__.invoke('update_status');
      const about=settings.querySelector('.about-panel');
      if(settings.settingsSection!=='about'||!about.textContent.includes('Transmog'))throw new Error('About did not open');
      const version=settings.aboutUpdates.shadowRoot.querySelector('#update-panel').textContent;
      if(!version.includes(status.currentVersion)||!status.currentVersion)throw new Error('About omitted installed version');
      if(about.querySelector('a').href!=='https://transmog.net/')throw new Error('About website link is incorrect');
      if(!status.version&&shell.updateToolbarVisible)throw new Error('Updates remain prominent without an update');
      const button=about.querySelector('button');button.focus();await settings.showCredits();settings.$flushUpdates();
      if(!settings.creditsDialog.matches(':modal')||settings.credits.length<100)throw new Error('Credits licenses are unavailable');
      if(settings.creditsSummary.includes('Generated from the locked')||settings.creditsSummary.includes('Distributions must retain'))throw new Error('Developer credits copy remains visible');
      settings.closeCredits();settings.$flushUpdates();
      return {installedVersion:status.currentVersion,website:true,credits:true,quietToolbar:true};
    })()`);
    if(screenshotPath)await writeFile(screenshotPath.replace(/\.png$/i,'-about.png'),Buffer.from((await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:false})).data,'base64'));
    return result;
  } finally {
    await evaluate(`(()=>{const probe=globalThis.__aboutProbe;if(!probe)return;const shell=document.querySelector('app-shell');shell.settings.showSettingsSection(probe.section);shell.shadowRoot.querySelector('a[data-view="'+probe.view+'"]').click();delete globalThis.__aboutProbe;})()`);
  }
}
