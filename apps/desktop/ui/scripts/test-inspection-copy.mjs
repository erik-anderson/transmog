import assert from 'node:assert/strict';
import test from 'node:test';
import {build} from 'esbuild';
const output=await build({entryPoints:[new URL('../src/inspection-copy.ts',import.meta.url).pathname.replace(/^\/([A-Za-z]:)/,'$1')],bundle:true,platform:'node',format:'esm',write:false});
const {copyScalar,copyTree,boundedCopy,fieldCopyActions,COPY_TEXT_BYTES}=await import('data:text/javascript;base64,'+Buffer.from(output.outputFiles[0].text).toString('base64'));
test('clipboard scalar serialization preserves empty, Unicode, wide integers and escaped controls',()=>{
  assert.equal(copyScalar('hello 🌍'), 'hello 🌍');
  assert.equal(copyScalar('a\n\t\\n\r\0'), 'a\\n\\t\\\\n\\r\\u0000');
  assert.equal(copyScalar('9007199254740993'),'9007199254740993');
});
test('subtrees keep duplicate names, order, empty values and two-space nesting',()=>{
  assert.equal(copyTree({name:'Root',children:[{name:'A',value:''},{name:'A',children:[{name:'Bytes',value:'00 0A FF'}]}]}),'Root\n  A: \n  A\n    Bytes: 00 0A FF');
});
test('copy fails as a whole on output and recursive limits',()=>{
  assert.throws(()=>boundedCopy('é'.repeat(COPY_TEXT_BYTES/2+1)),/limit/);
  assert.throws(()=>copyTree({name:'root',children:Array.from({length:10000},()=>({name:'x'}))}),/node limit/);
  let root={name:'leaf'};for(let i=0;i<34;i++)root={name:'parent',children:[root]};assert.throws(()=>copyTree(root),/depth/);
});
test('field actions read the real value on demand and disable unavailable values',async()=>{
  let reads=0;const actions=fieldCopyActions('Name',()=>{reads++;return 'full\nvalue';});
  assert.equal(reads,0);assert.equal(await actions[0].read(()=>true),'Name');assert.equal(reads,0);
  assert.equal(await actions[2].read(()=>true),'Name: full\\nvalue');assert.equal(reads,1);
  assert.deepEqual(fieldCopyActions('Redacted field',null).map(action=>!!action.disabled),[false,true,true]);
});
