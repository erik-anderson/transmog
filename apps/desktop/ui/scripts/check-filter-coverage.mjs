import assert from 'node:assert/strict';
import {resolve} from 'node:path';

export async function checkFilterCoverage(page,root,{traffic,picker,open,apply,optionCount,settle}) {
  const rows=await page.evaluate(()=>globalThis.__workspaceFixture.sessions);
  await page.evaluate(async()=>{
    const t=document.querySelector('app-shell').traffic,s=globalThis.__workspaceFixture,seed=s.sessions[0];
    s.sessions=['HTTP/2','HTTP/2.0','http/2.0','HTTP/3','HTTP/3.0','HTTP/0.9','Unavailable'].map((protocol,index)=>({...seed,id:'coverage-'+index,requestNumber:String(index+1),protocol,method:index===5?'PROPFIND':index===6?'—':'GET',fetchDestination:index===5?'future-destination':null}));
    t.queryRevision++;await t.refreshSessions(undefined,true);
  });await settle();
  await open('protocol');
  assert.equal(await optionCount('HTTP/2'),'3');assert.equal(await optionCount('HTTP/3'),'2');
  assert.equal(await optionCount('HTTP/0.9'),'1');assert.equal(await optionCount('Other'),'1');assert.equal(await optionCount('Unavailable protocol'),'1');
  assert.equal(await picker.getByRole('checkbox',{name:'HTTP/2.0',exact:true}).count(),0,'Recorded spellings split one protocol into duplicate choices');
  for(const width of [1440,960,800]) {
    await page.setViewportSize({width,height:width===1440?900:width===960?720:600});
    const box=await picker.boundingBox();assert.ok(box.x>=0&&box.y>=0&&box.x+box.width<=width+1);
    await picker.getByRole('button',{name:'Apply filter',exact:true}).scrollIntoViewIfNeeded();
    await page.screenshot({path:resolve(root,'../../../target/ui-check/filter-coverage-'+width+'.png')});
  }
  const http2=picker.getByRole('checkbox',{name:'HTTP/2',exact:true});await http2.focus();await page.keyboard.press('Space');await apply();
  assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.totalMatched),3);
  // Captured spelling remains available in the table for diagnosis.
  assert.match(await traffic.locator('td[data-column-id="protocol"]').allTextContents().then(values=>values.join(' ')),/HTTP\/2\.0/);
  await open('protocol');await picker.getByRole('button',{name:'Clear selection',exact:true}).click();
  await picker.getByRole('checkbox',{name:'Other',exact:true}).check();await apply();
  assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.totalMatched),1);
  await page.evaluate(async()=>{const t=document.querySelector('app-shell').traffic,s=globalThis.__workspaceFixture;s.sessions.push({...s.sessions[5],id:'later-protocol',requestNumber:'8',protocol:'Future/1'});t.queryRevision++;await t.refreshSessions(undefined,true);});await settle();
  assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.totalMatched),2,'Other froze the values present when selected');
  await open('protocol');await picker.getByLabel('Search observed values').fill('http/2');await page.waitForFunction(()=>!document.querySelector('app-shell').traffic.pickerLoading);
  assert.equal(await optionCount('Other'),'2','Option search changed the residual count');await page.keyboard.press('Escape');
  await page.evaluate(async()=>{const t=document.querySelector('app-shell').traffic;await t.clearFilters();});
  await open('method');assert.equal(await optionCount('PROPFIND'),'2');assert.equal(await optionCount('Unavailable method'),'1');assert.equal(await optionCount('Other'),'2');
  await picker.getByRole('checkbox',{name:'Other',exact:true}).check();await apply();assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.totalMatched),2);
  await open('fetch-destination');assert.equal(await optionCount('future-destination'),'2');assert.equal(await optionCount('Other'),'2');assert.equal(await optionCount('Missing fetch destination'),'0');
  await picker.getByRole('checkbox',{name:'Other',exact:true}).check();await apply();assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.totalMatched),2);
  // A selected residual remains editable after surrounding filters make its count zero.
  await page.evaluate(async()=>{const t=document.querySelector('app-shell').traffic;t.filterGroups.push({dimension:'protocol',values:[{kind:'text',value:'http/2'}]});t.queryRevision++;await t.refreshSessions(undefined,true);});
  await open('method');assert.equal(await optionCount('Other'),'0');assert.equal(await picker.getByRole('checkbox',{name:'Other',exact:true}).isChecked(),true);await page.keyboard.press('Escape');
  await page.evaluate(async rows=>{const t=document.querySelector('app-shell').traffic,s=globalThis.__workspaceFixture;s.sessions=rows;t.filterGroups=[];t.queryRevision++;await t.refreshSessions(undefined,true);},rows);await settle();
  await open('method');assert.equal(await picker.getByRole('checkbox',{name:'Other',exact:true}).count(),0,'An unselected zero residual should be hidden');await page.keyboard.press('Escape');
  await page.setViewportSize({width:1440,height:900});
}
