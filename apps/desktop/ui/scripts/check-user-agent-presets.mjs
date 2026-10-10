import assert from 'node:assert/strict';
import {build} from 'esbuild';
import {resolve} from 'node:path';

export async function checkUserAgentPresets(root) {
  const {outputFiles}=await build({entryPoints:[resolve(root,'src/user-agent-presets.ts')],bundle:true,write:false,format:'esm',platform:'node'});
  const {userAgentPresets,presetClientHints}=await import('data:text/javascript;base64,'+Buffer.from(outputFiles[0].contents).toString('base64'));
  const googlebook=userAgentPresets.find(preset=>preset.title==='Chrome — Googlebook');
  assert.ok(googlebook);
  for(const [year,month,day,version]of [[2026,8,30,15],[2026,9,1,16],[2026,11,31,16],[2027,0,1,16],[2027,9,1,17]]) {
    assert.equal(presetClientHints(googlebook,new Date(year,month,day,12))['Sec-CH-UA-Platform-Version'],`"${version}.0.0"`,'Googlebook did not resolve Chromium’s dynamic Android version');
  }
  const custom=structuredClone(googlebook);custom.metadata.platformVersion='14.2.0';
  assert.equal(presetClientHints(custom,new Date(2027,9,1))['Sec-CH-UA-Platform-Version'],'"14.2.0"','An explicit platform version became dynamic');
}
