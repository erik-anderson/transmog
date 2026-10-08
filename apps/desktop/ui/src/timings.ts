import type { SessionDetail } from './models.js';
export interface TimingRow {id:string;label:string;value:string;note:string;}
export interface TimelineRow {id:string;label:string;time:string;offset:string;}
export interface TransportView {id:string;title:string;summary:string;fields:TimingRow[];}
const labels:Record<string,string>={
 'client-connected':'Client TCP accepted','client-first-byte':'First client socket read','client-identity-done':'Client identification finished','client-tls-begin':'Client TLS handshake began','client-tls-done':'Client TLS handshake finished','request-headers':'Client request headers received','client-request-done':'Client request body received','route-begin':'Routing began','route-done':'Routing finished','upstream-begin':'Request submitted upstream','upstream-connected':'Upstream connection assigned','upstream-request-consumed':'Outgoing body consumed by HTTP adapter','response-headers':'Upstream response headers received','upstream-response-done':'Upstream response body received','client-response-begin':'Client response headers committed','client-response-queued':'Client response body queued','exchange-done':'Exchange processing finished'};
export function milliseconds(micros:number|null|undefined):string {return micros==null?'Unavailable':(micros/1000).toLocaleString(undefined,{maximumFractionDigits:3})+' ms';}
function bytes(value:number|null|undefined):string {return value==null?'Unavailable':value.toLocaleString()+' bytes';}
export function timingView(detail:SessionDetail):{summary:string;phases:TimingRow[];timeline:TimelineRow[];transports:TransportView[];saved:TimingRow[]} {
 const points=detail.performance?.points??[], transports=detail.performance?.transports??[];
 const point=(id:string)=>points.find(p=>p.milestone===id)?.offsetMicros;
 const span=(a:string,b:string)=>{const begin=point(a),end=point(b);return begin==null||end==null||end<begin?undefined:end-begin;};
 const phases:TimingRow[]=[
  {id:'total',label:'Processing since request headers',value:milliseconds(span('request-headers','exchange-done')),note:'Ends when proxy processing finishes; does not confirm client receipt.'},
  {id:'upload',label:'Client body upload',value:milliseconds(span('request-headers','client-request-done')),note:'May overlap upstream transfer for streaming requests.'},
  {id:'route',label:'Routing decision',value:milliseconds(span('route-begin','route-done')),note:'Local policy selection.'},
  {id:'setup',label:'Upstream assignment wait',value:milliseconds(span('upstream-begin','upstream-connected')),note:'Includes pool queueing and setup when a new connection is needed.'},
  {id:'wait',label:'Response header wait',value:(()=>{const headers=point('response-headers'),assigned=point('upstream-connected'),sent=point('upstream-request-consumed');return milliseconds(headers==null||assigned==null||sent==null||headers<Math.max(assigned,sent)?undefined:headers-Math.max(assigned,sent));})(),note:'After assignment and body consumption. Includes network, server and queueing time.'},
  {id:'receive',label:'Upstream response transfer',value:milliseconds(span('response-headers','upstream-response-done')),note:'Includes backpressure and local body processing.'},
  {id:'forward',label:'Client response queueing',value:milliseconds(span('client-response-begin','client-response-queued')),note:'Includes streaming wait; queued bytes may still be in transport buffers.'}];
 const timeline=points.slice().sort((a,b)=>a.offsetMicros-b.offsetMicros).map(p=>{const date=new Date(p.unixMillis);return {id:p.milestone,label:labels[p.milestone]??p.milestone,time:p.unixMillis>0&&Number.isFinite(date.getTime())?date.toISOString():'Unavailable',offset:milliseconds(p.offsetMicros)};});
 const views=transports.map(t=>{
 const fields:TimingRow[]=[];const add=(label:string,value:string,note='')=>fields.push({id:label,label,value,note});
 const setup=t.setupTimings??[];
 const relative=(offset:number)=>milliseconds(Math.abs(offset))+(offset<0?' before request headers':' after request headers');
 const phaseNote=(phase:string)=>{const timing=setup.find(item=>item.phase===phase);return timing?'Original connection phase '+milliseconds(timing.endedOffsetMicros-timing.beganOffsetMicros)+'. Started '+relative(timing.beganOffsetMicros)+'; finished '+relative(timing.endedOffsetMicros)+'.':'Original setup start was not recorded; a per-request wait cannot be calculated.';};
 if(setup.length){const began=Math.min(...setup.map(item=>item.beganOffsetMicros)),ended=Math.max(...setup.map(item=>item.endedOffsetMicros));add('Connection setup started',relative(began),'Calculated on this request’s zero timestamp, including a reused connection.');add('Connection setup elapsed',milliseconds(ended-began),'Whole connection/session setup, across its measured phases.');}
 add('Peer',t.peer??'Unavailable');add('Local endpoint',t.local??'Unavailable');add(setup.length?'DNS wait for this request':'DNS lookup (connection)',milliseconds(t.dnsMicros),phaseNote('dns'));add(setup.length?'TCP wait for this request':'TCP connect (connection)',milliseconds(t.tcpMicros),phaseNote('tcp'));add((t.quic?'QUIC / TLS':'TLS')+(setup.length?' wait for this request':' handshake (connection)'),milliseconds(t.tlsMicros),phaseNote(t.quic?'quic':'tls'));
 add('TLS version',t.tlsVersion??'Unavailable');add('Session resumed',t.tlsResumed==null?'Unavailable':t.tlsResumed?'Yes':'No');add('Cipher',t.cipher??'Unavailable');add('ALPN',t.alpn??'Unavailable');add('Received',bytes(t.bytesRead));add('Sent',bytes(t.bytesWritten));
 if(t.quic){add('Smoothed round trip',milliseconds(t.quic.rttMicros));add('Congestion window',bytes(t.quic.congestionWindow));add('Packets sent / received',t.quic.packetsSent.toLocaleString()+' / '+t.quic.packetsReceived.toLocaleString());add('Packets declared lost',t.quic.packetsLost.toLocaleString());add('Retransmitted stream data',bytes(t.quic.retransmittedBytes));}
 return {id:t.leg+':'+t.connectionId,title:t.leg==='client'?'Client ↔ proxy':'Proxy ↔ upstream',summary:(t.shared?'Reused / shared connection':'First observed exchange on connection')+' · '+t.outcome.replaceAll('-',' ')+' · sample at '+milliseconds(t.sampledOffsetMicros)+' · '+t.connectionId,fields};});
 const saved=Object.entries(detail.savedEvidence??{}).map(([label,value])=>({id:label,label,value,note:''}));
 return {summary:points.length?'Times use one monotonic clock, relative to complete client request headers. Negative offsets belong to earlier connection events. Wall times are UTC. Setup costs show this request’s wait after upstream admission; completed reused phases cost zero.':'This capture has no measured proxy timeline. Original imported timers, when present, are shown below.',phases,timeline,transports:views,saved};
}
