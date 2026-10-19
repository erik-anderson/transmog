import {DraftBaseline} from '../draft-state.js';
import {sameRuleContent} from '../automation-state.js';
import '../draft-status/draft-status.js';
import '../rule-activation/rule-activation.js';
import {attr,observable} from '@microsoft/webui-framework';
import {invoke} from '@tauri-apps/api/core';
import '../match-editor/match-editor.js';
import type {MatchEditor} from '../match-editor/match-editor.js';
import type {AutomationRule,AutomationStatus,HeaderOperation,HeaderPredicate,HeaderRuleTest,RawHeaderField,SelectedResponse,UrlCondition,TextTransformTest} from '../models.js';
import {bodyEditSummary} from '../text-edit-report.js';
import {WorkspaceElement} from '../workspace-element.js';
import {describeError,parseHeaderLines,encodeHeaders} from '../utilities.js';

type Phase='request'|'response';
type ValueEncoding='text'|'hex';
type OperationRow={id:string;operation:HeaderOperation['operation'];name:string;encoding:ValueEncoding;value:string};
type PredicateRow={id:string;phase:Phase;name:string;kind:HeaderPredicate['condition']['kind'];encoding:ValueEncoding;value:string};
type RuleRow={id:string;name:string;phase:string;state:string;selected:boolean;first:boolean;last:boolean;};
const emptyActions=()=>({headers:[],replaceBody:null,discardBody:false,abortReason:null});
export function headerRulePhase(rule:AutomationRule):Phase|null {
 const q=rule.request,s=rule.response;
 if(s.textTransform||q.userAgent||q.responseAsset||q.replaceBody!==null||q.discardBody||q.abortReason||s.replaceBody!==null||s.discardBody||s.abortReason)return null;
 if(q.headers.length&&!s.headers.length&&!rule.matcher.responseHeaders.length&&rule.matcher.responseStatus===null&&rule.matcher.responseStatusClass===null)return 'request';
 return !q.headers.length&&s.headers.length?'response':null;
}
function bytes(value:string,encoding:ValueEncoding):number[]{
 if(encoding==='text')return [...new TextEncoder().encode(value)];
 const raw=value.replace(/\s/g,'');if(!/^(?:[\da-f]{2})*$/i.test(raw))throw new Error('Hex values require complete byte pairs (00–FF), separated only by whitespace.');return raw.match(/../g)?.map(pair=>Number.parseInt(pair,16))??[];
}
function display(value:number[]):{encoding:ValueEncoding;value:string}{
 try {const text=new TextDecoder('utf-8',{fatal:true,ignoreBOM:true}).decode(new Uint8Array(value));if(/[\u0000-\u0008\u000a-\u001f\u007f]/.test(text))throw new Error();return {encoding:'text',value:text};}catch{return {encoding:'hex',value:value.map(b=>b.toString(16).padStart(2,'0')).join(' ')};}
}
const id=()=>crypto.randomUUID();

export class HeaderRules extends WorkspaceElement {
 @attr({mode:'boolean'}) embedded=false;
 @attr({attribute:'editor-kind'}) editorKind='headers';
 @observable hrText=false;
 @observable hrTitle='Change headers';
 @observable hrNewLabel='New header rule';
 @observable hrFind='original';
 @observable hrReplacement='modified';
 @observable hrOccurrences='all';
 @observable hrExpected='';
 @observable hrMedia='text/html';
 @observable hrMaxBytes='1048576';
 @observable hrSampleBody='original token=keep original';
 @observable hrBeforeText='';
 @observable hrAfterText='';
 @observable hrBodySummary='';
 @observable headerState:AutomationStatus|null=null;
 @observable selection:SelectedResponse|null=null;
 @attr({attribute:'active',mode:'boolean'}) hrActive=false;
 @observable hrRows:RuleRow[]=[];
 @observable hrOrder:Array<{id:string;name:string;kind:string;state:string}>=[];
 @observable hrEditor=false;
 @observable hrId='';
 @observable hrName='';
 @observable hrPhase:Phase='response';
 @observable hrMethod='GET';
 @observable hrCondition:UrlCondition|null={kind:'exact',value:''};
 @observable hrEnabled=false;
 @observable hrPlacement='keep';
 @observable hrOperations:OperationRow[]=[];
 @observable hrPredicates:PredicateRow[]=[];
 @observable hrStatusFilter='';
 @observable hrStatusClass='';
 @observable hrDirty=false;
 @observable hrCanSave=true;
 @observable activationRule:AutomationRule|null=null;
 @observable activationBusy=false;
 private baseline=new DraftBaseline();
 @observable hrBusy=false;
 @observable hrStale=false;
 @observable hrError='';
 @observable hrMessage='';
 @observable hrAdditional='';
 @observable hrUndo=false;
 @observable hrCanMoveEarlier=false;
 @observable hrCanMoveLater=false;
 @observable hrTestBusy=false;
 @observable hrTestError='';
 @observable hrTestSummary='';
 @observable hrChecks:Array<{id:string;label:string;state:string;detail:string}>=[];
 @observable hrResults:Array<{index:number;name:string;operation:string;state:string}>=[];
 @observable hrBefore:Array<{id:string;name:string;value:string}>=[];
 @observable hrAfter:Array<{id:string;name:string;value:string}>=[];
 @observable hrSampleMethod='GET';
 @observable hrSampleUrl='https://example.test/';
 @observable hrSampleRequest='';
 @observable hrSampleStatus='200';
 @observable hrSampleResponse='Content-Type: text/html\nCache-Control: max-age=60';
 hrForm!:HTMLFormElement;
 hrNameInput!:HTMLInputElement;
 hrMatcher!:MatchEditor;
 hrUnsaved!:HTMLDialogElement;
 hrNewButton!:HTMLButtonElement;
 private loaded:AutomationRule|null=null;
 private baseMatcher:AutomationRule['matcher']|null=null;
 private context=0;
 private testRevision=0;
 private undo:{rule:AutomationRule;generation:number}|null=null;
 private leaveResolve:((leave:boolean)=>void)|null=null;
 private ready=false;
 editorKindChanged():void{this.hrText=this.editorKind==='response-text';this.hrTitle=this.hrText?'Find and replace response text':'Change headers';this.hrNewLabel=this.hrText?'New text replacement rule':'New header rule';if(this.ready)this.headerStateChanged();}
 private phase(rule:AutomationRule):Phase|null{return this.hrText?(rule.response.textTransform&&!rule.request.userAgent&&!rule.request.responseAsset&&!rule.request.headers.length&&!rule.response.headers.length?'response':null):headerRulePhase(rule);}
 protected hydratedCallback():void {this.editorKindChanged();this.ready=true;this.headerStateChanged();}
 private owned(rule:AutomationRule):boolean{return this.headerState?.responseSets?.some(set=>set.rules.some(item=>item.id===rule.id)||set.applied?.rules.some(item=>item.id===rule.id))??false;}
 private ordered():AutomationRule[]{return (this.headerState?.rules??[]).filter(r=>this.phase(r)&&!this.owned(r)).sort((a,b)=>a.priority-b.priority||(a.id<b.id?-1:a.id>b.id?1:0));}
 headerStateChanged():void {
  const current=this.headerState?.rules.find(rule=>rule.id===this.hrId);
  if(this.loaded&&current&&current.enabled!==this.loaded.enabled&&sameRuleContent(this.loaded,current)){this.loaded=structuredClone(current);this.hrEnabled=current.enabled!==false;}
  this.activationRule=current??null;
  const ordered=this.ordered();this.hrRows=ordered.map((r,i)=>({id:r.id,name:r.displayName??r.id,phase:this.phase(r)==='request'?'Request':'Response',state:r.enabled===false?'Disabled':'Enabled',selected:r.id===this.hrId,first:i===0,last:i===ordered.length-1}));
  const selected=this.hrRows.find(row=>row.id===this.hrId);this.hrCanMoveEarlier=Boolean(selected&&!selected.first);this.hrCanMoveLater=Boolean(selected&&!selected.last);
  this.hrStale=Boolean(this.loaded&&this.headerState?.rules.find(r=>r.id===this.loaded!.id)?.revision!==this.loaded.revision);
  this.hrUndo=Boolean(this.undo&&this.undo.generation===this.headerState?.generation);
  const phase=this.hrPhase;
  this.hrOrder=(this.headerState?.rules??[]).filter(r=>phase==='request'?Boolean(r.request.userAgent||r.request.responseAsset||r.request.headers.length||r.request.replaceBody||r.request.abortReason||r.request.discardBody):Boolean(r.response.textTransform||r.response.headers.length||r.response.replaceBody||r.response.abortReason||r.response.discardBody)).sort((a,b)=>a.priority-b.priority||(a.id<b.id?-1:a.id>b.id?1:0)).map(r=>({id:r.id,name:r.displayName??r.id,kind:r.request.responseAsset?'Saved response · first matching rule stops later hooks':r.request.userAgent?'User-Agent · first matching policy wins':r.response.textTransform?'Response text replacement · server response body':headerRulePhase(r)?'Header rule · operations compose':'Advanced native rule',state:(r.enabled===false?'Disabled':'Enabled')+(this.owned(r)&&this.headerState?.responseSets?.some(set=>set.paused&&set.applied?.rules.some(item=>item.id===r.id))?' · Response set paused':'')+(r.request.responseAsset&&this.headerState?.autoresponsesEnabled===false?' · Saved responses paused':'')}));
 }
 async refresh():Promise<void>{try{this.accept(await invoke<AutomationStatus>('automation_status'));this.hrMessage='Rules refreshed. Your draft is preserved.';}catch(error){this.hrError='Rules could not be refreshed: '+describeError(error);}}
 private accept(status:AutomationStatus):void{if(this.headerState&&status.generation<this.headerState.generation)return;this.headerState=status;this.$emit('header-rules-state',status);this.headerStateChanged();}
 private async leave():Promise<boolean>{if(this.hrBusy||this.leaveResolve)return false;if(!this.hrDirty)return true;return new Promise(resolve=>{this.leaveResolve=resolve;this.hrUnsaved.showModal();});}
 async canLeaveDraft():Promise<boolean>{return this.leave();}
 async resolveUnsaved(choice:string,event?:Event):Promise<void>{event?.preventDefault();if(this.hrBusy)return;if(choice==='save'){if(!this.hrForm.checkValidity()){await this.resolveUnsaved('stay');this.hrForm.reportValidity();return;}if(!await this.save())return;}if(choice==='discard')this.hrDirty=false;const resolve=this.leaveResolve;this.leaveResolve=null;this.hrUnsaved.close();resolve?.(choice!=='stay');}
 async newRule(source?:SelectedResponse,phase?:Phase):Promise<void>{if(!await this.leave())return;if(source)this.selection=source;this.load(null);if(phase)this.hrPhase=phase;}
 async selectRule(ruleId:string):Promise<void>{if(ruleId===this.hrId&&!this.hrStale)return;if(!await this.leave())return;const rule=this.headerState?.rules.find(r=>r.id===ruleId);if(rule)this.load(rule);}
 private load(rule:AutomationRule|null,focus=true):void {
  ++this.context;this.invalidateTest();this.loaded=rule?structuredClone(rule):null;this.baseMatcher=rule?structuredClone(rule.matcher):null;this.hrId=rule?.id??'';this.hrName=rule?.displayName??'';this.hrPhase=rule?this.phase(rule)??'response':'response';this.hrMethod=rule?.matcher.method??(rule?'ANY':this.selection?.detail.requests.find(head=>head.boundary==='client-request')?.method??'GET');this.hrEnabled=rule?.enabled??false;this.hrPlacement='keep';
  const target=this.selection?.detail.requests.find(head=>head.boundary==='client-request')?.target??'https://example.test/';
  this.hrCondition=rule?(rule.matcher.url??null):{kind:'exact',value:target};
  const transform=rule?.response.textTransform;this.hrFind=transform?.find??'original';this.hrReplacement=transform?.replacement??'modified';this.hrOccurrences=transform?.occurrences??'all';this.hrExpected=transform?.expectedMatches?.toString()??'';const sourceType=this.selection?.detail.responses.find(head=>head.boundary==='client-response')?.headers.find(field=>field.name.toLowerCase()==='content-type')?.value.split(';')[0]?.trim().toLowerCase();this.hrMedia=transform?.mediaType??(['text/html','text/css','text/plain','text/javascript','application/javascript','application/json','application/xml','image/svg+xml'].includes(sourceType??'')?sourceType!:'text/html');if(!rule&&this.hrText)this.hrSampleResponse='Content-Type: '+this.hrMedia;this.hrMaxBytes=String(transform?.maxBytes??1048576);this.hrBeforeText='';this.hrAfterText='';this.hrBodySummary='';
  this.hrOperations=rule?(this.hrPhase==='request'?rule.request.headers:rule.response.headers).map(action=>action.operation==='remove'?{id:id(),operation:'remove',name:action.field,encoding:'text',value:''}:{id:id(),operation:action.operation,name:new TextDecoder().decode(new Uint8Array(action.field.name)),...display(action.field.value)}):this.hrText?[]:[{id:id(),operation:'remove',name:'Content-Security-Policy',encoding:'text',value:''}];
  this.hrPredicates=[...(rule?.matcher.requestHeaders??[]).map(p=>({p,phase:'request' as const})),...(rule?.matcher.responseHeaders??[]).map(p=>({p,phase:'response' as const}))].map(({p,phase})=>({id:id(),phase,name:p.name,kind:p.condition.kind as HeaderPredicate['condition']['kind'],...(Array.isArray(p.condition.value)?display(p.condition.value):{encoding:'text' as const,value:typeof p.condition.value==='string'?p.condition.value:''})}));
  this.hrStatusFilter=rule?.matcher.responseStatus?.toString()??'';this.hrStatusClass=rule?.matcher.responseStatusClass?.toString()??'';this.hrAdditional=rule?[rule.matcher.host?'Host '+rule.matcher.host:'',rule.matcher.scheme?'Scheme '+rule.matcher.scheme:'',rule.matcher.port?'Port '+rule.matcher.port:'',rule.matcher.pathPrefix?'Path prefix '+rule.matcher.pathPrefix:'',rule.matcher.query!==null?'Exact query '+rule.matcher.query:''].filter(Boolean).join(' · '):'';
  this.hrSampleMethod=this.hrMethod==='ANY'?'GET':this.hrMethod;this.hrSampleUrl=rule?.matcher.url?.kind==='exact'?rule.matcher.url.value:target;this.hrEditor=true;this.hrDirty=false;this.hrError='';this.hrMessage='';this.headerStateChanged();this.$flushUpdates();this.baseline.capture(this.draftValues());this.updateDirty();if(focus&&this.ready&&this.hrActive)this.hrNameInput.focus({preventScroll:true});
 }
 textEdit(field:'hrFind'|'hrReplacement'|'hrOccurrences'|'hrExpected'|'hrMedia'|'hrMaxBytes'|'hrSampleBody',event:Event):void{this[field]=(event.currentTarget as HTMLInputElement).value;this.dirty();}
 edit(field:'hrName'|'hrMethod'|'hrStatusFilter'|'hrStatusClass'|'hrPlacement',event:Event):void{this[field]=(event.currentTarget as HTMLInputElement).value;this.dirty();}
 setEnabled(event:Event):void{this.hrEnabled=(event.currentTarget as HTMLInputElement).checked;this.dirty();}
 setPhase(event:Event):void{const phase=(event.currentTarget as HTMLSelectElement).value as Phase;if(phase==='request'&&(this.hrStatusFilter||this.hrStatusClass||this.hrPredicates.some(p=>p.phase==='response'))){this.hrError='Remove response-only conditions before switching to Request. Your draft is preserved.';this.$flushUpdates();(event.currentTarget as HTMLSelectElement).value=this.hrPhase;return;}this.hrPhase=phase;this.dirty();this.headerStateChanged();}
 private draftValues():unknown {
  const rows=(items:Array<OperationRow|PredicateRow>)=>items.map(({id:_id,encoding,value,...row})=>{
   try{return {...row,value:bytes(value,encoding)};}catch{return {...row,invalidValue:value,encoding};}
  });
  return {name:this.hrName,phase:this.hrPhase,method:this.hrMethod,matcher:this.hrMatcher.value,
   additional:this.hrAdditional,placement:this.hrPlacement,operations:rows(this.hrOperations),predicates:rows(this.hrPredicates),
   status:this.hrStatusFilter,statusClass:this.hrStatusClass,find:this.hrFind,replacement:this.hrReplacement,
   occurrences:this.hrOccurrences,expected:this.hrExpected,media:this.hrMedia,maxBytes:this.hrMaxBytes,
   enabled:this.hrId?null:this.hrEnabled};
 }
 private updateDirty():void {this.hrDirty=this.baseline.differs(this.draftValues());this.hrCanSave=!this.hrId||this.hrDirty;}
 private dirty():void{this.updateDirty();this.hrError='';this.hrMessage='';this.invalidateTest();}

 matcherChanged():void{this.dirty();}
 clearAdditional():void{if(this.baseMatcher){this.baseMatcher={...this.baseMatcher,scheme:null,host:null,port:null,pathPrefix:null,query:null};}this.hrAdditional='';this.dirty();}
 addOperation():void{this.hrOperations=[...this.hrOperations,{id:id(),operation:'set',name:'',encoding:'text',value:''}];this.dirty();}
 editOperation(rowId:string,field:'operation'|'name'|'value',event:Event):void{const value=(event.currentTarget as HTMLInputElement).value;this.hrOperations=this.hrOperations.map(row=>row.id===rowId?{...row,[field]:value}:row);this.dirty();}
 changeEncoding(rowId:string,predicate:boolean,event:Event):void{const encoding=(event.currentTarget as HTMLSelectElement).value as ValueEncoding;try{if(predicate)this.hrPredicates=this.hrPredicates.map(row=>row.id===rowId?{...row,encoding,value:encoding==='hex'?bytes(row.value,row.encoding).map(b=>b.toString(16).padStart(2,'0')).join(' '):new TextDecoder('utf-8',{fatal:true,ignoreBOM:true}).decode(new Uint8Array(bytes(row.value,row.encoding)))}:row);else this.hrOperations=this.hrOperations.map(row=>row.id===rowId?{...row,encoding,value:encoding==='hex'?bytes(row.value,row.encoding).map(b=>b.toString(16).padStart(2,'0')).join(' '):new TextDecoder('utf-8',{fatal:true,ignoreBOM:true}).decode(new Uint8Array(bytes(row.value,row.encoding)))}:row);this.dirty();}catch{this.hrError='These bytes cannot be represented as UTF-8 text. Keep Hex bytes to preserve them.';this.$flushUpdates();const row=(predicate?this.hrPredicates:this.hrOperations).find(row=>row.id===rowId);if(row)(event.currentTarget as HTMLSelectElement).value=row.encoding;}}
 moveOperation(rowId:string,delta:number):void{const rows=[...this.hrOperations],index=rows.findIndex(r=>r.id===rowId),next=index+delta;if(index<0||next<0||next>=rows.length)return;[rows[index],rows[next]]=[rows[next]!,rows[index]!];this.hrOperations=rows;this.dirty();}
 removeOperation(rowId:string):void{this.hrOperations=this.hrOperations.filter(r=>r.id!==rowId);this.dirty();}
 addPredicate(phase:Phase):void{this.hrPredicates=[...this.hrPredicates,{id:id(),phase,name:'',kind:'exists',encoding:'text',value:''}];this.dirty();}
 editPredicate(rowId:string,field:'name'|'kind'|'value',event:Event):void{const value=(event.currentTarget as HTMLInputElement).value;this.hrPredicates=this.hrPredicates.map(row=>row.id===rowId?{...row,[field]:value}:row);this.dirty();}
 removePredicate(rowId:string):void{this.hrPredicates=this.hrPredicates.filter(r=>r.id!==rowId);this.dirty();}
 private draft():AutomationRule{
  if(!this.hrName.trim())throw new Error('Enter a rule name.');if(!this.hrText&&!this.hrOperations.length)throw new Error('Add at least one header operation.');
  const number=(value:string,min:number,max:number,label='response status')=>{if(value==='')return null;if(!/^\d+$/.test(value)||Number(value)<min||Number(value)>max)throw new Error(`Enter a ${label} from ${min} to ${max}.`);return Number(value);};
  for(const row of [...this.hrOperations,...this.hrPredicates])if(!/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/.test(row.name))throw new Error('Enter a valid HTTP header name: '+(row.name||'(empty)')+'.');
  const operations:HeaderOperation[]=this.hrOperations.map(row=>row.operation==='remove'?{operation:'remove',field:row.name}:{operation:row.operation,field:{name:[...new TextEncoder().encode(row.name)],value:bytes(row.value,row.encoding)}});
  const predicates=(phase:Phase):HeaderPredicate[]=>this.hrPredicates.filter(p=>p.phase===phase).map(p=>({name:p.name,condition:p.kind==='exists'?{kind:'exists'}:{kind:p.kind,value:p.kind==='regex'?p.value:bytes(p.value,p.encoding)}}));
  const matcher={...(this.baseMatcher??{method:null,url:null,scheme:null,host:null,port:null,pathPrefix:null,query:null,requestHeaders:[],responseHeaders:[],responseStatus:null,responseStatusClass:null}),method:this.hrMethod.trim().toUpperCase()==='ANY'?null:this.hrMethod.trim().toUpperCase(),url:this.hrMatcher.value,requestHeaders:predicates('request'),responseHeaders:predicates('response'),responseStatus:number(this.hrStatusFilter,100,599),responseStatusClass:number(this.hrStatusClass,1,5)};
  return {id:this.hrId||(this.hrText?'text-':'header-')+id(),displayName:this.hrName.trim(),enabled:this.hrEnabled,revision:this.loaded?.revision??1,priority:this.loaded?.priority??0,matcher,request:{...emptyActions(),headers:this.hrPhase==='request'?operations:[],responseAsset:null,allowNonIdempotentBodyReplacement:false},response:{...emptyActions(),headers:this.hrPhase==='response'?operations:[],...(this.hrText?{textTransform:{find:this.hrFind,replacement:this.hrReplacement,occurrences:this.hrOccurrences as 'first'|'all',expectedMatches:number(this.hrExpected,0,1048576,'required match count'),mediaType:this.hrMedia,encoding:'utf-8' as const,maxBytes:Number(this.hrMaxBytes)}}:{})}};
 }
 async save(event?:Event,enable=false):Promise<boolean>{event?.preventDefault();if(this.hrBusy||this.activationBusy||!this.headerState)return false;if(!enable&&!this.hrCanSave)return true;this.hrBusy=true;this.hrError='';const context=this.context;try{const rule=this.draft();if(enable)rule.enabled=true;const status=await invoke<AutomationStatus>(this.hrText?'change_response_transform':'change_header_rule',{generation:this.headerState.generation,change:{action:'save',rule,expectedRevision:this.loaded?.revision??null,placement:this.hrPlacement}});this.accept(status);if(context!==this.context)return true;const saved=status.rules.find(r=>r.id===rule.id)!;this.load(saved,false);this.hrMessage=saved.enabled?'Saved and enabled for new matching live requests.':'Saved disabled. Live participation is unchanged for this rule until explicitly enabled.';return true;}catch(error){if(context===this.context){this.hrError='Header rule was not saved: '+describeError(error);if(typeof error==='object'&&error!==null&&'category' in error&&!['invalid-input','conflict'].includes(String(error.category)))void this.showError('Header rule was not saved',this.hrError);}return false;}finally{this.hrBusy=false;}}
 duplicateDraft():void{this.loaded=null;this.hrId='';this.hrName+=' copy';this.hrEnabled=false;this.hrStale=false;this.hrPlacement='keep';this.activationRule=null;this.dirty();this.hrMessage='Draft copied with a new identity. Saving keeps it disabled.';}
 async discard():Promise<void>{if(this.hrBusy)return;const rule=this.headerState?.rules.find(r=>r.id===this.hrId);this.load(rule??null);}
 async remove():Promise<void>{if(this.hrBusy||!this.loaded||!this.headerState||!await this.leave())return;const rule=structuredClone(this.loaded);this.hrBusy=true;try{const status=await invoke<AutomationStatus>(this.hrText?'change_response_transform':'change_header_rule',{generation:this.headerState.generation,change:{action:'remove',id:rule.id,expectedRevision:rule.revision}});this.undo={rule,generation:status.generation};this.accept(status);this.hrEditor=false;this.loaded=null;this.hrId='';this.hrDirty=false;this.hrMessage='Rule removed. Undo is available until another automation change.';this.headerStateChanged();this.$flushUpdates();if(this.hrActive){if(this.embedded)this.$emit('rule-create-focus');else this.hrNewButton.focus();}}catch(error){this.hrError=describeError(error);}finally{this.hrBusy=false;}}
 async undoRemove():Promise<void>{if(!this.undo||!this.headerState||this.hrBusy||!await this.leave())return;if(!this.undo)return;if(this.undo.generation!==this.headerState.generation){this.hrError='Automation changed after removal. Current work was preserved.';return;}this.hrBusy=true;try{const status=await invoke<AutomationStatus>(this.hrText?'change_response_transform':'change_header_rule',{generation:this.undo.generation,change:{action:'restore',rule:this.undo.rule}});const restored=this.undo.rule.id;this.undo=null;this.accept(status);this.load(status.rules.find(r=>r.id===restored)!,false);this.hrMessage='Removed rule restored.';}catch(error){this.hrError=describeError(error);}finally{this.hrBusy=false;}}
 async move(ruleId:string,earlier:boolean):Promise<void>{if(this.hrBusy||!this.headerState||!await this.leave())return;const rule=this.headerState.rules.find(r=>r.id===ruleId);if(!rule)return;this.hrBusy=true;try{const status=await invoke<AutomationStatus>(this.hrText?'change_response_transform':'change_header_rule',{generation:this.headerState.generation,change:{action:'move',id:ruleId,expectedRevision:rule.revision,earlier}});this.accept(status);if(this.loaded){const updated=status.rules.find(r=>r.id===this.loaded!.id);if(updated)this.load(updated,false);}this.hrMessage='Header rule order saved for new requests.';}catch(error){this.hrError=describeError(error);}finally{this.hrBusy=false;}}
 sample(field:'hrSampleMethod'|'hrSampleUrl'|'hrSampleRequest'|'hrSampleResponse'|'hrSampleStatus',event:Event):void{this[field]=(event.currentTarget as HTMLInputElement).value;this.invalidateTest();}
 private invalidateTest():void{++this.testRevision;this.hrTestBusy=false;this.hrTestError='';this.hrTestSummary='';this.hrChecks=[];this.hrResults=[];this.hrBefore=[];this.hrAfter=[];this.hrBodySummary='';this.hrBeforeText='';this.hrAfterText='';}
 async testDraft():Promise<void>{if(this.hrBusy)return;const revision=++this.testRevision,context=this.context;this.hrTestBusy=true;this.hrTestError='';try{const input={rule:this.draft(),method:this.hrSampleMethod,url:this.hrSampleUrl,requestHeaders:encodeHeaders(parseHeaderLines(this.hrSampleRequest)),responseStatus:Number(this.hrSampleStatus),responseHeaders:encodeHeaders(parseHeaderLines(this.hrSampleResponse))};if(this.hrText){const result=await invoke<TextTransformTest>('test_response_transform',{input:{sample:input,body:this.hrSampleBody}});if(revision!==this.testRevision||context!==this.context)return;this.hrTestSummary=result.test.matched?'Draft matches this sample.':'Draft does not match; sample is unchanged.';this.hrChecks=result.test.checks.map((c,i)=>({id:String(i),label:c.label,state:c.matched?'Matches':'Does not match',detail:c.detail}));this.hrBodySummary=result.report?bodyEditSummary(result.report):'Not selected by the conditions';this.hrBeforeText=result.beforeText;this.hrAfterText=result.afterText;return;}const result=await invoke<HeaderRuleTest>('test_header_rule',{input});if(revision!==this.testRevision||context!==this.context)return;this.hrTestSummary=result.test.matched?'Draft matches this sample. Operations below are simulated in order.':'Draft does not match this sample. Its headers pass through unchanged.';this.hrChecks=result.test.checks.map((c,i)=>({id:String(i),label:c.label,state:c.matched?'Matches':'Does not match',detail:c.detail}));this.hrResults=result.operations.map(r=>({...r,state:r.changed?'Changed':'No change'}));const fields=(headers:RawHeaderField[])=>headers.map((f,i)=>{const v=display(f.value);return {id:String(i),name:new TextDecoder().decode(new Uint8Array(f.name)),value:(v.encoding==='hex'?'Hex: ':'')+v.value};});this.hrBefore=fields(result.before);this.hrAfter=fields(result.after);}catch(error){if(revision===this.testRevision&&context===this.context)this.hrTestError=describeError(error);}finally{if(revision===this.testRevision)this.hrTestBusy=false;}}
 keyboard(event:KeyboardEvent):void{if((event.ctrlKey||event.metaKey)&&event.key.toLowerCase()==='s'){event.preventDefault();this.hrForm.requestSubmit();}}
 disconnectedCallback():void{++this.context;this.invalidateTest();this.leaveResolve?.(false);this.leaveResolve=null;super.disconnectedCallback();}
}
HeaderRules.define('header-rules');
