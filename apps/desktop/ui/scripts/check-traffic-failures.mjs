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
        {...seed,id:'dns-failure',status:null,terminal:'failed',protocol:'Unavailable',requestBytes:0,responseBytes:0,topLevelNavigation:false,storedBodies:[]},
        {...seed,id:'http2-failure',status:null,terminal:'failed',protocol:'HTTP/2'},
      ].map(row=>({...row,url:'http://example.test/'+row.id,path:'/'+row.id}));
      await traffic.refreshSessions(undefined,true);
    });
    const dns = page.locator('tr[data-session-id="dns-failure"]');
    await dns.getByText('Failed',{exact:true}).waitFor({state:'visible'});
    assert.equal((await dns.locator('td[data-column-id="protocol"]').textContent()).trim(),'Unavailable');
    assert.equal(await dns.locator('td[data-column-id="protocol"]').getAttribute('title'),'Unavailable');
    assert.equal((await page.locator('tr[data-session-id="http2-failure"] td[data-column-id="protocol"]').textContent()).trim(),'HTTP/2');
    await dns.click();
    await page.waitForFunction(()=>document.querySelector('app-shell').traffic.selectedDetail?.id==='dns-failure');
    assert.equal(await page.locator('.selection-bar .status-badge').textContent(),'Failed');
    await page.screenshot({path:resolve(artifacts,'dns-failure-wide.png')});
    await page.setViewportSize({width:800,height:600});
    await dns.focus();
    await page.keyboard.press('Enter');
    await dns.locator('td[data-column-id="protocol"]').scrollIntoViewIfNeeded();
    assert.equal(await dns.locator('td[data-column-id="protocol"]').getAttribute('title'),'Unavailable');
    assert.equal(await page.locator('.selection-bar .status-badge').textContent(),'Failed');
    await page.screenshot({path:resolve(artifacts,'dns-failure-small.png')});
    await page.setViewportSize(viewport);
    for (const [id, label] of [['active-post','Pending'],['failed-post','Failed'],['missing-status','Unavailable'],['http-failure','503']]) {
      const row = page.locator(`tr[data-session-id="${id}"]`);
      await row.getByText(label,{exact:true}).waitFor({state:'visible'});
      await row.click();
      await page.waitForFunction(id=>document.querySelector('app-shell').traffic.selectedDetail?.id===id,id);
      assert.equal(await page.locator('.selection-bar .status-badge').textContent(),label);
      if (id==='failed-post') assert.equal(await page.locator('.selection-bar .status-badge').getAttribute('data-tone'),'failed');
    }
    await page.locator('tr[data-session-id="failed-post"]').click();
    assert.equal(await page.locator('#selected-request-actions button').filter({hasText:'Create auto-response'}).getAttribute('title'),'This request failed; its response cannot be saved as an auto-response.');
    const request = page.locator('message-inspector[side="request"]');
    await request.getByRole('button',{name:'Body',exact:true}).click();
    await request.getByLabel('Body viewer',{exact:true}).selectOption('metadata');
    await request.locator('.body-preview').getByText(/"availability": "complete"/).waitFor({state:'visible'});
    assert.match(await request.locator('.body-facts').textContent(),/629 B retained \/ 629 B observed.*complete/);
    assert.doesNotMatch(await request.locator('.body-facts').textContent(),/incomplete|interrupted|lost/);
    await page.screenshot({path:resolve(artifacts,'failed-post-wide.png')});
    await page.setViewportSize({width:760,height:680});
    await page.getByRole('combobox',{name:'Message to inspect',exact:true}).selectOption('request');
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
