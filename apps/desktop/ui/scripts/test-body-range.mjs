import assert from 'node:assert/strict';
import test from 'node:test';
import {build} from 'esbuild';

const output=await build({entryPoints:[new URL('../src/body-range.ts',import.meta.url).pathname.replace(/^\/([A-Za-z]:)/,'$1')],bundle:true,platform:'node',format:'esm',write:false});
const {decodeBodyPage,readBodyRange}=await import('data:text/javascript;base64,'+Buffer.from(output.outputFiles[0].text).toString('base64'));
const metadata={lengthKnown:true,exchangeId:'body',boundary:'client-response',observedBytes:8,retainedBytes:8,wireBodyBytes:null,availability:'complete',mediaType:'application/octet-stream',charset:null,contentCodings:['gzip'],sha256:null,reason:null};
const header={metadata,totalBytes:'8',offset:'2',readLimit:65536};
function frame(header,bytes=[0,255,128,13,10]) {
  const text=new TextEncoder().encode(JSON.stringify(header)),result=new Uint8Array(4+text.length+bytes.length);
  new DataView(result.buffer).setUint32(0,text.length,true);result.set(text,4);result.set(bytes,4+text.length);return result.buffer;
}

test('body IPC preserves binary bytes and borrows the ArrayBuffer, including empty pages',()=>{
  const buffer=frame(header),page=decodeBodyPage(buffer,'2',5);
  assert.deepEqual(Array.from(page.bytes),[0,255,128,13,10]);assert.equal(page.bytes.buffer,buffer);
  assert.deepEqual(page.metadata,metadata);
  const empty=decodeBodyPage(frame({...header,offset:'0',totalBytes:'0',metadata:{...metadata,retainedBytes:0}},[]),'0',5);
  assert.equal(empty.bytes.length,0);
});

test('page offsets beyond JavaScript integer precision remain exact',()=>{
  const total='9007199254741009',offset='9007199254740994';
  const page=decodeBodyPage(frame({...header,totalBytes:total,offset,metadata:{...metadata,retainedBytes:Number(total)}}),offset,5);
  assert.equal(page.offset,offset);assert.equal(page.totalBytes,total);
  assert.throws(()=>decodeBodyPage(frame({...header,offset:'18446744073709551616'}),'18446744073709551616',5));
});

test('malformed frames, mismatched ranges and excessive budgets are rejected',()=>{
  for(const buffer of [null,{},new Uint8Array(8),new ArrayBuffer(3),new ArrayBuffer(4),frame(null),frame({...header,metadata:null})])assert.throws(()=>decodeBodyPage(buffer,'2',5));
  const broken=frame(header);new DataView(broken).setUint32(0,broken.byteLength,true);assert.throws(()=>decodeBodyPage(broken,'2',5));
  const invalidUtf8=frame(header);new Uint8Array(invalidUtf8)[4]=255;assert.throws(()=>decodeBodyPage(invalidUtf8,'2',5));
  for(const patch of [{offset:'3'},{offset:'02'},{totalBytes:'6'},{totalBytes:'1'},{readLimit:4},{readLimit:0},{readLimit:16777217},{metadata:{...metadata,retainedBytes:9}},{metadata:{...metadata,contentCodings:[2]}}])assert.throws(()=>decodeBodyPage(frame({...header,...patch}),'2',5));
  for(const budget of [0,4,16777217,NaN])assert.throws(()=>decodeBodyPage(frame(header),'2',budget));
  assert.throws(()=>decodeBodyPage(frame(header,[]),'2',5));
  assert.throws(()=>decodeBodyPage(frame({...header,metadata:{...metadata,reason:'x'.repeat(262144)}}),'2',5));
});

test('availability must be a string token, without coercion from malformed metadata',()=>{
  for(const availability of [['complete'],['capturing'],{toString:'complete'},null,2]) {
    assert.throws(()=>decodeBodyPage(frame({...header,metadata:{...metadata,availability}}),'2',5),/body byte page is invalid/);
  }
});

test('body reads use one raw Tauri response and verify its source identity',async()=>{
  const calls=[];
  globalThis.window={__TAURI_INTERNALS__:{invoke:async(command,args)=>{calls.push({command,args});return frame(header);}}};
  try {
    const page=await readBodyRange('body','client-response','2',5);
    assert.deepEqual(Array.from(page.bytes),[0,255,128,13,10]);
    assert.deepEqual(calls,[{command:'read_body_range',args:{request:{sessionId:'body',boundary:'client-response',offset:'2',maxBytes:5}}}]);
    await assert.rejects(readBodyRange('different','client-response','2',5),/requested source/);
    await assert.rejects(readBodyRange('body','client-request','2',5),/requested source/);
  } finally {delete globalThis.window;}
});
