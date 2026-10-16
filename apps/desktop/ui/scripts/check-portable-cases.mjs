import assert from 'node:assert/strict';
import {resolve} from 'node:path';
export async function checkPortableCases(page,root){
  await page.goto('https://workspace.test/');await page.waitForFunction(()=>document.querySelector('app-shell').traffic.queryLoaded);
  await page.evaluate(()=>{
    const state=globalThis.__workspaceFixture;
    state.caseChoices={rules:[{id:'rule-a',label:'Keep the exact encoded response'}],variants:[]};
    state.caseReview={manifest:{id:'portable-test',kind:'case',title:'Checkout investigation — customer-provided context <script>inert</script>',sources:[{id:'source-a',artifactSha256:'fixture-digest',sourceKey:'root',label:'Original source with a long descriptive label — '+ 'customer evidence '.repeat(8),dependency:{state:'present'},evidenceCompleteness:'truncated'}],rules:[{id:'rule-a'}],assets:[{}],experiments:[{id:'future',label:'Unsupported optional experiment'}],notes:[{id:'note',source:{artifactSha256:'fixture-digest',sourceKey:'root',exchangeId:'original',occurrence:'0'},text:'Authored observation <img src=x onerror=alert(1)>\nOriginal config output remains source evidence.'}],contexts:[{id:'context-a',kind:'capture-metadata',completeness:'partial',content:{state:'present',sha256:'context-digest'}}],markers:[{}],runs:[],objects:[{role:'capture',bytes:'1000'},{role:'context',bytes:'20',sha256:'context-digest',mediaType:'application/json'}]},inventory:{missingDependencies:1,incompleteSources:1,experiments:[{id:'future',state:'unavailable',reason:'This optional experiment is not supported by this build.'}]},excluded:['Unselected evidence and working history'],activation:'Opening and importing do not activate rules or scripts.'};
  });
  const traffic=page.locator('traffic-workspace');const dialog=page.locator('portable-case dialog');
  await traffic.getByRole('button',{name:'Files',exact:true}).click();await traffic.getByRole('button',{name:'Save portable case…',exact:true}).click();
  await dialog.getByLabel('Title',{exact:true}).fill('A portable investigation');
  await dialog.getByLabel('Evidence',{exact:true}).selectOption('filtered');
  await dialog.getByLabel('Reproduction notes and expected / actual behavior').fill('Reproduce checkout.');
  await dialog.getByLabel('I authorize a new derived copy',{exact:false}).check();
  await dialog.getByRole('button',{name:'Review selected content',exact:true}).click();await dialog.getByText(/Verified|Review the frozen/).waitFor();
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.caseExportInput.scope.kind),'filtered');
  await dialog.getByRole('heading',{name:/Checkout investigation/}).waitFor();assert.equal(await dialog.locator('script,img').count(),0);
  for(const [width,height] of [[1440,900],[960,720],[800,600]]){
    await page.setViewportSize({width,height});await dialog.getByRole('button',{name:'Save portable case…',exact:true}).scrollIntoViewIfNeeded();
    const rect=await dialog.boundingBox();assert.ok(rect.x>=0&&rect.y>=0&&rect.x+rect.width<=width+1&&rect.y+rect.height<=height+1);
    await page.screenshot({path:resolve(root,`../../../target/ui-check/portable-case-${width}.png`)});
  }
  await page.emulateMedia({forcedColors:'active'});await page.screenshot({path:resolve(root,'../../../target/ui-check/portable-case-forced-colors.png')});await page.emulateMedia({forcedColors:'none'});
  await page.evaluate(()=>{globalThis.__workspaceFixture.deferCaseSave=true;});
  await dialog.getByRole('button',{name:'Save portable case…',exact:true}).click();await page.waitForFunction(()=>globalThis.__workspaceFixture.releaseCaseSave);
  await dialog.getByRole('button',{name:'Close review',exact:true}).click();await traffic.getByRole('button',{name:'Files',exact:true}).click();await traffic.getByRole('button',{name:'Save portable case…',exact:true}).click();
  await dialog.getByRole('button',{name:'Cancel operation',exact:true}).click();await dialog.getByText('Case operation canceled.',{exact:true}).waitFor();
  await dialog.getByRole('button',{name:'Close review',exact:true}).click();
  await page.evaluate(()=>{globalThis.__workspaceFixture.caseLocked=true;});
  await traffic.getByRole('button',{name:'Files',exact:true}).click();await traffic.getByRole('button',{name:'Open portable case…',exact:true}).click();
  const password=page.getByRole('dialog',{name:'Unlock portable package'});await password.getByLabel('Password',{exact:true}).fill('wrong');await password.getByRole('button',{name:'Continue',exact:true}).click();
  await password.getByText('The package password is incorrect',{exact:true}).waitFor();await password.getByLabel('Password',{exact:true}).fill('fixture');await password.getByRole('button',{name:'Continue',exact:true}).click();
  await dialog.getByRole('button',{name:'Inspect in Traffic',exact:true}).click();await dialog.getByText(/Evidence opened in this window/).waitFor();
  await dialog.getByRole('button',{name:'View context',exact:true}).click();await dialog.getByText(/Original config output <script>/).waitFor();assert.equal(await dialog.locator('script,img').count(),0);
  await dialog.getByText('Copy selected rules into this workbench',{exact:true}).waitFor();
  assert.equal(await dialog.getByLabel('Unsupported optional experiment',{exact:false}).isDisabled(),true);
  await dialog.getByLabel('Ordinary saved rules',{exact:true}).check();await dialog.getByRole('button',{name:'Review rule import',exact:true}).click();
  await dialog.getByText('conflicting',{exact:true}).waitFor();await dialog.getByRole('button',{name:'Import rules disabled',exact:true}).click();
  await dialog.getByText(/Imported rules disabled. Named experiments/).first().waitFor();assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.caseImported),true);
  await page.keyboard.press('Escape');assert.equal(await dialog.isVisible(),false);
  assert.equal(await traffic.getByRole('button',{name:'Files',exact:true}).evaluate(e=>e===e.getRootNode().activeElement),true);
  await traffic.getByRole('button',{name:'Files',exact:true}).click();await traffic.getByRole('button',{name:'Open portable case…',exact:true}).click();await dialog.getByRole('button',{name:'Inspect linked request',exact:true}).click();await page.waitForFunction(()=>document.querySelector('app-shell').traffic.selectedSessionId==='first');assert.equal(await dialog.isVisible(),false);
  await page.setViewportSize({width:1440,height:900});
  // Production manifests omit empty arrays. A notes-only case remains usable.
  await page.evaluate(()=>{globalThis.__workspaceFixture.caseReview={manifest:{id:'notes-only',kind:'case',title:'Notes-only case',notes:[{id:'note',text:'Standalone reproduction notes'}]},inventory:{missingDependencies:0,incompleteSources:0,experiments:[]},excluded:[],activation:'Opening and importing do not activate rules or scripts.'};});
  await traffic.getByRole('button',{name:'Files',exact:true}).click();await traffic.getByRole('button',{name:'Save portable case…',exact:true}).click();
  await dialog.getByRole('button',{name:'Review selected content',exact:true}).waitFor();
  await dialog.getByLabel('Evidence',{exact:true}).selectOption('none');
  await dialog.getByLabel('I authorize a new derived copy',{exact:false}).check();
  await dialog.getByRole('button',{name:'Review selected content',exact:true}).click();await dialog.getByRole('heading',{name:'Notes-only case',exact:true}).waitFor();
  await dialog.getByText('No captured evidence is included.',{exact:true}).waitFor();await dialog.getByText('Standalone reproduction notes',{exact:true}).waitFor();
  await dialog.getByRole('button',{name:'Close review',exact:true}).click();
}
