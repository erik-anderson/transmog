import assert from 'node:assert/strict';
import {resolve} from 'node:path';

export async function checkCopy(page,root) {
  await page.goto('https://workspace.test/');
  await page.evaluate(async()=>{
    const state=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic;
    Object.defineProperty(navigator,'clipboard',{configurable:true,value:{writeText:async text=>{globalThis.__copied=text;}}});
    t.preferences={...t.preferences,layout:'stacked',listSplit:25};
    state.visualizerChoices={body:[{extensionId:'example.protocol',visualizerId:'structure',name:'Copy structure'}],headers:[]};
    const node=(id,name,value,children=[],childrenToken=null)=>({id,name,value,children,childrenToken,spans:[]});
    state.visualizerDocument={title:'Copy fixture',status:'complete',presentation:'tree',summary:null,sources:[],diagnostics:[],roots:[
      node('first','First',{kind:'integer',value:'9007199254740993'}),
      node('branch','Branch',{kind:'object'},[], 'deferred'),
      {...node('bytes','Bytes',{kind:'bytes'}),spans:[{sourceId:'derived',offset:'1',length:'3'}]},
      node('long','Long',{kind:'string',value:'full-'.repeat(200)}),
    ]};
    state.visualizerDocument.sources=[{id:'derived',name:'Binary',bytesBase64:btoa(String.fromCharCode(1,0,10,255)),parents:[],transform:'fixture',exactMapping:false}];
    state.expandedVisualizerDocument=structuredClone(state.visualizerDocument);
    state.expandedVisualizerDocument.roots[1].childrenToken=null;
    state.expandedVisualizerDocument.roots[1].children=[node('child','Child',{kind:'string',value:'hello\nworld'})];
    state.emitNative('extensions-changed');await t.refreshSessions(undefined,true);
  });
  await page.locator('tr[data-session-id="first"]').click();
  const inspector=page.locator('traffic-workspace message-inspector[side="response"]');
  await inspector.getByRole('button',{name:'Body',exact:true}).click();
  await inspector.getByLabel('Body viewer',{exact:true}).selectOption({label:'Copy structure'});
  const visualizer=inspector.locator('extension-visualizer[slot="traffic-response-body"]');
  const row=name=>visualizer.getByRole('treeitem').filter({has:page.locator('.visualizer-field',{hasText:new RegExp('^'+name+'$')})});
  const menu=page.getByRole('menu',{name:'Copy inspection value',exact:true});
  await row('First').click();
  const selected=await visualizer.getByRole('tree').getAttribute('aria-activedescendant');
  await row('Bytes').click({button:'right'});await menu.getByRole('menuitem',{name:'Copy value',exact:true}).click();
  await page.waitForFunction(()=>globalThis.__copied==='00 0A FF');
  assert.equal(await visualizer.getByRole('tree').getAttribute('aria-activedescendant'),selected);
  await row('Long').click({button:'right'});await menu.getByRole('menuitem',{name:'Copy value',exact:true}).click();
  await page.waitForFunction(()=>globalThis.__copied==='full-'.repeat(200));
  await row('Branch').click({button:'right'});
  assert.equal(await menu.getByRole('menuitem',{name:'Copy value',exact:true}).isDisabled(),true);
  await menu.getByRole('menuitem',{name:'Copy subtree',exact:true}).click();
  await page.waitForFunction(()=>globalThis.__copied==='Branch\n  Child: hello\\nworld');
  assert.equal(await row('Branch').getAttribute('aria-expanded'),'false');
  assert.equal(await visualizer.getByRole('tree').getAttribute('aria-activedescendant'),selected);
  await visualizer.getByRole('tree').focus();await page.keyboard.press('Shift+F10');
  await menu.getByRole('menuitem',{name:'Copy name',exact:true}).waitFor();await page.keyboard.press('Escape');
  assert.equal(await visualizer.getByRole('tree').evaluate(element=>element===element.getRootNode().activeElement),true);
  await inspector.getByRole('button',{name:'Headers',exact:true}).click();
  const header=inspector.locator('.headers-table tbody tr').first();
  await header.click({button:'right'});await menu.getByRole('menuitem',{name:'Copy name and value',exact:true}).click();
  await page.waitForFunction(()=>globalThis.__copied==='Accept: */*');
  await page.evaluate(()=>{
    const state=globalThis.__workspaceFixture;
    state.authValues={public:'full public',secret:'private-value'};
    state.authInspection={findings:[{id:'f0',format:'Basic',role:'Credential',status:'decoded',recognition:'recognized',verification:'Not checked',source:{label:'Header',transforms:[]},diagnostics:[],children:[],fields:[{id:'public',name:'User',value:'full public',available:true,sensitive:false,note:''},{id:'secret',name:'Password',value:'••••••••',available:true,sensitive:true,note:''}]}],coverage:[],diagnostics:[]};
  });
  await inspector.getByRole('button',{name:/^Auth/}).click();
  const auth=inspector.locator('auth-inspector'),summary=auth.locator('.auth-finding > summary').first();
  await summary.click({button:'right'});await menu.getByRole('menuitem',{name:'Copy subtree',exact:true}).click();
  await page.waitForFunction(()=>globalThis.__copied==='Basic · Credential\n  User: full public\n  Password: [masked]');
  await summary.click({button:'right'});await menu.getByRole('menuitem',{name:'Copy subtree including secrets',exact:true}).click();
  await page.waitForFunction(()=>globalThis.__copied.includes('Password: private-value'));
  assert.equal(await auth.getByText('private-value',{exact:true}).count(),0);
  await page.screenshot({path:resolve(root,'../../../target/ui-check/copy-auth.png')});
  await page.goto('https://workspace.test/');
}
