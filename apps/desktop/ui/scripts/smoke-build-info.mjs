import { writeFile } from "node:fs/promises";

export async function smokeBuildInfo(call, evaluate, screenshotPath) {
  try {
    const result = await evaluate(`(async()=>{
      const shell=document.querySelector('app-shell'),settings=shell.settings;
      await settings.ready;
      globalThis.__buildInfoProbe={view:shell.activeView,section:settings.settingsSection,focus:shell.shadowRoot.activeElement};
      shell.shadowRoot.querySelector('a[data-view="settings"]').click();
      settings.showSettingsSection('support');
      await settings.refreshDiagnostics();settings.$flushUpdates();
      const report=JSON.parse(settings.supportDetails),build=report.build;
      if(!build?.identity?.target||!build.identity.profile||!build.identity.rustc.startsWith('rustc '))throw new Error('Build identity is unavailable');
      if(build.identity.sourceCommit!==null&&!/^[0-9a-f]{40}$/.test(build.identity.sourceCommit))throw new Error('Invalid source commit');
      if(build.identity.sourceDirty!==null&&typeof build.identity.sourceDirty!=='boolean')throw new Error('Invalid source modified state');
      const names=new Set(build.dependencies.filter(item=>item.version).map(item=>item.name));
      for(const name of ['tauri','microsoft-webui','@microsoft/webui-framework','@tauri-apps/api','boring','v8','swc_ecma_parser'])if(!names.has(name))throw new Error('Missing built dependency '+name);
      const sources=new Set(build.bundledSources.map(item=>item.name));
      for(const name of ['BoringSSL','Chromium certificate policy','Chromium root store','Chromium Public Suffix List'])if(!sources.has(name))throw new Error('Missing bundled source '+name);
      if(!report.runtime.webviewVersion)throw new Error('Runtime WebView observation is unavailable');
      const details=settings.querySelector('.support-facts').nextElementSibling;
      globalThis.__buildInfoProbe.details=details;globalThis.__buildInfoProbe.open=details.open;
      details.open=true;
      const text=details.querySelector('pre');
      if(!text.textContent.includes('sourceCommit')||!text.textContent.includes('bundledSources'))throw new Error('Technical details omitted build metadata');
      text.scrollIntoView({block:'center'});text.focus();
      return {identity:build.identity,dependencies:build.dependencies,bundledSources:build.bundledSources,runtimeWebView:report.runtime.webviewVersion,technicalDetails:true};
    })()`);
    if (screenshotPath)
      await writeFile(
        screenshotPath.replace(/\.png$/i, "-build-info.png"),
        Buffer.from(
          (await call("Page.captureScreenshot", { format: "png", captureBeyondViewport: false }))
            .data,
          "base64",
        ),
      );
    return result;
  } finally {
    await evaluate(
      `(()=>{const probe=globalThis.__buildInfoProbe;if(!probe)return;const shell=document.querySelector('app-shell');if(probe.details)probe.details.open=probe.open;shell.settings.showSettingsSection(probe.section);shell.shadowRoot.querySelector('a[data-view="'+probe.view+'"]').click();if(probe.focus?.isConnected)probe.focus.focus();delete globalThis.__buildInfoProbe;})()`,
    );
  }
}
