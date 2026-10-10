import assert from 'node:assert/strict';
import {test} from 'node:test';
import {sanitizeHar,sanitizeNetLog} from './sanitize-browser-archives.mjs';

test('NetLog removes machine snapshots and sanitizes endpoints and encoded message bytes',()=>{
  const payload=Buffer.from('GET http://placeholder.invalid:34567/ HTTP/1.1\r\nX-Address: 10.42.0.11\r\nX-Profile: C:\\Users\\FixturePerson\\Desktop\\profile\r\n\r\n');
  const event=(id,time,params)=>({source:{id,type:id,start_time:'1000'},time:String(time),type:1,phase:0,params});
  const raw={
    constants:{clientInfo:{command_line:'C:\\Users\\FixturePerson\\profile'},activeFieldTrialGroups:['private-group']},
    polledData:[{dns:{search:['private.corp'],address:'192.168.1.40'}}],
    events:[
      event(0,1000,{search:['private.corp'],address:'192.168.1.40'}),
      event(1,1000,{url:'http://placeholder.invalid:34567/',source_dependency:{id:2,type:2}}),
      event(2,1001,{local_address:'192.168.1.40:49152',remote_address:'[fe80::2%adapter]:34567',bytes:payload.toString('base64'),byte_count:payload.length}),
      event(1,1002,{}),
    ],
  };
  const clean=sanitizeNetLog(raw,34567);
  assert.deepEqual(clean.events.map(event=>event.time),['0','1','2']);
  assert.equal(clean.constants.timeTickOffset,String(Date.parse('2026-01-01T00:00:00.000Z')));
  assert.equal(clean.constants.clientInfo,undefined);
  assert.deepEqual(clean.polledData,[]);
  const socket=clean.events[1].params;
  assert.match(socket.local_address,/^192\.0\.2\./);
  assert.match(socket.remote_address,/^\[2001:db8::\d+\]:8080$/);
  const decoded=Buffer.from(socket.bytes,'base64');
  assert.equal(socket.byte_count,decoded.length);
  assert.match(decoded.toString(),/placeholder\.invalid:8080/);
  assert.match(decoded.toString(),/X-Profile: \/fixture\/profile/);
  assert.doesNotMatch(JSON.stringify(clean)+decoded.toString(),/FixturePerson|private\.corp|192\.168\.1\.40|10\.42\.0\.11|fe80:/);
});

function har() {
  return {log:{pages:[{id:'random-page',startedDateTime:'2026-10-10T02:00:00.000Z'}],entries:[{
    pageref:'random-page',_frameref:'random-frame',startedDateTime:'2026-10-10T02:00:00.000Z',
    request:{url:'http://placeholder.invalid:34567/',headers:[],cookies:[]},
    response:{headers:[],cookies:[],content:{text:'controlled response'}},serverIPAddress:'192.168.1.40',
  }]}};
}

test('HAR replaces addresses and generated identities while retaining response content',()=>{
  const clean=sanitizeHar(har(),34567);
  const entry=clean.log.entries[0];
  assert.equal(entry.serverIPAddress,'192.0.2.1');
  assert.equal(entry.request.url,'http://placeholder.invalid:8080/');
  assert.equal(entry.pageref,'page_1');
  assert.equal(entry._frameref,'frame_1');
  assert.equal(entry.startedDateTime,'2026-01-01T00:00:00.000Z');
  assert.equal(entry.response.content.text,'controlled response');
});

test('Fixture generation rejects credential headers instead of publishing them',()=>{
  const raw=har();
  raw.log.entries[0].request.headers.push({name:'Authorization',value:'private credential'});
  assert.throws(()=>sanitizeHar(raw,34567));
});
