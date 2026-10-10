import assert from 'node:assert/strict';
import {isIP} from 'node:net';

const fixtureEpoch=Date.parse('2026-01-01T00:00:00.000Z');
const ipv4=/\b(?:\d{1,3}\.){3}\d{1,3}\b/g;
const ipv6=/(?<![\w:])(?:[\da-f]{0,4}:){2,}[\da-f:.]*(?:%[\w.-]+)?/gi;
const paths=/\b[A-Z]:[\\/][^\r\n"<>]+|\/(?:Users|home|tmp|var\/folders)\/[^\r\n"<>]+/gi;

function textSanitizer(port) {
  const addresses=new Map();
  return text=>text.replaceAll(':'+port,':8080').replace(paths,'/fixture/profile')
    .replace(ipv6,address=>{
      const plain=address.split('%')[0];
      if(isIP(plain)!==6)return address;
      if(!addresses.has(address))addresses.set(address,'2001:db8::'+(addresses.size+1));
      return addresses.get(address);
    })
    .replace(ipv4,address=>{
      if(isIP(address)!==4)return address;
      if(!addresses.has(address))addresses.set(address,'192.0.2.'+(addresses.size+1));
      return addresses.get(address);
    });
}

function mapStrings(value,sanitize) {
  if(typeof value==='string')return sanitize(value);
  if(Array.isArray(value))return value.map(item=>mapStrings(item,sanitize));
  if(value && typeof value==='object')return Object.fromEntries(Object.entries(value).map(([key,item])=>[key,mapStrings(item,sanitize)]));
  return value;
}

function audit(document) {
  function check(text) {
    assert.doesNotMatch(text,/\b(?:authorization|proxy-authorization|cookie|set-cookie)\s*:/i);
    assert.doesNotMatch(text,/\b[A-Z]:[\\/]|\/(?:Users|home|tmp|var\/folders)\//i);
    for(const candidate of text.match(ipv4)??[]) {
      if(isIP(candidate)===4)assert.match(candidate,/^192\.0\.2\./);
    }
    for(const candidate of text.match(ipv6)??[]) {
      if(isIP(candidate)===6)assert.match(candidate,/^2001:db8:/i);
    }
    for(const match of text.matchAll(/https?:\/\/([^\s/"';}]+)/g))assert.match(match[1],/^placeholder\.invalid(?::8080)?$/);
  }
  function visit(value,key) {
    if(typeof value==='string') {
      check(value);
      if(key==='bytes')check(Buffer.from(value,'base64').toString('utf8'));
    } else if(Array.isArray(value))for(const item of value)visit(item,key);
    else if(value && typeof value==='object')for(const [name,item]of Object.entries(value))visit(item,name);
  }
  visit(document);
}

export function sanitizeNetLog(raw,port) {
  const constants=structuredClone(raw.constants);
  delete constants.clientInfo;
  delete constants.activeFieldTrialGroups;
  const relevant=new Set(raw.events.filter(event=>event.params?.url?.startsWith(`http://placeholder.invalid:${port}/`)).map(event=>event.source.id));
  assert.ok(relevant.size>0,'No placeholder requests were captured');
  function dependencies(value) {
    if(Array.isArray(value))for(const item of value)dependencies(item);
    else if(value && typeof value==='object') {
      if(Number.isInteger(value.id) && Number.isInteger(value.type))relevant.add(value.id);
      for(const item of Object.values(value))dependencies(item);
    }
  }
  let previousSize;
  do {
    previousSize=relevant.size;
    for(const event of raw.events)if(relevant.has(event.source.id))dependencies(event.params);
  } while(relevant.size!==previousSize);
  const selected=raw.events.filter(event=>relevant.has(event.source.id));
  const base=Math.min(...selected.flatMap(event=>[Number(event.time),Number(event.source.start_time)]).filter(Number.isFinite));
  const sanitize=textSanitizer(port);
  const events=selected.map(original=>{
    const event=mapStrings(original,sanitize);
    event.time=String(Number(original.time)-base);
    event.source.start_time=String(Number(original.source.start_time)-base);
    if(original.params?.bytes) {
      const bytes=Buffer.from(original.params.bytes,'base64');
      const text=bytes.toString('utf8');
      assert.ok(Buffer.from(text,'utf8').equals(bytes),'Fixture byte events must contain the controlled UTF-8 messages');
      const replacement=Buffer.from(sanitize(text));
      event.params.bytes=replacement.toString('base64');
      event.params.byte_count=replacement.length;
    }
    return event;
  });
  constants.timeTickOffset=String(fixtureEpoch);
  const result={constants,events,polledData:[]};
  audit(result);
  return result;
}

export function sanitizeHar(raw,port) {
  const result=mapStrings(raw,textSanitizer(port));
  const base=Math.min(...raw.log.entries.map(entry=>Date.parse(entry.startedDateTime)));
  const pages=new Map();
  for(const [index,page]of result.log.pages.entries()) {
    pages.set(page.id,'page_'+(index+1));
    page.id='page_'+(index+1);
    page.startedDateTime=new Date(fixtureEpoch+Date.parse(page.startedDateTime)-base).toISOString();
  }
  for(const entry of result.log.entries) {
    entry.pageref=pages.get(entry.pageref);
    entry._frameref='frame_1';
    entry.startedDateTime=new Date(fixtureEpoch+Date.parse(entry.startedDateTime)-base).toISOString();
    assert.deepEqual(entry.request.cookies,[]);
    assert.deepEqual(entry.response.cookies,[]);
    for(const message of [entry.request,entry.response])for(const header of message.headers)assert.doesNotMatch(header.name,/^(?:authorization|proxy-authorization|cookie|set-cookie)$/i);
  }
  audit(result);
  return result;
}
