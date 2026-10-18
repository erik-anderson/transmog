import type {ColumnId,TrafficFilter,TrafficFilterGroup,TrafficFilterValue,TrafficDimension} from './models.js';
import {columnDefinitions,stateText} from './table-model.js';
import {localDateTime} from './traffic-ranges.js';
import initialState from './initial-state.json';

export interface FilterChoice {key:string;label:string;value:TrafficFilterValue;}
export const filterDimensions:Array<{id:TrafficDimension;label:string;numeric:boolean;placeholder:string}>= [
  {id:'traffic-type',label:'Traffic type',numeric:false,placeholder:'HTTP, WebSocket or SSE'},
  ...['method','status','state','protocol'].map(id=>columnDefinitions.find(column=>column.id===id)!),
  {id:'fetch-destination',label:'Fetch destination',numeric:false,placeholder:'Sec-Fetch-Dest'},
  ...columnDefinitions.filter(column=>!['method','status','state','protocol'].includes(column.id)),
];
export function filterValueKey(value:TrafficFilterValue):string {return value.kind==='unavailable'?'unavailable':value.kind+':'+String(value.value).toLowerCase();}
export function filterLabel(dimension:TrafficDimension):string {return filterDimensions.find(field=>field.id===dimension)?.label??dimension;}
const text=(value:string,label=value):FilterChoice=>({key:'text:'+value.toLowerCase(),label,value:{kind:'text',value:value.toLowerCase()}});
export function fixedFilterChoices(dimension:TrafficDimension):FilterChoice[] {
  switch(dimension) {
    case 'process':return [{key:'unavailable',label:'No process name',value:{kind:'unavailable'}}];
    case 'content-type':return [{key:'unavailable',label:'Missing content type',value:{kind:'unavailable'}}];
    case 'stream-sent':case 'stream-received':case 'duration':case 'http-duration':case 'started-at':return [{key:'unavailable',label:'Unavailable',value:{kind:'unavailable'}}];
    case 'traffic-type':return [text('http','HTTP'),text('websocket','WebSocket'),text('sse','SSE')];
    case 'method':return ['GET','POST','PUT','PATCH','DELETE','HEAD','OPTIONS','CONNECT','TRACE'].map(value=>text(value));
    case 'state':return ['active','completed','cancelled','failed','interrupted','open-at-capture','incomplete'].map(value=>text(value,stateText(value as Parameters<typeof stateText>[0])));
    case 'protocol':return ['HTTP/1.0','HTTP/1.1','HTTP/2','HTTP/3'].map(value=>text(value));
    case 'fetch-destination':return initialState.fetchDestinationOptions.map(option=>text(option.value,option.label));
    case 'status':return [{key:'unavailable',label:'Unavailable status',value:{kind:'unavailable'}},...[1,2,3,4,5].map(value=>({key:'status-family:'+value,label:value+'xx',value:{kind:'status-family' as const,value}})),...[101,200,201,204,206,301,302,304,307,308,400,401,403,404,405,408,409,413,415,422,429,500,502,503,504].map(value=>({key:'number:'+value,label:String(value),value:{kind:'number' as const,value:String(value)}}))];
    default:return [];
  }
}
export function valueLabel(dimension:TrafficDimension,value:TrafficFilterValue):string {
  const fixed=fixedFilterChoices(dimension).find(choice=>choice.key===filterValueKey(value));
  if(fixed)return fixed.label;
  if(value.kind==='text'&&value.value==='')return '(Empty value)';
  return value.kind==='unavailable'?'Unavailable':value.kind==='status-family'?value.value+'xx':String(value.value);
}
export interface FilterChip {dimension:TrafficDimension;label:string;}
export function trafficFilterChips(filters:TrafficFilter[],groups:TrafficFilterGroup[],destinations:string[]):FilterChip[] {
  return filterDimensions.flatMap(field=>{
    const parts=[...groups.filter(group=>group.dimension===field.id).flatMap(group=>group.values.length?[group.values.map(value=>valueLabel(field.id,value)).join(' or ')]:[]),...filters.filter(filter=>filter.column===field.id).map(filter=>({contains:'contains',equals:'=',minimum:'≥',maximum:'≤'}[filter.operator])+' '+(field.id==='started-at'?(localDateTime(filter.value).replace('T',' ')||filter.value+' Unix ms'):filter.value+(['duration','http-duration'].includes(field.id)?' ms':['request-bytes','response-bytes','stream-sent','stream-received'].includes(field.id)?' B':'')))];
    if(field.id==='fetch-destination'&&destinations.length)parts.push(destinations.map(value=>valueLabel(field.id,{kind:'text',value})).join(' or '));
    return parts.length?[{dimension:field.id,label:field.label+': '+parts.join(' · ')}]:[];
  });
}
export function columnDimension(dimension:TrafficDimension):ColumnId|null {return columnDefinitions.some(column=>column.id===dimension)?dimension as ColumnId:null;}
