import type {ExtensionContribution} from '../extension-settings/models.js';

export type Presentation='tree'|'table'|'text'|'hex';
export interface VisualizerChoice {extensionId:string;visualizerId:string;name:string;key:string;}
export interface VisualizerSelection {sessionId:string;boundary:string;surface:'headers'|'body';}
export interface VisualizerHandle {slot:string;generation:string;instanceId:string;pageId:string;}
export interface Span {sourceId:string;offset:string;length:string;}
export interface Format {protocol?:string|null;charset?:string|null;byteOrder?:string|null;radix?:number|null;encoding?:string|null;units?:string|null;}
export type Value={kind:'string'|'integer'|'timestamp'|'guid';value:string}|{kind:'float';value:number}|{kind:'boolean';value:boolean}|{kind:'enum';value:{name:string;raw:string}}|{kind:'flags';value:{names:string[];raw:string}}|{kind:'bytes'|'object'|'array'};
export interface VisualizerNode {id:string;name:string;value?:Value|null;format?:Format|null;spans:Span[];children:VisualizerNode[];childrenToken?:string|null;presentation?:Presentation|null;}
export interface DerivedSource {id:string;name:string;bytesBase64:string;parents:Span[];transform:string;exactMapping:boolean;}
export interface SourceInfo {id:string;name:string;length:string;availability:string;sessionId:string;boundary:string;httpDecoded?:{parent:Span;codings:string[]};}
export const sourceDescription=(source:SourceInfo):string=>`${source.length} bytes · ${source.availability}`+(source.httpDecoded?` · HTTP content decoding (${source.httpDecoded.codings.join(', ')}) · enclosing parent regions only`:'');
export interface VisualizerDocument {title:string;status:string;presentation:Presentation;summary:string|null;roots:VisualizerNode[];sources:DerivedSource[];diagnostics:Array<{code:string;message:string;nodeId:string|null}>;}
export interface LiveStatus {available:boolean;enabled:boolean;missedHints:string;}
export interface LiveUpdate {view:VisualizerView|null;status:LiveStatus;busy:boolean;}
export interface VisualizerView {handle:VisualizerHandle;instance:{id:string;extensionId:string;visualizerId:string;digest:string;development?:boolean;declaration:ExtensionContribution};document:VisualizerDocument;sources:SourceInfo[];live:LiveStatus;}
export interface SourcePage {sourceId:string;totalBytes:string;offset:string;bytesBase64:string;}
export interface TreeRow {id:string;domId:string;parent:string;level:number;position:number;setSize:number;indent:string;top:string;name:string;value:string;format:string;expandable:boolean;expanded:boolean;expandedState:string|null;selected:boolean;toggleLabel:string;}
export const DOCUMENT_NODE='$document';

/** Wide protocol numbers remain text; formatting never rounds through Number. */
export function valueText(value:Value|null|undefined):string {
  if(!value)return '';
  switch(value.kind){
    case 'bytes':return 'Bytes';case 'object':return 'Object';case 'array':return 'Array';
    case 'enum':return `${value.value.name} (${value.value.raw})`;
    case 'flags':return `${value.value.names.join(' | ')||'No flags'} (${value.value.raw})`;
    default:return String(value.value);
  }
}
export function formatText(format:Format|null|undefined):string {
  return format?[format.protocol,format.charset,format.byteOrder,format.radix?`Base ${format.radix}`:'',format.encoding,format.units].filter(Boolean).join(' · '):'';
}
export function documentNode(document:VisualizerDocument):VisualizerNode {
  return {id:DOCUMENT_NODE,name:document.title,children:document.roots,spans:[],presentation:document.presentation};
}
export function nodeIndex(root:VisualizerNode):Map<string,VisualizerNode> {
  const index=new Map<string,VisualizerNode>();
  const visit=(node:VisualizerNode):void=>{index.set(node.id,node);for(const child of node.children)visit(child);};visit(root);return index;
}
export function treeRows(root:VisualizerNode,expanded:Set<string>,selected:string,prefix:string):TreeRow[] {
  const rows:TreeRow[]=[];
  const visit=(node:VisualizerNode,parent:string,level:number,position:number,setSize:number):void=>{
    const expandable=!!(node.children.length||node.childrenToken),open=expanded.has(node.id);
    rows.push({id:node.id,domId:prefix+'-'+node.id,parent,level,position,setSize,indent:(level-1)*16+'px',top:rows.length*26+'px',name:node.name,value:valueText(node.value).slice(0,512),format:formatText(node.format),expandable,expanded:open,expandedState:expandable?String(open):null,selected:selected===node.id,toggleLabel:(open?'Collapse ':'Expand ')+node.name});
    if(open)node.children.forEach((child,i)=>visit(child,node.id,level+1,i+1,node.children.length));
  };visit(root,'',1,1,1);return rows;
}
export interface TableRow {id:string;name:string;cells:string[];}
export function tablePage(node:VisualizerNode,page:number):{columns:Array<{id:string;name:string}>;rows:TableRow[];text:string;previous:boolean;next:boolean;limited:boolean} {
  const structured=node.children.some(child=>child.children.length);
  const columns:Array<{id:string;name:string}>=[];
  let limited=false;
  if(structured){
    const keys=new Set<string>();
    for(const row of node.children){const counts=new Map<string,number>();for(const field of row.children){const ordinal=counts.get(field.name)??0;counts.set(field.name,ordinal+1);const id=JSON.stringify([field.name,ordinal]);if(!keys.has(id)){if(columns.length===64){limited=true;continue;}keys.add(id);columns.push({id,name:field.name});}}}
  }else columns.push({id:'value',name:'Value'},{id:'format',name:'Format'});
  const offset=Math.max(0,Math.min(page,Math.max(0,Math.ceil(node.children.length/50)-1)))*50;
  const rows=node.children.slice(offset,offset+50).map(row=>{
    const values=new Map<string,string>(),counts=new Map<string,number>();
    for(const field of row.children){const ordinal=counts.get(field.name)??0;counts.set(field.name,ordinal+1);values.set(JSON.stringify([field.name,ordinal]),valueText(field.value));}
    return {id:row.id,name:row.name,cells:structured?columns.map(column=>(values.get(column.id)??'').slice(0,512)):[valueText(row.value).slice(0,512),formatText(row.format)]};
  });
  return {columns,rows,text:node.children.length?`${offset+1}–${offset+rows.length} of ${node.children.length} rows`:'No child rows',previous:offset>0,next:offset+50<node.children.length,limited};
}
