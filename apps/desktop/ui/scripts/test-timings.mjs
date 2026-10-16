import assert from 'node:assert/strict';
import test from 'node:test';
import {fileURLToPath} from 'node:url';
import {build} from 'esbuild';

const output=await build({entryPoints:[fileURLToPath(new URL('../src/timings.ts',import.meta.url))],bundle:true,platform:'node',format:'esm',write:false});
const {timingView}=await import('data:text/javascript;base64,'+Buffer.from(output.outputFiles[0].text).toString('base64'));
const detail=points=>({requests:[],responses:[],id:'timing-test',performance:{points:Object.entries(points).map(([milestone,offsetMicros])=>({milestone,offsetMicros,unixMillis:1800000000000+offsetMicros/1000})),transports:[],work:[]}});

test('wait begins after both connection assignment and body consumption; views share names and values',()=>{
  const view=timingView(detail({'request-headers':0,'route-begin':0,'route-done':0,'upstream-begin':500,'upstream-connected':10000,'upstream-request-consumed':15000,'response-headers':80000,'upstream-response-done':100000,'client-response-begin':81000,'client-response-queued':101000,'exchange-done':102000}));
  assert.equal(view.phases.find(row=>row.id==='wait').value,'65 ms');
  assert.equal(view.phases.find(row=>row.id==='route').value,'0 ms');
  for(const row of [...view.waterfall,...view.minor]) {
    const phase=view.phases.find(phase=>phase.id===row.id);
    assert.equal(row.label,phase.label);assert.equal(row.value,phase.value);
    assert.ok(view.report.includes(phase.label+': '+phase.value));
  }
  assert.match(view.phases.find(row=>row.id==='total').note,/local proxy completion/);
  assert.match(view.phases.find(row=>row.id==='forward').note,/delivery may finish later/);
});

test('early responses and missing observations stay unavailable, with original imported names',()=>{
  const data=detail({'request-headers':0,'upstream-connected':10000,'upstream-request-consumed':20000,'response-headers':5000});
  data.savedEvidence={'HAR wait':'4.5','ClientBeginRequest':'00:00:00.000'};
  const view=timingView(data);
  assert.equal(view.phases.find(row=>row.id==='wait').value,'Unavailable');
  assert.equal(view.phases.find(row=>row.id==='total').value,'Unavailable');
  assert.equal(view.waterfall.some(row=>row.id==='wait'),false);
  assert.equal(view.saved[0].label,'HAR wait');assert.match(view.report,/ClientBeginRequest: 00:00:00.000/);
});
