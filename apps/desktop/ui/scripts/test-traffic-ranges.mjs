import assert from 'node:assert/strict';
import test from 'node:test';
import {fileURLToPath} from 'node:url';
import {build} from 'esbuild';
const output=await build({entryPoints:[fileURLToPath(new URL('../src/traffic-ranges.ts',import.meta.url))],bundle:true,platform:'node',format:'esm',write:false});
const {decimalToBase,localDateTime,localToMillis,inclusiveRange}=await import('data:text/javascript;base64,'+Buffer.from(output.outputFiles[0].text).toString('base64'));
test('unit conversion is exact and preserves the entire unsigned range',()=>{
 assert.equal(decimalToBase('1.001','s'),'1001');assert.equal(decimalToBase('0.5','KiB'),'512');
 assert.equal(decimalToBase('18446744073709551615','1',true),'18446744073709551615');
 for(const value of ['','-1','NaN','Infinity','1e3','1.0001'])assert.throws(()=>decimalToBase(value,'s'));
 assert.throws(()=>decimalToBase('18446744073709551616','B'));assert.throws(()=>decimalToBase('0','1',true));assert.throws(()=>decimalToBase('0.1','B'));
});
test('ranges are inclusive, independently open and reject reversed bounds',()=>{
 assert.deepEqual(inclusiveRange('duration','0','1.5','s').map(({operator,value})=>({operator,value})),[{operator:'minimum',value:'0'},{operator:'maximum',value:'1500'}]);
 assert.equal(inclusiveRange('request-number','9007199254740993','','1')[0].value,'9007199254740993');
 assert.deepEqual(inclusiveRange('response-bytes','','','B'),[]);assert.throws(()=>inclusiveRange('duration','2','1','s'));
});
test('local dates roundtrip exact milliseconds, leap dates and DST boundaries',()=>{
 const prior=process.env.TZ;process.env.TZ='America/Los_Angeles';
 try{
  const stamp=Date.UTC(2026,9,17,20,12,34,567);assert.equal(localToMillis(localDateTime(String(stamp))),String(stamp));
  assert.throws(()=>localToMillis('2026-02-29T10:00'));assert.throws(()=>localToMillis('2026-03-08T02:30'));
  assert.equal(localToMillis('2026-11-01T01:30'),String(Date.UTC(2026,10,1,8,30)));
  const later=String(Date.UTC(2026,10,1,9,30)),text=localDateTime(later);
  assert.equal(inclusiveRange('started-at',text,'','1',{from:{text,value:later}})[0].value,later);
  assert.equal(inclusiveRange('started-at',text,'','1')[0].value,String(Date.UTC(2026,10,1,8,30)));
  assert.equal(localToMillis('2024-02-29T12:00:00.001'),String(Date.UTC(2024,1,29,20,0,0,1)));
  assert.throws(()=>localToMillis('1960-01-01T00:00'));assert.equal(localDateTime('18446744073709551615'),'');
 }finally{if(prior===undefined)delete process.env.TZ;else process.env.TZ=prior;}
});
