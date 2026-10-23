import { writeFile } from "node:fs/promises";

export async function smokeLoopback(call, evaluate, screenshotPath) {
  try {
    const result = await evaluate(`(async()=>{
      const shell=document.querySelector('app-shell'),settings=shell.settings,manager=settings.loopbackManager;
      globalThis.__loopbackProbe={view:shell.activeView,section:settings.settingsSection};
      shell.shadowRoot.querySelector('a[data-view="settings"]').click();settings.showSettingsSection('connection');settings.$flushUpdates();
      const button=[...settings.querySelectorAll('button')].find(button=>button.textContent==='Manage loopback exemptions…');button.focus();button.click();
      for(let index=0;index<120&&manager.loopbackBusy;index++)await new Promise(resolve=>setTimeout(resolve,100));
      if(manager.loopbackBusy||manager.loopbackError||!manager.loopbackCurrent)throw new Error('Native AppContainer enumeration failed: '+manager.loopbackError);
      if(!manager.exemptionDialog.matches(':modal'))throw new Error('Loopback manager did not open as a modal');
      const count=manager.loopbackRows.length;
      if(count){const cells=manager.containerTable.querySelector('tbody tr').children;if(getComputedStyle(cells[3]).whiteSpace!=='normal'||getComputedStyle(cells[7]).whiteSpace!=='pre-wrap')throw new Error('Native AppContainer metadata is truncated instead of wrapped');}
      if(new Set(manager.loopbackRows.map(row=>row.sid)).size!==count)throw new Error('AppContainers were not grouped by SID');
      if(count){manager.selectMatching();manager.$flushUpdates();if(manager.loopbackSelected!==count)throw new Error('Native inventory multiselection failed');}
      if(manager.loopbackSelectText!=='Select all'||manager.loopbackAddCount+manager.loopbackRemoveCount!==count)throw new Error('Native selection actions do not describe the inventory');
      const add=[...manager.querySelectorAll('button')].find(button=>button.textContent.startsWith('Add selected exemptions ('));
      const remove=[...manager.querySelectorAll('button')].find(button=>button.textContent.startsWith('Remove selected exemptions ('));
      if(!add||!remove||add.disabled!==(manager.loopbackAddCount===0)||remove.disabled!==(manager.loopbackRemoveCount===0))throw new Error('Native exemption actions do not reflect selected state');
      const exempt=manager.loopbackRemoveCount;
      const filter=manager.querySelector('.loopback-exempt-only input');filter.click();manager.$flushUpdates();
      if(manager.loopbackRows.length!==exempt||manager.loopbackSelectText!=='Select filtered')throw new Error('Native exempt-only filter failed');
      manager.selectMatching();manager.$flushUpdates();
      if(manager.loopbackAddCount!==0||manager.loopbackRemoveCount!==exempt||!add.disabled)throw new Error('Native already-exempt selection allows adding');
      filter.click();manager.selectMatching();manager.$flushUpdates();
      for(const heading of ['Display Name','Description','Package','AppContainer name','AppContainer SID','AppContainer user(s)'])if(!manager.exemptionDialog.textContent.includes(heading))throw new Error('Missing column '+heading);
      const scroller=manager.querySelector('.loopback-table-scroll');if(count>30&&scroller.scrollHeight<=scroller.clientHeight)throw new Error('Populated native AppContainers did not scroll');
      return {enumerated:count,exempt,groupedUsers:true,multiSelection:true,selectionActions:true,exemptFilter:true,columns:true,readOnly:true};
    })()`);
    if (screenshotPath)
      await writeFile(
        screenshotPath.replace(/\.png$/i, "-loopback.png"),
        Buffer.from(
          (await call("Page.captureScreenshot", { format: "png", captureBeyondViewport: false }))
            .data,
          "base64",
        ),
      );
    return result;
  } finally {
    await evaluate(
      `(()=>{const probe=globalThis.__loopbackProbe;if(!probe)return;const shell=document.querySelector('app-shell'),manager=shell.settings.loopbackManager;const filter=manager.querySelector('.loopback-exempt-only input');if(filter.checked)filter.click();manager.clearSelection();manager.close();shell.settings.showSettingsSection(probe.section);shell.shadowRoot.querySelector('a[data-view="'+probe.view+'"]').click();delete globalThis.__loopbackProbe;})()`,
    );
  }
}
