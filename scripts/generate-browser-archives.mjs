// Produce valid browser HAR/NetLog samples using only a local placeholder site.
import {createRequire} from 'node:module';
import {createServer} from 'node:http';
import {mkdir} from 'node:fs/promises';
import {resolve} from 'node:path';
const {chromium}=createRequire(new URL('../e2e/playwright/package.json',import.meta.url))('playwright');
const output=resolve(import.meta.dirname,'../target/browser-archives');await mkdir(output,{recursive:true});
const server=createServer((request,response)=>{
  const css=request.url.startsWith('/style.css');
  response.writeHead(200,{'Content-Type':css?'text/css':'text/html; charset=utf-8','X-Placeholder':'placeholder'});
  response.end(css?'body { color: rgb(10, 20, 30); }':'<!doctype html><link rel="stylesheet" href="/style.css?placeholder=value"><h1>Placeholder</h1>');
});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const browser=await chromium.launch({headless:true,args:['--disable-background-networking','--host-resolver-rules=MAP placeholder.invalid 127.0.0.1, MAP * ~NOTFOUND','--log-net-log='+resolve(output,'placeholder.netlog'),'--net-log-capture-mode=Everything']});
try {const context=await browser.newContext({recordHar:{path:resolve(output,'placeholder.har'),content:'embed',mode:'full'}});const page=await context.newPage();await page.goto('http://placeholder.invalid:'+server.address().port+'/');await context.close();}
finally {await browser.close();await new Promise(resolve=>server.close(resolve));}
console.log('Browser archives saved in '+output);
