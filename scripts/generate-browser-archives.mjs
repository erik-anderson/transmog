import {createRequire} from 'node:module';
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {mkdtemp, mkdir, readFile, rm, writeFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {basename, dirname, resolve} from 'node:path';
import {sanitizeHar, sanitizeNetLog} from './sanitize-browser-archives.mjs';

const {chromium}=createRequire(new URL('../e2e/playwright/package.json',import.meta.url))('playwright');
const output=resolve(import.meta.dirname,'../crates/app/tests/fixtures/browser');
const temporaryRoot=resolve(tmpdir());
const temporary=await mkdtemp(resolve(temporaryRoot,'transmog-browser-fixture-'));
const server=createServer((request,response)=>{
  const css=request.url.startsWith('/style.css');
  response.writeHead(200,{
    'Content-Type':css?'text/css':'text/html; charset=utf-8',
    'X-Placeholder':'placeholder',
    'Date':'Thu, 01 Jan 2026 00:00:00 GMT',
  });
  response.end(css?'body { color: rgb(10, 20, 30); }':'<!doctype html><link rel="stylesheet" href="/style.css?placeholder=value"><h1>Placeholder</h1>');
});
let browser;
try {
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  const port=server.address().port;
  browser=await chromium.launch({headless:true,args:[
    '--disable-background-networking','--no-proxy-server',
    '--host-resolver-rules=MAP placeholder.invalid 127.0.0.1, MAP * ~NOTFOUND',
    '--log-net-log='+resolve(temporary,'placeholder.netlog'),'--net-log-capture-mode=Everything',
  ]});
  const context=await browser.newContext({recordHar:{
    path:resolve(temporary,'placeholder.har'),content:'embed',mode:'full',
  }});
  const page=await context.newPage();
  await page.goto('http://placeholder.invalid:'+port+'/');
  await context.close();
  await browser.close();browser=undefined;
  const rawHar=JSON.parse(await readFile(resolve(temporary,'placeholder.har'),'utf8'));
  const rawNetLog=JSON.parse(await readFile(resolve(temporary,'placeholder.netlog'),'utf8'));
  const har=sanitizeHar(rawHar,port);
  const netlog=sanitizeNetLog(rawNetLog,port);
  await mkdir(output,{recursive:true});
  await writeFile(resolve(output,'placeholder.har'),JSON.stringify(har,null,2)+'\n');
  await writeFile(resolve(output,'placeholder.netlog'),JSON.stringify(netlog,null,2)+'\n');
  console.log(`Sanitized Chromium fixtures: ${rawHar.log.browser.version}; ${har.log.entries.length} HTTP exchanges; ${netlog.events.length} NetLog events; ${output}`);
} finally {
  if(browser)await browser.close();
  if(server.listening)await new Promise(resolve=>server.close(resolve));
  assert.equal(dirname(resolve(temporary)),temporaryRoot);
  assert.ok(basename(temporary).startsWith('transmog-browser-fixture-'));
  await rm(temporary,{recursive:true,force:true});
}
