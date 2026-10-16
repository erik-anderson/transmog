import assert from 'node:assert/strict';
import test from 'node:test';
import {fileURLToPath} from 'node:url';
import {build} from 'esbuild';
const compiled=await build({entryPoints:[fileURLToPath(new URL('../src/traffic-timeline/timeline-model.ts',import.meta.url))],bundle:true,platform:'node',format:'esm',write:false});
const {overlaps,clampWindow,timelineTime,requestAddress}=await import('data:text/javascript;base64,'+Buffer.from(compiled.outputFiles[0].text).toString('base64'));

test('chart labels keep zero and unknown distinct and use readable units',()=>{
  assert.equal(timelineTime(null),'Unavailable');assert.equal(timelineTime(0),'0 ms');
  assert.equal(timelineTime(1250),'1.25 ms');assert.equal(timelineTime(1200000),'1.2 s');
  assert.equal(timelineTime(90_000_000),'1.5 min');assert.equal(timelineTime(7_200_000_000),'2 h');
  assert.deepEqual(requestAddress('https://example.test:8443/api/items?q=%2F'),{path:'/api/items?q=%2F',host:'example.test:8443'});
  assert.deepEqual(requestAddress('Unavailable'),{path:'Unavailable',host:''});
});
test('time filtering includes overlapping requests, boundary points, and unavailable evidence',()=>{
  assert.equal(overlaps({startMillis:10,durationMicros:100000},50,70),true);
  assert.equal(overlaps({startMillis:10,durationMicros:10000},50,70),false);
  assert.equal(overlaps({startMillis:70,durationMicros:0},50,70),true);
  assert.equal(overlaps({startMillis:null,durationMicros:null},50,70),true);
});
test('panning keeps width at both boundaries and handles cannot cross',()=>{
  assert.deepEqual(clampWindow(20,40,0,100,'window',90),[80,100]);
  assert.deepEqual(clampWindow(20,40,0,100,'window',-90),[0,20]);
  const [from,to]=clampWindow(20,40,0,100,'start',90);assert.ok(from<to);assert.equal(to,40);
  const [start,end]=clampWindow(20,40,0,100,'end',-90);assert.ok(start<end);assert.equal(start,20);
});
