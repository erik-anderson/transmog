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
  // Frozen case previews and saved comparisons use the existing trusted flows.
  await page.evaluate(()=>{const state=globalThis.__workspaceFixture;state.pagePreviewCanceled=false;state.caseLocked=false;state.caseReview.manifest.comparisons=[{id:'saved-pair',label:'Working / failing',kind:'exchange',engine:'transmog-comparison/1',left:[{artifactSha256:'fixture-digest',sourceKey:'root'}],right:[{artifactSha256:'fixture-digest',sourceKey:'root'}],recordedSummary:'Recorded 2 changes; not a current equality claim.'},{id:'saved-scopes',label:'Capture membership',kind:'captures',engine:'transmog-comparison/1',left:[{artifactSha256:'fixture-digest',sourceKey:'root'}],right:[{artifactSha256:'fixture-digest',sourceKey:'root'}],recordedSummary:'Historical metadata only.'}];state.caseReview.manifest.experiments.push({id:'frozen',label:'Frozen CSS',variants:[{id:'blue',label:'Blue',revisions:[{revision:'7',rules:[{kind:'auto-response',enabledIntent:true}]}]}]});state.caseChoices.comparisons=[{id:'comparison-1',label:'Working / failing',scope:{kind:'selected',ids:['first','second']}}];});
  await page.evaluate(()=>document.querySelector('app-shell').traffic.portableCase.closePackage());
  await traffic.getByRole('button',{name:'Files',exact:true}).click();await traffic.getByRole('button',{name:'Open portable case…',exact:true}).click();
  await page.evaluate(()=>{globalThis.__workspaceFixture.deferCaseChoices=true;});
  await dialog.getByRole('button',{name:'Preview case…',exact:true}).click();await page.waitForFunction(()=>globalThis.__workspaceFixture.releaseCaseChoices);
  await dialog.getByRole('button',{name:'Close review',exact:true}).click();await page.evaluate(()=>globalThis.__workspaceFixture.releaseCaseChoices());await page.waitForFunction(()=>!document.querySelector('app-shell').traffic.portableCase.caseBusy);assert.equal(await dialog.isVisible(),false);
  assert.notEqual(await page.evaluate(()=>document.querySelector('app-shell').traffic.portableCase.caseStage),'preview','Late document choices replaced the closed review');
  await traffic.getByRole('button',{name:'Files',exact:true}).click();await traffic.getByRole('button',{name:'Open portable case…',exact:true}).click();
  await dialog.getByRole('button',{name:'Preview case…',exact:true}).click();await dialog.getByRole('heading',{name:'Preview frozen responses',exact:true}).waitFor();
  assert.equal(await dialog.getByLabel('Enable scripts for this preview',{exact:true}).isChecked(),false);
  await dialog.getByLabel('Responses',{exact:true}).selectOption({label:'Frozen CSS / Blue / revision 7'});
  await dialog.getByLabel('I understand captured content will run',{exact:false}).check();
  for(const [width,height] of [[1440,900],[960,720],[800,600]]){await page.setViewportSize({width,height});await dialog.getByRole('button',{name:'Open case preview',exact:true}).scrollIntoViewIfNeeded();const rect=await dialog.boundingBox();assert.ok(rect.x>=0&&rect.y>=0&&rect.x+rect.width<=width+1&&rect.y+rect.height<=height+1);await page.screenshot({path:resolve(root,`../../../target/ui-check/case-integration-${width}.png`)});}
  await dialog.getByRole('button',{name:'Open case preview',exact:true}).click();await dialog.getByText(/Case preview opened/).first().waitFor();
  assert.deepEqual(await page.evaluate(()=>globalThis.__workspaceFixture.previewArgs.options.casePreview.selection),{experimentId:'frozen',variantId:'blue',revision:'7'});assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.previewArgs.enableScripts),false);
  await dialog.getByRole('button',{name:'Preview diagnostics…',exact:true}).click();const diagnostics=page.getByRole('dialog',{name:'Captured preview diagnostics',exact:true});await diagnostics.getByText(/fixture-case@1/).waitFor({state:'attached'});await diagnostics.getByRole('button',{name:'Close',exact:true}).click();assert.equal(await dialog.getByRole('button',{name:'Preview diagnostics…',exact:true}).evaluate(e=>e===e.getRootNode().activeElement),true);
  await dialog.locator('.case-row').filter({hasText:'Working / failing'}).getByRole('button',{name:'Recompute comparison',exact:true}).click();
  const comparison=page.getByRole('dialog',{name:'Compare requests and responses'});await comparison.waitFor();await page.waitForFunction(()=>!document.querySelector('app-shell').comparison.comparisonBusy);
  assert.deepEqual(await page.evaluate(()=>globalThis.__workspaceFixture.comparisonInput.options),{excludedHeaders:['date','x-request-id'],normalizeNewlines:true});
  await comparison.getByRole('button',{name:'Save comparison in case…',exact:true}).click();await dialog.getByRole('button',{name:'Review selected content',exact:true}).waitFor();
  assert.equal(await dialog.getByLabel('Evidence',{exact:true}).inputValue(),'comparison');assert.equal(await dialog.getByLabel('Working / failing',{exact:true}).isChecked(),true);
  await dialog.getByLabel('I authorize a new derived copy',{exact:false}).check();await dialog.getByRole('button',{name:'Review selected content',exact:true}).click();await dialog.getByRole('heading',{name:/Checkout investigation/}).waitFor();
  assert.deepEqual(await page.evaluate(()=>globalThis.__workspaceFixture.caseExportInput.comparisonIds),['comparison-1']);
  await dialog.getByRole('button',{name:'Close review',exact:true}).click();await comparison.getByRole('button',{name:'Close',exact:true}).click();
  await traffic.getByRole('button',{name:'Files',exact:true}).click();await traffic.getByRole('button',{name:'Open portable case…',exact:true}).click();
  await dialog.locator('.case-row').filter({hasText:'Capture membership'}).getByRole('button',{name:'Recompute comparison',exact:true}).click();
  const captures=page.getByRole('dialog',{name:'Compare capture scopes'});await captures.waitFor();await page.waitForFunction(()=>!document.querySelector('app-shell').comparison.captureBusy);assert.ok(await page.evaluate(()=>globalThis.__workspaceFixture.calls.reopen_case_capture_comparison));await captures.getByRole('button',{name:'Close',exact:true}).click();
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
