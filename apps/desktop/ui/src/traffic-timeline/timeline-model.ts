import type {TrafficTimelineRow} from '../models.js';

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
