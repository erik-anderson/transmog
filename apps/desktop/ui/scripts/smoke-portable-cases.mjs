// Real WebView2 + Tauri IPC. File selection is owned by the test-only native adapter.
import assert from 'node:assert/strict';
import {smokeCaseIntegration} from './smoke-case-integration.mjs';
import {createRequire} from 'node:module';
import {writeFile} from 'node:fs/promises';
import {resolve} from 'node:path';
const {chromium}=createRequire(new URL('../../../../e2e/playwright/package.json',import.meta.url))('playwright');
const argument=name=>process.argv[process.argv.indexOf(name)+1];
const port=argument('--port'),output=argument('--output');
assert.match(port,/^\d{1,5}$/);assert.ok(output);
const browser=await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
try{
 const page=browser.contexts()[0].pages().find(p=>p.url().includes('transmog-ui.localhost'));assert.ok(page);page.setDefaultTimeout(30000);
 const errors=[];page.on('pageerror',error=>errors.push(error.message));
 await page.waitForFunction(()=>document.querySelector('app-shell')?.traffic?.queryLoaded);
 const traffic=page.locator('traffic-workspace'),dialog=page.locator('portable-case dialog');
 await traffic.getByRole('button',{name:'Files',exact:true}).click();await traffic.getByRole('button',{name:'Save case…',exact:true}).click();
 await dialog.getByRole('button',{name:'Review selected content',exact:true}).waitFor();
 await dialog.getByLabel('Title',{exact:true}).fill('Native encrypted case qualification');
 await dialog.getByLabel('Traffic to include',{exact:true}).selectOption('none');
 await dialog.getByLabel('Notes').fill('Synthetic secret note <script>inert</script> Café\r\nExpected: retain exact authored context.');

 await dialog.getByRole('button',{name:'Review selected content',exact:true}).click();await dialog.getByRole('heading',{name:'Native encrypted case qualification',exact:true}).waitFor();
 await dialog.getByLabel('Protect the saved file with a password',{exact:true}).check();await dialog.getByRole('button',{name:'Save case…',exact:true}).click();
 const protect=page.getByRole('dialog',{name:'Protect the whole case'});await protect.getByLabel('Password',{exact:true}).fill('synthetic-native-test');await protect.getByLabel('Confirm password',{exact:true}).fill('synthetic-native-test');await protect.getByRole('button',{name:'Continue',exact:true}).click();
 await dialog.locator('header p').filter({hasText:/^Saved /}).waitFor();const destination=await dialog.locator('header p').textContent();
 await dialog.getByRole('button',{name:'Close',exact:true}).click();await traffic.getByRole('button',{name:'Files',exact:true}).click();await traffic.getByRole('button',{name:'Open case…',exact:true}).click();
 const unlock=page.getByRole('dialog',{name:'Unlock case'});await unlock.getByLabel('Password',{exact:true}).fill('wrong');await unlock.getByRole('button',{name:'Continue',exact:true}).click();
 await unlock.getByText('The package password is incorrect',{exact:true}).waitFor();await unlock.getByLabel('Password',{exact:true}).fill('synthetic-native-test');await unlock.getByRole('button',{name:'Continue',exact:true}).click();
 await dialog.getByText('Opening this case does not enable its rules or contact captured sites.',{exact:true}).waitFor();assert.equal(await dialog.getByLabel('Protect the saved file with a password',{exact:true}).isChecked(),true);assert.equal(await dialog.locator('script,img').count(),0);
 for(const [width,height] of [[1440,900],[960,720],[800,600]]){await page.setViewportSize({width,height});await page.screenshot({path:resolve(output,`portable-case-native-${width}.png`)});const box=await dialog.boundingBox();assert.ok(box.x>=0&&box.y>=0&&box.x+box.width<=width+1&&box.y+box.height<=height+1);}
 await dialog.getByRole('button',{name:'More',exact:true}).click();await dialog.getByRole('button',{name:'Save a copy…',exact:true}).click();await dialog.getByRole('button',{name:'Save a copy…',exact:true}).click();await protect.getByLabel('Password',{exact:true}).fill('synthetic-replacement-test');await protect.getByLabel('Confirm password',{exact:true}).fill('synthetic-replacement-test');await protect.getByRole('button',{name:'Continue',exact:true}).click();await dialog.locator('header p').filter({hasText:/^Saved /}).waitFor();
 // The test picker returns the original filename. Working-copy reads below
 // still use the old pinned package after its name has been replaced.
 await dialog.getByRole('button',{name:'Back to case',exact:true}).click();await dialog.getByRole('button',{name:'More',exact:true}).click();await dialog.getByText('Extract to a folder',{exact:true}).click();await dialog.getByRole('button',{name:'Save unencrypted folder…',exact:true}).click();await dialog.getByText(/^Folder saved to /).waitFor();
 const workingCopy=await dialog.locator('header p').textContent();
 await dialog.getByRole('button',{name:'Close',exact:true}).click();await traffic.getByRole('button',{name:'Files',exact:true}).click();await traffic.getByRole('button',{name:'Open case…',exact:true}).click();await unlock.getByLabel('Password',{exact:true}).fill('synthetic-native-test');await unlock.getByRole('button',{name:'Continue',exact:true}).click();await unlock.getByText('The package password is incorrect',{exact:true}).waitFor();await unlock.getByLabel('Password',{exact:true}).fill('synthetic-replacement-test');await unlock.getByRole('button',{name:'Continue',exact:true}).click();await dialog.getByText('Opening this case does not enable its rules or contact captured sites.',{exact:true}).waitFor();
 await page.keyboard.press('Escape');assert.equal(await dialog.isVisible(),false);
 assert.equal(await traffic.getByRole('button',{name:'Files',exact:true}).evaluate(e=>e===e.getRootNode().activeElement),true);await smokeCaseIntegration({page,output,chromium,processId:argument('--process-id'),closeHelper:argument('--close-helper')});assert.deepEqual(errors,[]);
 await writeFile(resolve(output,'portable-cases.json'),JSON.stringify({nativeWebView2:true,productionIpc:true,encryptedSaveAndOpen:true,passwordRetry:true,atomicPinnedReplacement:true,explicitUnencryptedFolderAction:true,inertAuthoredText:true,escapeFocus:true,widths:[1440,960,800],destination,workingCopy,errors},null,2));
 process.stdout.write('Native encrypted case save/open, pinned replacement, password retry, explicit working copy, inert text, responsive layout and Escape focus verified.\n');
}catch(error){
 const page=browser.contexts()[0]?.pages().find(p=>p.url().includes('transmog-ui.localhost'));
 if(page){await page.screenshot({path:resolve(output,'portable-case-native-failure.png')}).catch(()=>{});await writeFile(resolve(output,'portable-case-native-failure.txt'),JSON.stringify({dialogs:await page.getByRole('dialog').allTextContents(),state:await page.locator('portable-case').evaluate(e=>({stage:e.caseStage,status:e.caseStatus,busy:e.caseBusy}))},null,2)).catch(()=>{});}
 throw error;
}finally{await browser.close();}
