import type {TrafficTimelineRow} from '../models.js';

/** Compact chart labels; inspection details retain the exact measured value. */
export function timelineTime(micros:number|null):string {
  if(micros==null)return 'Unavailable';
  const [scale,unit]:[number,string]=Math.abs(micros)>=3_600_000_000?[3_600_000_000,'h']:Math.abs(micros)>=60_000_000?[60_000_000,'min']:Math.abs(micros)>=1_000_000?[1_000_000,'s']:[1000,'ms'];
  return (micros/scale).toLocaleString(undefined,{maximumFractionDigits:3})+' '+unit;
}
export function requestAddress(url:string):{path:string;host:string} {
  try {const address=new URL(url);return {path:address.pathname+address.search+address.hash||'/',host:address.host};}
  catch {return {path:url,host:''};}
}

export function intervalEnd(row:TrafficTimelineRow):number|null {
  return row.startMillis==null||row.durationMicros==null?null:row.startMillis+row.durationMicros/1000;
}
export function overlaps(row:TrafficTimelineRow,from:number,to:number):boolean {
  const end=intervalEnd(row);
  // Unplaceable evidence remains visible and explicitly unavailable.
  return row.startMillis==null||end==null||row.startMillis<=to&&end>=from;
}
export function clampWindow(from:number,to:number,minimum:number,maximum:number,part:string,delta:number):[number,number] {
  const smallest=Math.min(1,Math.max(.001,(maximum-minimum)/1000));
  if(part==='start')return [Math.max(minimum,Math.min(to-smallest,from+delta)),to];
  if(part==='end')return [from,Math.min(maximum,Math.max(from+smallest,to+delta))];
  const movement=Math.max(minimum-from,Math.min(maximum-to,delta));
  return [from+movement,to+movement];
}
