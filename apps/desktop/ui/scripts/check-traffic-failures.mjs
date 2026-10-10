import assert from 'node:assert/strict';
import { resolve } from 'node:path';

export async function checkTrafficFailures(page, artifacts) {
  const viewport = page.viewportSize();
  const saved = await page.evaluate(() => globalThis.__workspaceFixture.sessions);
  try {
    await page.evaluate(async () => {
      const state = globalThis.__workspaceFixture, traffic = document.querySelector('app-shell').traffic;
      const seed = state.sessions[0];
      state.sessions = [
        {...seed,id:'active-post',method:'POST',status:null,terminal:'active'},
        {...seed,id:'failed-post',method:'POST',status:null,terminal:'failed',requestBytes:629,storedBodies:[{
          lengthKnown:true,exchangeId:'failed-post',boundary:'client-request',observedBytes:629,wireBodyBytes:629,
          retainedBytes:629,availability:'complete',mediaType:'application/json',charset:null,contentCodings:[],sha256:null,reason:null,
        }]},
        {...seed,id:'missing-status',status:null,terminal:'completed'},
        {...seed,id:'http-failure',status:503,terminal:'failed'},
      ].map(row=>({...row,url:'http://example.test/'+row.id,path:'/'+row.id}));
      await traffic.refreshSessions(undefined,true);
    });
    for (const [id, label] of [['active-post','Pending'],['failed-post','Failed'],['missing-status','Unavailable'],['http-failure','503']]) {
      const row = page.locator(`tr[data-session-id="${id}"]`);
      await row.getByText(label,{exact:true}).waitFor({state:'visible'});
      await row.click();
      await page.waitForFunction(id=>document.querySelector('app-shell').traffic.selectedDetail?.id===id,id);
      assert.equal(await page.locator('.selection-bar .status-badge').textContent(),label);
      if (id==='failed-post') assert.equal(await page.locator('.selection-bar .status-badge').getAttribute('data-tone'),'failed');
    }
    await page.locator('tr[data-session-id="failed-post"]').click();
    assert.equal(await page.locator('.selection-bar').getByRole('button',{name:'Create auto-response',exact:true}).getAttribute('title'),'This request failed; its response cannot be saved as an auto-response.');
    const request = page.locator('message-inspector[side="request"]');
    await request.getByRole('button',{name:'Body',exact:true}).click();
    await request.getByLabel('Body viewer',{exact:true}).selectOption('metadata');
    await request.locator('.body-preview').getByText(/"availability": "complete"/).waitFor({state:'visible'});
    assert.match(await request.locator('.body-facts').textContent(),/629 B retained \/ 629 B observed.*complete/);
    assert.doesNotMatch(await request.locator('.body-facts').textContent(),/incomplete|interrupted|lost/);
    await page.screenshot({path:resolve(artifacts,'failed-post-wide.png')});
    await page.setViewportSize({width:760,height:680});
    await page.getByRole('button',{name:'Request',exact:true}).click();
    assert.equal(await page.locator('.selection-bar .status-badge').textContent(),'Failed');
    assert.equal(await request.getByLabel('Body viewer',{exact:true}).isVisible(),true);
    await page.screenshot({path:resolve(artifacts,'failed-post-small.png')});
  } finally {
    await page.setViewportSize(viewport);
    await page.evaluate(async rows=>{
      globalThis.__workspaceFixture.sessions=rows;
      const traffic=document.querySelector('app-shell').traffic;
      await traffic.refreshSessions(undefined,true);
      await traffic.inspectSession(rows[0]);
      document.querySelector('app-shell').shadowRoot.querySelector('message-inspector[side="request"]').showMode('headers');
    },saved);
  }
}
