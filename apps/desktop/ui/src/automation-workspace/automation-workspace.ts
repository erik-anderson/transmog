import initialState from '../initial-state.json';
import { attr, observable } from '@microsoft/webui-framework';
import '../script-editor/script-editor.js';
import '../match-editor/match-editor.js';
import '../user-agent-overrides/user-agent-overrides.js';
import '../response-body-editor/response-body-editor.js';
import {responseBodyLanguage, type BodyCommit, type ResponseBodyEditor} from '../response-body-editor/response-body-editor.js';
import type {UserAgentOverrides} from '../user-agent-overrides/user-agent-overrides.js';
import type { MatchEditor } from '../match-editor/match-editor.js';
import type { ScriptEditor } from '../script-editor/script-editor.js';
import { invoke } from '@tauri-apps/api/core';
import { WorkspaceElement } from '../workspace-element.js';
import type { ResponseBodyFileInfo, MatchExample, MatchTestResult, UrlCondition, ResponseAssetInspection, SessionDetail, BodyInspection, AutomationCandidate, AutomationRule, AutomationStatus, ResponseAsset, AutoResponseSource, SelectedResponse } from '../models.js';
import { describeError, optionalText, parseHeaderLines, encodeHeaders, shortUrl, implicitDefaultPort, formatContentCodings, supportedContentCodings, editableCharacterEncoding, responseTextEncoding, encodeEditedText, clientResponseSource, autoResponseUnavailableReason } from '../utilities.js';
import { defaultWorkspace, formatBytes } from '../table-model.js';
import {ListSelection,isTextEditing} from '../list-selection.js';

const AUTORESPONSE_PRIORITY_BASE = -1_000_000;
const MAX_AUTORESPONSE_EDIT_BYTES = 16 * 1024 * 1024;
type RuleRow=AutomationRule & {order:number;name:string;criteria:string;state:string;toggleLabel:string;first:boolean;last:boolean;selected:boolean;selectionState:string;tabIndex:string;shadowedName:string;shadowedBy:string;diagnostic:string;responseText:string};
type BatchRow={included:boolean;order:number;id:string;name:string;method:string;url:string;eligible:boolean;reason:string;duplicate:string;startedAt:number};
type CapturedDraft={sessionId:string;detail:SessionDetail;headers:ResponseAsset['headers'];inspection:BodyInspection|null};

export class AutomationWorkspace extends WorkspaceElement {
  userAgentOverrides!:UserAgentOverrides;
  @observable headerOverrideCount=0;
  scriptEditor!: ScriptEditor;
  @attr view = 'traffic';
  @attr theme = 'system';
  @observable automationSection='responses';
  @observable batchActionId='';
  showAutomationSection(section:string):void {this.finishRuleDrag();this.automationSection=section;}
  async backToRules():Promise<void> {if(!this.batchReviewHidden){this.cancelBatch();return;}if(!await this.canLeaveEditor())return;const focused=this.ruleSelection.focused;this.editorRevision++;this.editorHidden=true;this.autoResponseSourceState=null;this.editingAutoResponseId=null;this.draftDirty=false;this.$flushUpdates();if(focused)this.focusRule(focused);else this.newResponseButton.focus();}
  openBatchMenu(id:string,event:MouseEvent):void {this.batchActionId=id;this.positionRuleMenu(event);}
  includeBatchRow(id:string,event:Event):void {const included=(event.currentTarget as HTMLInputElement).checked;this.batchRows=this.batchRows.map(row=>row.id===id?{...row,included}:row);this.renderBatchReview();}
  batchKeyboard(id:string,event:KeyboardEvent):void {if(isTextEditing(event))return;if(event.key==='Delete'){event.preventDefault();this.skipBatchRow(id);}else if(event.altKey && (event.key==='ArrowUp'||event.key==='ArrowDown')){event.preventDefault();this.moveBatchRow(id,event.key==='ArrowUp'?-1:1);}}
  @observable selection: SelectedResponse | null = initialState.selection;
  @observable autoresponseState:AutomationStatus|null=null;
  @observable autoresponsePending=false;
  @observable matcherCondition:UrlCondition|null=null;
  @observable matchTestResult:MatchTestResult|null=null;
  @observable matcherTestError='';
  @observable matcherTestBusy=false;
  @observable matchExamples:MatchExample[]=[];
  @observable responseBodyFile='';
  @observable responseBodyFileDetails='';
  @observable responseBodyFileError='';
  @observable fileChoiceBusy=false;
  @observable compressionType='';
  @observable compressionUnavailableText='';
  private bodyFileTouched=false;
  @observable existingResponse=false;
  @observable ruleUsageText='';
  @observable requestHeadersOpen=false;
  @observable responseOpen=true;
  responseToggled(event:Event):void {this.responseOpen=(event.currentTarget as HTMLDetailsElement).open;}
  openResponseForError():void {this.responseOpen=true;this.$flushUpdates();}
  @observable customMethodHidden=true;
  @observable advancedMatcherText='';
  autoResponseCustomMethod!:HTMLInputElement;
  private retainAdvancedMatcher=true;
  matchEditor!:MatchEditor;
  matchTestUrl!:HTMLInputElement;
  matchTestHeaders!:HTMLTextAreaElement;
  matchTestMethod!:HTMLInputElement;
  autoResponseEnabled!:HTMLInputElement;
  private savedResponse:ResponseAssetInspection|null=null;
  private responseBodyPath:string|null=null;
  private matcherTimer:number|undefined;
  private usageTimer:number|undefined;
  private matcherTestRevision=0;
  private savedHeaderText='';
  private originalResponseHeaders:Array<{field:ResponseAsset['headers'][number];line:string|null}>=[];
  private capturedDraft:CapturedDraft|null=null;
  private capturedOrigin:{sessionId:string;detail:SessionDetail}|null=null;
  autoresponseStateChanged():void {if(this.autoresponseState && this.autoresponseState!==this.automationStatus)this.renderAutomation(this.autoresponseState);}
  @observable automationText = initialState.automationText;
  @observable automationKind = initialState.automationKind;
  @observable rules:RuleRow[] = initialState.rules;
  @observable visibleRules:RuleRow[]=[];
  @observable ruleSearch='';
  @observable ruleFilter='all';
  @observable ruleSelectionCount=0;
  @observable ruleSelectionText='Select a rule to edit its properties.';
  @observable ruleDropId='';
  @observable ruleDropAfter=false;
  @observable ruleDragging=false;
  @observable ruleDragText='';
  @observable ruleDragLabel='';
  @observable ruleDragX='0px';
  @observable ruleDragY='0px';
  @observable ruleDiagnosticText='';
  @observable ruleSupersededBy='';
  @observable draftDirty=false;
  @observable batchReviewHidden=true;
  @observable batchRows:BatchRow[]=[];
  @observable batchLoading=false;
  @observable batchEligibleCount=0;
  @observable batchReviewText='';
  @observable batchError='';
  @observable ruleListShare='45%';
  @observable ruleListSplit=45;
  @observable ruleMenuX='12px';
  @observable ruleMenuY='80px';
  @observable loadingSavedResponse=false;
  ruleMoreButton!:HTMLButtonElement;
  @observable preferences=defaultWorkspace();
  @observable allRulesSelected=false;
  ruleMenu!:HTMLElement;
  newRuleMenu!:HTMLElement;
  unsavedDialog!:HTMLDialogElement;
  private ruleSelection=new ListSelection();
  private assetsIndex=new Map<string,ResponseAsset>();
  private ruleHistory:Array<{before:AutomationRule[];after:AutomationRule[];label:string}>=[];
  private leaveEditorPromise:Promise<boolean>|null=null;
  private resolveLeave:((value:boolean)=>void)|null=null;
  private batchGeneration=0;
  @observable editorHidden = initialState.editorHidden;
  @observable editorTitle = initialState.editorTitle;
  @observable sourceText = initialState.sourceText;
  @observable sourceSessionId = initialState.sourceSessionId;
  @observable sourceLabel = initialState.sourceLabel;
  @observable sourceAvailable = initialState.sourceAvailable;
  @observable sourceAvailabilityText = initialState.sourceAvailabilityText;
  @observable bodyStatusText = initialState.bodyStatusText;
  @observable bodyHidden = initialState.bodyHidden;
  @observable encodingOptionsHidden = initialState.encodingOptionsHidden;
  @observable savingAutoResponse = initialState.savingAutoResponse;
  @observable changingAutoResponse = initialState.changingAutoResponse;
  @observable autoResponseError = initialState.autoResponseError;
  @observable requestHeadersError = initialState.requestHeadersError;
  @observable responseHeadersError = initialState.responseHeadersError;
  @observable undoRemoveText = initialState.undoRemoveText;
  @observable responseReadOnly = initialState.responseReadOnly;
  @observable responseHeadersHidden = initialState.responseHeadersHidden;
  @observable capturedOptionsHidden = initialState.capturedOptionsHidden;
  @observable preserveEncodingDisabled = initialState.preserveEncodingDisabled;
  @observable bodyReadOnly = initialState.bodyReadOnly;
  @observable requestHeadersHidden = initialState.requestHeadersHidden;
  @observable editBodyDisabled = initialState.editBodyDisabled;
  @observable responseFieldsHidden = initialState.responseFieldsHidden;
  @observable highlightedRuleId: string | null = initialState.highlightedRuleId;
  autoResponseForm!: HTMLFormElement;
  autoResponseList!: HTMLDivElement;
  autoResponseMethod!: HTMLSelectElement;
  autoResponseStatus!: HTMLInputElement;
  autoResponseMediaType!: HTMLInputElement;
  autoResponseBody!: HTMLTextAreaElement;
  autoResponseEditBody!: HTMLInputElement;
  autoResponsePreserveEncoding!: HTMLInputElement;
  bodyEditor!:ResponseBodyEditor;
  bodyDialogButton!:HTMLButtonElement;
  private savedNonBodySnapshot='';
  private bodyDialogBefore:{revision:number;value:string;edit:boolean;readOnly:boolean;dirty:boolean;error:string}|null=null;
  private bodyDialogCommitted=false;
  newResponseButton!: HTMLButtonElement;
  newResponseOptionsButton!:HTMLButtonElement;
  undoRemoveButton!: HTMLButtonElement;
  private automationStatus: AutomationStatus | null = null;
  private autoResponseSourceState: AutoResponseSource | null = null;
  private editingAutoResponseId: string | null = null;
  private draggedRuleId: string | null = null;
  private draggedRuleIds:string[]=[];
  private dragGeneration:number|null=null;
  private dragOriginalSelection:ListSelection|null=null;
  private dragPointer:{x:number;y:number}|null=null;
  private dragScrollTimer:number|undefined;
  private rulePointer:{id:string;event:PointerEvent;x:number;y:number}|null=null;
  private ignoreRuleClick=false;
  private cancelRuleDrag=()=>this.finishRuleDrag();
  private editorRevision = 0;
  private editorOpener: HTMLElement | null = null;
  private sourceQuery: Promise<void> | null = null;

  protected hydratedCallback(): void { window.addEventListener('blur',this.cancelRuleDrag);void this.refreshAutomation(); }
  disconnectedCallback():void {
    this.finishRuleDrag();
    window.removeEventListener('blur',this.cancelRuleDrag);
    window.clearTimeout(this.matcherTimer);window.clearTimeout(this.usageTimer);this.matcherTestRevision++;this.editorRevision++;
    this.resolveLeave?.(false);this.resolveLeave=null;this.leaveEditorPromise=null;
    super.disconnectedCallback();
  }
  viewChanged(): void { if (this.view === 'automation') {void this.refreshSourceAvailability();this.refreshUsageSoon();}else this.finishRuleDrag(); }
  refreshUsageSoon():void {
    if(this.usageTimer!==undefined)return;
    this.usageTimer=window.setTimeout(()=>{this.usageTimer=undefined;if(this.view==='automation')void invoke<AutomationStatus>('automation_status').then(status=>{if(this.isConnected){this.renderAutomation(status);void this.refreshResponseFile();}}).catch(()=>undefined);},500);
  }
  private currentMatcher(data:FormData,existing?:AutomationRule):AutomationRule['matcher'] {
    const selectedMethod=String(data.get('method')??'GET');const method=(selectedMethod==='CUSTOM'?String(data.get('customMethod')??''):selectedMethod).toUpperCase();
    const exact=this.parseEditorHeaders('requestHeaders',data).map(header=>({name:header.name,condition:{kind:'exact',value:Array.from(new TextEncoder().encode(header.value))}}));
    const extra=this.retainAdvancedMatcher?existing?.matcher.requestHeaders.filter(header=>header.condition.kind!=='exact')??[]:[];
    const advanced=this.retainAdvancedMatcher?existing?.matcher:undefined;
    return {...existing?.matcher,method:method==='ANY'?null:method,url:this.matchEditor.value,examples:this.matchExamples,scheme:advanced?.scheme??null,host:advanced?.host??null,port:advanced?.port??null,pathPrefix:advanced?.pathPrefix??null,query:advanced?.query??null,requestHeaders:[...exact,...extra],responseHeaders:advanced?.responseHeaders??[],responseStatus:advanced?.responseStatus??null,responseStatusClass:advanced?.responseStatusClass??null};
  }
  removeAdvancedConditions():void {this.retainAdvancedMatcher=false;this.advancedMatcherText='';this.draftDirty=true;this.scheduleMatcherTest();}
  private setEditorMethod(method:string|null):void {
    const value=method??'ANY';const known=['GET','POST','PUT','PATCH','DELETE','HEAD','OPTIONS','ANY'].includes(value);
    this.autoResponseMethod.value=known?value:'CUSTOM';this.autoResponseCustomMethod.value=known?'':value;this.customMethodHidden=known;
  }
  markDraftDirty(event:Event):void {
    const target=event.target as HTMLElement;
    if(target.closest('[data-matcher-test],[data-placeholder-editor]') || (target instanceof HTMLInputElement && target.name==='enabled' && this.editingAutoResponseId))return;
    if(target===this.autoResponseMediaType)this.updateLinkedTextStatus();
    this.draftDirty=true;
  }
  onMatcherChange():void {this.draftDirty=true;this.scheduleMatcherTest();}
  scheduleMatcherTest():void {window.clearTimeout(this.matcherTimer);this.matcherTimer=window.setTimeout(()=>{void this.testMatcher();},300);}
  formKeyboard(event:KeyboardEvent):void {if((event.ctrlKey||event.metaKey) && event.key.toLowerCase()==='s'){event.preventDefault();this.autoResponseForm.requestSubmit();}}
  async canLeaveEditor():Promise<boolean> {
    if(this.savingAutoResponse)return false;
    if(this.bodyEditor?.surface?.open&&!this.bodyEditor.bodyBusy)return false;
    if(!this.draftDirty || this.editorHidden)return true;
    if(this.leaveEditorPromise)return this.leaveEditorPromise;
    this.leaveEditorPromise=new Promise(resolve=>{this.resolveLeave=resolve;});this.unsavedDialog.showModal();
    return this.leaveEditorPromise;
  }
  async resolveUnsaved(choice:string):Promise<void> {
    if(choice==='save') {if(this.autoResponseForm.reportValidity())await this.saveAutoResponse(new Event('submit',{cancelable:true}));if(this.draftDirty)choice='stay';}
    if(choice==='discard')this.draftDirty=false;
    this.unsavedDialog.close();const resolve=this.resolveLeave;this.resolveLeave=null;this.leaveEditorPromise=null;resolve?.(choice!=='stay');
  }
  unsavedCancelled(event:Event):void {event.preventDefault();void this.resolveUnsaved('stay');}
  private nonBodySnapshot():string {
    const value=(name:string)=>(this.autoResponseForm.elements.namedItem(name) as HTMLInputElement|HTMLTextAreaElement|HTMLSelectElement).value;
    const selected=value('method'),method=(selected==='CUSTOM'?value('customMethod'):selected).toUpperCase();
    const headers=(name:string)=>{try{return parseHeaderLines(value(name)).filter(header=>name!=='responseHeaders'||!['content-length','content-encoding','content-type'].includes(header.name.toLowerCase()));}catch{return value(name);}};
    return JSON.stringify({name:value('name').trim(),method,url:this.matchEditor.value,requestHeaders:headers('requestHeaders'),retainAdvanced:this.retainAdvancedMatcher,examples:this.matchExamples,status:Number(value('status')),mediaType:optionalText(value('mediaType')),responseHeaders:headers('responseHeaders'),compression:this.autoResponsePreserveEncoding.checked});
  }
  openBodyDialog():void {
    if(this.savingAutoResponse||this.loadingSavedResponse||this.fileChoiceBusy||this.responseBodyPath||this.editBodyDisabled)return;
    this.bodyDialogBefore={revision:this.editorRevision,value:this.autoResponseBody.value,edit:this.autoResponseEditBody.checked,readOnly:this.bodyReadOnly,dirty:this.draftDirty,error:this.autoResponseError};this.bodyDialogCommitted=false;
    const canSave=Boolean(this.editingAutoResponseId&&this.savedResponse&&this.nonBodySnapshot()===this.savedNonBodySnapshot);
    this.bodyEditor.open(this.autoResponseBody.value,responseBodyLanguage(this.autoResponseMediaType.value,this.autoResponseBody.value),canSave,this.bodyDialogButton);
  }
  async commitBodyDialog(event:CustomEvent<BodyCommit>):Promise<void> {
    const before=this.bodyDialogBefore;
    if(!before||before.revision!==this.editorRevision){this.bodyEditor.bodyError='The selected rule changed. Close this editor and reopen it for the current rule.';return;}
    const {value,save}=event.detail;
    if(value!==this.autoResponseBody.value){
      this.autoResponseBody.value=value;this.draftDirty=true;
      if(this.autoResponseSourceState?.kind==='captured'){this.autoResponseEditBody.checked=true;this.updateAutoResponseBodyEditState();}
    }
    if(!save){this.bodyDialogCommitted=true;this.bodyEditor.close();return;}
    if(!this.bodyEditor.bodyCanSave||this.nonBodySnapshot()!==this.savedNonBodySnapshot){this.bodyEditor.bodyError='Other properties have unsaved changes. Apply the body, then save the rule.';return;}
    this.bodyEditor.bodyBusy=true;this.bodyEditor.bodyError='';
    let saved=false;
    try {if(this.autoResponseForm.reportValidity())saved=await this.saveAutoResponse(new Event('submit',{cancelable:true}));}
    finally{this.bodyEditor.bodyBusy=false;}
    if(saved){this.bodyDialogCommitted=true;this.bodyEditor.close();}
    else this.bodyEditor.bodyError=this.autoResponseError||'The rule could not be saved. Apply the body and review the rule properties.';
  }
  bodyDialogClosed():void {
    const before=this.bodyDialogBefore;
    if(before&&!this.bodyDialogCommitted&&before.revision===this.editorRevision){this.autoResponseBody.value=before.value;this.autoResponseEditBody.checked=before.edit;this.bodyReadOnly=before.readOnly;this.draftDirty=before.dirty;this.autoResponseError=before.error;this.updateCompressionChoice();}
    this.bodyDialogBefore=null;this.bodyDialogCommitted=false;
  }
  async revertAutoResponse():Promise<void> {
    const rule=this.automationStatus?.rules.find(rule=>rule.id===this.editingAutoResponseId);this.draftDirty=false;
    const captured=this.capturedDraft;
    if(rule)await this.editAutoResponse(rule,true);
    else if(captured)await this.populateCapturedAutoResponse(captured.sessionId,captured.detail,captured);
    else if(this.capturedOrigin)await this.populateCapturedAutoResponse(this.capturedOrigin.sessionId,this.capturedOrigin.detail);
    else await this.beginScratchAutoResponse();
  }
  async setEditorEnabled():Promise<void> {
    const id=this.editingAutoResponseId;if(!id){this.draftDirty=true;return;}
    await this.mutateAutoResponse(id,'toggle');
    this.autoResponseEnabled.checked=this.automationStatus?.rules.find(rule=>rule.id===id)?.enabled??true;
  }
  private ruleOrder():string[] {return this.visibleRules.map(rule=>rule.id);}
  preferencesChanged():void {this.ruleListSplit=this.preferences.autoresponseSplit??45;this.ruleListShare=this.ruleListSplit+'%';}
  private renderRuleSelection():void {
    this.ruleSelectionCount=this.ruleSelection.ids.size;
    this.ruleSelectionText=this.ruleSelectionCount?`${this.ruleSelectionCount} rule${this.ruleSelectionCount===1?'':'s'} selected`:'Select a rule to edit its properties.';
    this.rules=this.rules.map(rule=>({...rule,selected:this.ruleSelection.ids.has(rule.id),selectionState:String(this.ruleSelection.ids.has(rule.id)),tabIndex:this.ruleSelection.focused===rule.id?'0':'-1'}));
    const search=this.ruleSearch.toLowerCase();
    this.visibleRules=this.rules.filter(rule=>(!search || (rule.name+' '+rule.criteria).toLowerCase().includes(search)) && (this.ruleFilter==='all' || (this.ruleFilter==='enabled' && rule.enabled!==false) || (this.ruleFilter==='disabled' && rule.enabled===false) || (this.ruleFilter==='shadowed' && Boolean(rule.shadowedBy))));
    if(!this.visibleRules.some(rule=>rule.tabIndex==='0') && this.visibleRules[0])this.visibleRules=this.visibleRules.map((rule,index)=>({...rule,tabIndex:index===0?'0':'-1'}));
    this.allRulesSelected=this.visibleRules.length>0 && this.visibleRules.every(rule=>this.ruleSelection.ids.has(rule.id));
  }
  searchRules(event:Event):void {this.finishRuleDrag();this.ruleSearch=(event.target as HTMLInputElement).value;this.ruleSelection.clear();this.renderRuleSelection();}
  filterRules(event:Event):void {this.finishRuleDrag();this.ruleFilter=(event.target as HTMLSelectElement).value;this.ruleSelection.clear();this.renderRuleSelection();}
  async selectRule(rule:AutomationRule,event?:MouseEvent,checkbox=false):Promise<void> {
    if(this.savingAutoResponse || this.changingAutoResponse)return;
    this.finishRuleDrag();
    const next=this.ruleSelection.clone();next.choose(rule.id,this.ruleOrder(),checkbox?{ctrlKey:true,shiftKey:event?.shiftKey??false}:event??{});
    if((next.ids.size!==1 || !next.ids.has(this.editingAutoResponseId??'')) && !await this.canLeaveEditor())return;
    this.ruleSelection=next;this.batchReviewHidden=true;this.renderRuleSelection();
    if(next.ids.size===1){const selected=this.rules.find(rule=>next.ids.has(rule.id));if(selected)await this.editAutoResponse(selected);}
    else {this.editorRevision++;this.editorHidden=true;this.editingAutoResponseId=null;this.autoResponseSourceState=null;}
    this.ruleSelection.focused=rule.id;this.renderRuleSelection();this.focusRule(rule.id);
  }
  selectRuleCheckbox(rule:AutomationRule,event:MouseEvent):void {event.stopPropagation();void this.selectRule(rule,event,true);}
  async ruleKeyboard(rule:AutomationRule,event:KeyboardEvent):Promise<void> {
    if(this.rulePointer){event.preventDefault();if(event.key==='Escape')this.cancelRulePointer();return;}
    if(isTextEditing(event))return;
    if((event.ctrlKey||event.metaKey) && event.key.toLowerCase()==='s'){event.preventDefault();if(!this.editorHidden)this.autoResponseForm.requestSubmit();return;}
    if(event.key==='Delete'){event.preventDefault();await this.mutateSelectedRules('remove');return;}
    if((event.ctrlKey||event.metaKey) && event.key.toLowerCase()==='z'){event.preventDefault();await this.undoRemoveAutoResponse();return;}
    if((event.ctrlKey||event.metaKey) && event.altKey && (event.key==='ArrowUp'||event.key==='ArrowDown')){event.preventDefault();await this.mutateSelectedRules(event.key==='ArrowUp'?'up':'down');return;}
    const next=this.ruleSelection.clone();if(!next.key(rule.id,this.ruleOrder(),event))return;
    if((next.ids.size!==1 || !next.ids.has(this.editingAutoResponseId??'')) && !await this.canLeaveEditor())return;
    this.ruleSelection=next;this.renderRuleSelection();this.$flushUpdates();
    const focused=this.rules.find(rule=>rule.id===next.focused);if(focused){this.focusRule(focused.id);const selected=this.rules.find(row=>next.ids.has(row.id));if(next.ids.size===1 && selected)await this.editAutoResponse(selected);else if(next.ids.size!==1){this.editorRevision++;this.editorHidden=true;}this.ruleSelection.focused=focused.id;this.renderRuleSelection();this.focusRule(focused.id);}
    else {this.editorRevision++;this.editorHidden=true;this.editingAutoResponseId=null;}
  }
  listKeyboard(event:KeyboardEvent):void {
    if(event.key==='Escape'&&this.draggedRuleId){event.preventDefault();this.cancelRulePointer();return;}
    if(event.defaultPrevented || isTextEditing(event))return;
    if((event.ctrlKey||event.metaKey) && event.key.toLowerCase()==='z'){event.preventDefault();void this.undoRemoveAutoResponse();}
    else if(event.key==='Delete'){event.preventDefault();void this.mutateSelectedRules('remove');}
  }
  positionRuleMenu(event:MouseEvent):void {const rect=(event.currentTarget as HTMLElement).getBoundingClientRect();this.ruleMenuX=Math.max(10,Math.min(rect.left,window.innerWidth-260))+'px';this.ruleMenuY=Math.max(50,Math.min(rect.bottom+4,window.innerHeight-320))+'px';this.$flushUpdates();}
  async openRuleMenu(rule:AutomationRule,event:MouseEvent):Promise<void> {event.preventDefault();if(!this.ruleSelection.ids.has(rule.id))await this.selectRule(rule);if(this.ruleSelection.ids.has(rule.id)){this.ruleMenuX=Math.max(10,Math.min(event.clientX,window.innerWidth-260))+'px';this.ruleMenuY=Math.max(50,Math.min(event.clientY,window.innerHeight-320))+'px';this.$flushUpdates();this.ruleMenu.showPopover();}}
  async selectAllRules(event:Event):Promise<void> {const input=event.target as HTMLInputElement,checked=input.checked;if(!await this.canLeaveEditor()){input.checked=this.allRulesSelected;return;}this.ruleSelection.ids=new Set(checked?this.visibleRules.map(rule=>rule.id):[]);this.renderRuleSelection();const selected=this.rules.find(rule=>this.ruleSelection.ids.has(rule.id));if(this.ruleSelectionCount===1 && selected)await this.editAutoResponse(selected);else {this.editorRevision++;this.editorHidden=true;}}
  resizeRuleList(event:CustomEvent<{value:number;committed:boolean}>):void {this.ruleListSplit=event.detail.value;this.ruleListShare=event.detail.value+'%';this.$emit('workspace-change',{patch:{autoresponseSplit:event.detail.value},committed:event.detail.committed});}
  private rememberRuleChange(before:AutomationRule[],after:AutomationRule[],label:string):void {
    this.ruleHistory.push({before:structuredClone(before),after:structuredClone(after),label});if(this.ruleHistory.length>20)this.ruleHistory.shift();this.undoRemoveText=label;
  }
  async mutateSelectedRules(mutation:'enable'|'disable'|'remove'|'up'|'down'|'duplicate'):Promise<void> {
    if(this.savingAutoResponse || this.changingAutoResponse || !this.ruleSelection.ids.size)return;
    if((mutation==='remove' || mutation==='duplicate') && !await this.canLeaveEditor())return;
    this.ruleMenu.hidePopover();this.changingAutoResponse=true;const originalSelection=this.ruleSelection.clone();
    try {
      const current=await invoke<AutomationStatus>('automation_status');const before=this.autoResponseRules(current),selected=this.ruleSelection.ids;let ordered=[...before];
      if(mutation==='remove')ordered=ordered.filter(rule=>!selected.has(rule.id));
      else if(mutation==='enable' || mutation==='disable')ordered=ordered.map(rule=>selected.has(rule.id)?{...rule,enabled:mutation==='enable',revision:rule.revision+1}:rule);
      else if(mutation==='duplicate') {
        const copies:string[]=[];ordered=ordered.flatMap(rule=>{if(!selected.has(rule.id))return [rule];const id='autoresponse-rule-'+crypto.randomUUID().replaceAll('-','');copies.push(id);return [rule,{...rule,id,displayName:Array.from('Copy of '+(rule.displayName??'Auto-response')).slice(0,128).join(''),enabled:false,revision:1}];});this.ruleSelection.ids=new Set(copies);
      } else if(mutation==='up') {
        for(let index=1;index<ordered.length;index++)if(selected.has(ordered[index]!.id) && !selected.has(ordered[index-1]!.id))[ordered[index-1],ordered[index]]=[ordered[index]!,ordered[index-1]!];
      } else {for(let index=ordered.length-2;index>=0;index--)if(selected.has(ordered[index]!.id) && !selected.has(ordered[index+1]!.id))[ordered[index],ordered[index+1]]=[ordered[index+1]!,ordered[index]!];}
      const label=mutation==='remove'?`Removed ${selected.size} rule${selected.size===1?'':'s'}.`:mutation==='duplicate'?'Created disabled copies.':mutation==='up'||mutation==='down'?'Changed rule priority.':'Changed selected rule enabled states.';
      const status=await this.activateAutoResponseOrder(current,ordered);this.rememberRuleChange(before,this.autoResponseRules(status),label);this.renderAutomation(status);
      if(mutation==='remove'){this.ruleSelection.clear();this.editingAutoResponseId=null;this.renderRuleSelection();this.editorHidden=true;this.draftDirty=false;this.editorRevision++;this.changingAutoResponse=false;this.$flushUpdates();this.undoRemoveButton.focus();}
      else {
        this.renderRuleSelection();const id=[...this.ruleSelection.ids][0];
        if(mutation==='duplicate'){this.changingAutoResponse=false;this.draftDirty=false;if(this.ruleSelection.ids.size===1 && id)await this.editAutoResponse(status.rules.find(rule=>rule.id===id)!,true);else {this.editorRevision++;this.editorHidden=true;}}
        if(this.editingAutoResponseId)this.autoResponseEnabled.checked=status.rules.find(rule=>rule.id===this.editingAutoResponseId)?.enabled??true;
        if(id)this.focusRule(id);
      }
    }catch(error:unknown){this.ruleSelection=originalSelection;this.renderRuleSelection();this.automationText='Rule change failed: '+describeError(error);this.automationKind='error';}
    finally{this.changingAutoResponse=false;}
  }
  async beginBatch(ids:string[]):Promise<void> {
    if(!Array.isArray(ids) || ids.length>256 || ids.some(id=>typeof id!=='string')){this.showNotice('Select fewer responses','Create 1–256 responses per batch. Your Traffic selection is preserved.',null,null);return;}
    if(!ids.length || !await this.canLeaveEditor())return;
    this.prepareEditor();this.newRuleMenu.hidePopover();this.editorHidden=true;this.batchReviewHidden=false;this.autoResponseSourceState=null;this.editingAutoResponseId=null;
    const revision=this.editorRevision;this.batchLoading=true;this.batchError='';
    this.batchRows=ids.map(id=>({id,included:true,order:0,name:'Checking response…',method:'',url:'',eligible:false,reason:'Checking retention…',duplicate:'',startedAt:0}));
    try {
    const status=await invoke<AutomationStatus>('automation_status');if(revision!==this.editorRevision)return;this.batchGeneration=status.generation;
    for(let offset=0;offset<ids.length;offset+=8) {
      const results=await Promise.all(ids.slice(offset,offset+8).map(async id=>{
        try {const detail=await invoke<SessionDetail>('session_detail',{id});const source=clientResponseSource(detail);const request=detail.requests.find(head=>head.boundary==='client-request');const method=request?.method??'';const url=implicitDefaultPort(request?.target??'');return {id,included:true,order:0,name:method+' '+shortUrl(url),method,url,eligible:Boolean(source),reason:source?'Ready':autoResponseUnavailableReason(detail),duplicate:'',startedAt:detail.startedAt??0};}
        catch(error:unknown){return {id,included:true,order:0,name:'Unavailable response',method:'',url:'',eligible:false,reason:describeError(error),duplicate:'',startedAt:0};}
      }));
      if(revision!==this.editorRevision)return;const updates=new Map(results.map(row=>[row.id,row]));this.batchRows=this.batchRows.map(row=>updates.get(row.id)??row);this.renderBatchReview();
    }
    if(revision===this.editorRevision){this.batchLoading=false;this.renderBatchReview();}
    }catch(error:unknown){if(revision===this.editorRevision)this.batchError=describeError(error);}
    finally{if(revision===this.editorRevision)this.batchLoading=false;}
  }
  private renderBatchReview():void {
    const seen=new Set<string>();
    this.batchRows=this.batchRows.map((row,index)=>{
      const key=row.method+' '+row.url;let duplicate=row.eligible && row.included && seen.has(key)?'Always superseded by an earlier selected response with the same match.':'';
      if(!duplicate && row.eligible && row.included){const existing=this.automationStatus?.rules.find(rule=>rule.request.responseAsset && rule.matcher.method===row.method && rule.matcher.url?.kind==='exact' && implicitDefaultPort(rule.matcher.url.value)===row.url && rule.matcher.requestHeaders.length===0 && !rule.matcher.host && !rule.matcher.scheme && !rule.matcher.port && !rule.matcher.pathPrefix && rule.matcher.query===null);if(existing)duplicate=`Will have higher priority than existing “${existing.displayName??'Auto-response'}” with the same match.`;}
      if(row.eligible && row.included)seen.add(key);return {...row,order:index+1,duplicate};
    });
    this.batchEligibleCount=this.batchRows.filter(row=>row.eligible && row.included).length;const unavailable=this.batchRows.filter(row=>!row.eligible).length;const excluded=this.batchRows.filter(row=>row.eligible && !row.included).length;
    this.batchReviewText=this.batchLoading?`${this.batchRows.length} selected · Checking retained responses…`:`${this.batchRows.length} selected · ${this.batchEligibleCount} ready${unavailable?` · ${unavailable} unavailable`:''}${excluded?` · ${excluded} excluded`:''}. Rules are added at the top in the order below.`;
  }
  skipBatchRow(id:string):void {this.batchRows=this.batchRows.map(row=>row.id===id?{...row,included:false}:row);this.renderBatchReview();}
  moveBatchRow(id:string,direction:number):void {const rows=[...this.batchRows],index=rows.findIndex(row=>row.id===id),next=index+direction;if(next<0 || next>=rows.length)return;[rows[index],rows[next]]=[rows[next]!,rows[index]!];this.batchRows=rows;this.renderBatchReview();}
  keepFirstBatchMatches():void {const seen=new Set<string>();this.batchRows=this.batchRows.map(row=>{const key=row.method+' '+row.url;const included=row.eligible && !seen.has(key);if(included)seen.add(key);return {...row,included};});this.renderBatchReview();}
  keepNewestBatchMatches():void {const newest=new Map<string,BatchRow>();for(const row of this.batchRows)if(row.eligible){const key=row.method+' '+row.url;const previous=newest.get(key);if(!previous || row.startedAt>previous.startedAt)newest.set(key,row);}this.batchRows=this.batchRows.map(row=>({...row,included:row.eligible && newest.get(row.method+' '+row.url)===row}));this.renderBatchReview();}
  async reviewBatchAgain():Promise<void> {const choices=new Map(this.batchRows.map(row=>[row.id,row.included]));await this.beginBatch(this.batchRows.map(row=>row.id));if(this.batchReviewHidden)return;this.batchRows=this.batchRows.map(row=>({...row,included:choices.get(row.id)??row.included}));this.renderBatchReview();}
  cancelBatch():void {if(this.savingAutoResponse)return;this.editorRevision++;this.batchReviewHidden=true;this.batchLoading=false;this.newResponseButton.focus();}
  async createBatchRules():Promise<void> {
    if(this.savingAutoResponse || this.batchLoading || !this.batchEligibleCount)return;this.savingAutoResponse=true;this.batchError='';
    try {
      const before=this.automationStatus?this.autoResponseRules(this.automationStatus):[];
      const result=await invoke<{status:AutomationStatus;createdIds:string[]}>('create_autoresponse_batch',{input:{ids:this.batchRows.filter(row=>row.eligible && row.included).map(row=>row.id),generation:this.batchGeneration}});
      this.rememberRuleChange(before,this.autoResponseRules(result.status),`Created ${result.createdIds.length} rules.`);this.renderAutomation(result.status);await this.refreshAutomation();this.ruleSelection.ids=new Set(result.createdIds);this.ruleSelection.focused=result.createdIds[0]??null;this.batchReviewHidden=true;this.renderRuleSelection();
      this.savingAutoResponse=false;const id=result.createdIds[0];if(id){this.focusRule(id);if(result.createdIds.length===1)await this.editAutoResponse(this.rules.find(rule=>rule.id===id)!);}
    }catch(error:unknown){this.batchError=describeError(error);}
    finally{this.savingAutoResponse=false;}
  }
  async testMatcher():Promise<void> {
    const revision=++this.matcherTestRevision;
    this.matcherTestBusy=true;this.matcherTestError='';
    try {
      const existing=this.automationStatus?.rules.find(rule=>rule.id===this.editingAutoResponseId);
      const matcher=this.currentMatcher(new FormData(this.autoResponseForm),existing);
      try {await invoke<MatchTestResult>('test_autoresponse_match',{input:{matcher,method:matcher.method??'GET',url:'https://example.test/',headers:[],ruleId:this.editingAutoResponseId,enabled:this.autoResponseEnabled.checked}});}
      catch(error:unknown){if(revision===this.matcherTestRevision)this.matchEditor.matchValidationError=describeError(error);throw error;}
      if(revision!==this.matcherTestRevision)return;this.matchEditor.matchValidationError='';
      if(!this.matchTestUrl.value){this.matchTestResult=null;return;}
      const headers=parseHeaderLines(this.matchTestHeaders.value);
      const result=await invoke<MatchTestResult>('test_autoresponse_match',{input:{matcher,method:this.matchTestMethod.value||'GET',url:this.matchTestUrl.value,headers,ruleId:this.editingAutoResponseId,enabled:this.autoResponseEnabled.checked}});
      if(revision===this.matcherTestRevision && !this.editorHidden)this.matchTestResult=result;
    } catch(error:unknown) {if(revision===this.matcherTestRevision){this.matcherTestError=describeError(error);this.matchTestResult=null;}}
    finally {if(revision===this.matcherTestRevision)this.matcherTestBusy=false;}
  }
  addMatchExample(expected:boolean):void {
    if(!this.matchTestUrl.value || this.matchExamples.length>=32)return;
    let headers:Array<{name:string;value:string}>;try{headers=parseHeaderLines(this.matchTestHeaders.value);}catch(error:unknown){this.matcherTestError=describeError(error);return;}
    this.draftDirty=true;const example={method:this.matchTestMethod.value||'GET',url:this.matchTestUrl.value,expected,headers};
    this.matchExamples=[...this.matchExamples.filter(item=>item.url!==example.url || item.method!==example.method),example];void this.testMatcher();
  }
  removeMatchExample(url:string,method?:string):void {this.draftDirty=true;this.matchExamples=this.matchExamples.filter(example=>example.url!==url || (method!==undefined && example.method!==method));void this.testMatcher();}
  async chooseResponseBody():Promise<void> {
    if(this.savingAutoResponse || this.fileChoiceBusy)return;
    const revision=this.editorRevision;
    this.fileChoiceBusy=true;
    try {
      const path=await invoke<string|null>('pick_response_body');
      if(!path || revision!==this.editorRevision || !this.isConnected)return;
      const info=await invoke<ResponseBodyFileInfo>('response_body_file_info',{path,reference:null});
      if(revision!==this.editorRevision || !this.isConnected)return;
      this.draftDirty=true;this.bodyFileTouched=true;this.setResponseFile(info);
    }catch(error:unknown){if(revision===this.editorRevision)this.autoResponseError=describeError(error);}
    finally{if(revision===this.editorRevision)this.fileChoiceBusy=false;}
  }
  private setResponseFile(info:ResponseBodyFileInfo):void {
    this.responseBodyPath=info.path;this.responseBodyFile=info.path;
    this.responseBodyFileDetails=info.linked?'Read directly for each request. External edits affect future responses.':'Contents will be copied when you save (up to 25 MB). External edits will not affect the saved response.';
    this.responseBodyFileError=info.error??'';
    this.updateCompressionChoice();
  }
  private async refreshResponseFile():Promise<void> {
    const path=this.responseBodyPath,revision=this.editorRevision;if(!path)return;
    try {
      const info=await invoke<ResponseBodyFileInfo>('response_body_file_info',{path,reference:this.bodyFileTouched?null:this.savedResponse?.asset.linkedBody?`${this.savedResponse.asset.id}@${this.savedResponse.asset.revision}`:null});
      if(revision===this.editorRevision && path===this.responseBodyPath)this.responseBodyFileError=info.error??'';
    }catch(error:unknown){if(revision===this.editorRevision && path===this.responseBodyPath)this.responseBodyFileError=describeError(error);}
  }
  clearResponseBodyFile():void {
    this.draftDirty=true;this.bodyFileTouched=Boolean(this.savedResponse?.asset.linkedBody);this.responseBodyPath=null;this.responseBodyFile='';this.responseBodyFileDetails='';this.responseBodyFileError='';
    if(this.savedResponse?.asset.linkedBody){
      this.bodyReadOnly=false;this.editBodyDisabled=false;
      this.updateLinkedTextStatus();
    }
    this.updateCompressionChoice();
  }
  private updateLinkedTextStatus():void {
    if(!this.savedResponse?.asset.linkedBody || this.responseBodyPath)return;
    const encoding=responseTextEncoding(this.autoResponseMediaType.value);
    this.bodyStatusText=encoding?`Enter a replacement text body, or choose another file. Text uses ${encoding.toUpperCase()}.`:'To save text, set a supported charset in Content type (for example, UTF-8), or choose another file.';
  }
  private editableHeaders(asset:Pick<ResponseAsset,'headers'>):string {
    this.originalResponseHeaders=[];const lines:string[]=[];const decoder=new TextDecoder('utf-8',{fatal:true});
    for(const field of asset.headers??[]) {
      const name=new TextDecoder().decode(new Uint8Array(field.name));
      if(['content-length','content-encoding','content-type'].includes(name.toLowerCase()))continue;
      let line:string|null=null;
      try {line=name+': '+decoder.decode(new Uint8Array(field.value));lines.push(line);}catch{/* Non-text values stay in their original ordered slots. */}
      this.originalResponseHeaders.push({field,line});
    }
    return lines.join('\n');
  }
  private mergeResponseHeaders(text:string,edited:Array<{name:string;value:string}>):ResponseAsset['headers'] {
    if(text===this.savedHeaderText)return this.originalResponseHeaders.map(({field})=>field);
    const result=encodeHeaders(edited),available=new Set(edited.map((_,index)=>index));
    const byLine=new Map<string,number[]>(),byName=new Map<string,number[]>();
    for(let index=edited.length-1;index>=0;index--){
      const header=edited[index]!;
      for(const [map,key] of [[byLine,header.name.toLowerCase()+': '+header.value],[byName,header.name.toLowerCase()]] as const){const queue=map.get(key)??[];queue.push(index);map.set(key,queue);}
    }
    const take=(queue:number[]|undefined):number|undefined=>{while(queue?.length){const index=queue.pop()!;if(available.delete(index))return index;}return undefined;};
    // Match unchanged fields first (including duplicates), then edited values by name.
    // A protected field stays before its next surviving text field; deleting an earlier
    // text field therefore cannot swap repeated values across the protected field.
    const matches=this.originalResponseHeaders.map(({line})=>{
      if(line===null)return undefined;
      const header=parseHeaderLines(line)[0]!;
      return take(byLine.get(header.name.toLowerCase()+': '+header.value));
    });
    this.originalResponseHeaders.forEach(({field,line},index)=>{
      if(line===null)return;
      if(matches[index]!==undefined){
        const match=matches[index]!;
        result[match]={name:edited[match]!.name===line.slice(0,line.indexOf(':'))?field.name:result[match]!.name,value:field.value};
      }
      else matches[index]=take(byName.get(new TextDecoder().decode(new Uint8Array(field.name)).toLowerCase()));
    });
    const protectedSlots=new Map<number,ResponseAsset['headers']>();let next=result.length;
    for(let index=this.originalResponseHeaders.length-1;index>=0;index--){
      const {field,line}=this.originalResponseHeaders[index]!;
      if(line!==null){if(matches[index]!==undefined)next=matches[index]!;}
      else {const slot=protectedSlots.get(next)??[];slot.unshift(field);protectedSlots.set(next,slot);}
    }
    return result.flatMap((field,index)=>[...(protectedSlots.get(index)??[]),field]).concat(protectedSlots.get(result.length)??[]);
  }
  private prepareEditor(): void {
    this.automationSection='responses';
    this.editorRevision++;
    if (this.editorHidden) {const opener=(this.getRootNode() as ShadowRoot).activeElement as HTMLElement|null;this.editorOpener=opener?.closest('#new-response-menu')?this.newResponseOptionsButton:opener;}
    this.autoResponseForm.reset();
    this.matchTestMethod.value='GET';
    this.draftDirty=false;this.batchReviewHidden=true;this.retainAdvancedMatcher=true;this.advancedMatcherText='';this.ruleDiagnosticText='';this.ruleSupersededBy='';this.customMethodHidden=true;
    this.batchLoading=false;this.loadingSavedResponse=false;
    this.savedResponse=null;this.responseBodyPath=null;this.responseBodyFile='';this.existingResponse=false;this.ruleUsageText='';this.matcherCondition={kind:'exact',value:''};this.matchTestResult=null;this.matcherTestError='';this.matchExamples=[];
    this.responseBodyFileDetails='';this.responseBodyFileError='';this.fileChoiceBusy=false;this.bodyFileTouched=false;this.editBodyDisabled=true;this.savedNonBodySnapshot='';
    this.compressionType='';
    this.compressionUnavailableText='';
    this.savedHeaderText='';this.originalResponseHeaders=[];
    this.capturedDraft=null;
    this.capturedOrigin=null;
    window.clearTimeout(this.matcherTimer);this.matcherTestRevision++;
    this.autoResponseEnabled.checked=true;
    for (const field of ['requestHeaders','responseHeaders']) (this.autoResponseForm.elements.namedItem(field) as HTMLTextAreaElement).setCustomValidity('');
    this.responseOpen=true;
    this.sourceSessionId = ''; this.sourceLabel = ''; this.sourceAvailable = false; this.sourceAvailabilityText = '';
    this.bodyStatusText = ''; this.bodyHidden = false; this.encodingOptionsHidden = true;
    this.autoResponseError = ''; this.requestHeadersError = ''; this.responseHeadersError = '';
    this.sourceQuery = null;
  }
  private openEditor(focusProperties=true): void {
    this.newRuleMenu.hidePopover();
    this.editorHidden = false; this.$flushUpdates();
    this.autoResponseForm.scrollIntoView({block:'start'});
    if(focusProperties)(this.autoResponseForm.elements.namedItem('name') as HTMLInputElement).focus({preventScroll:true});
  }
  async refreshSourceAvailability(): Promise<void> {
    const id = this.sourceSessionId;
    if (!id || this.editorHidden || !this.isConnected) return;
    if (this.sourceQuery) return this.sourceQuery;
    const revision = this.editorRevision;
    const pending = (async () => {
      try {
        const detail = await invoke<SessionDetail>('session_detail',{id});
        if (revision !== this.editorRevision || !this.isConnected) return;
        const request = detail.requests.find((head) => head.boundary === 'client-request');
        this.sourceLabel = request?.target ? `${request.method ?? 'Request'} ${shortUrl(request.target)}` : 'Original captured request';
        this.sourceAvailable = true; this.sourceAvailabilityText = '';
      } catch (error:unknown) {
        if (revision !== this.editorRevision || !this.isConnected) return;
        this.sourceAvailable = false;
        this.sourceAvailabilityText = /evict|unavailable|not found/i.test(describeError(error))
          ? 'Source no longer in Traffic.' : 'Source could not be checked. Try again when Traffic is available.';
      }
    })();
    this.sourceQuery = pending;
    try { await pending; } finally { if (this.sourceQuery === pending) this.sourceQuery = null; }
  }
  async showSourceTraffic(event:Event): Promise<void> {
    event.preventDefault();
    const id = this.sourceSessionId;
    const revision = this.editorRevision;
    await this.refreshSourceAvailability();
    if (this.sourceAvailable && revision === this.editorRevision && id === this.sourceSessionId) this.$emit('source-traffic-request',id);
  }
  clearHeaderError(field:'requestHeaders' | 'responseHeaders'): void {
    (this.autoResponseForm.elements.namedItem(field) as HTMLTextAreaElement).setCustomValidity('');
    if (field === 'requestHeaders') this.requestHeadersError = '';
    else this.responseHeadersError = '';
    this.autoResponseError = '';
    if(field==='requestHeaders')this.onMatcherChange();
  }
  private parseEditorHeaders(field:'requestHeaders' | 'responseHeaders',data:FormData): ReturnType<typeof parseHeaderLines> {
    try { return parseHeaderLines(String(data.get(field) ?? '')); }
    catch (error:unknown) {
      const message = describeError(error);
      if (field === 'requestHeaders') this.requestHeadersError = message;
      else this.responseHeadersError = message;
      const input = this.autoResponseForm.elements.namedItem(field) as HTMLTextAreaElement;
      if(field==='responseHeaders')this.responseOpen=true;else this.requestHeadersOpen=true;
      this.$flushUpdates();
      input.setCustomValidity(message); input.focus();
      throw error;
    }
  }
  private renderAutoResponseRules(status: AutomationStatus): void {
    const rules = this.autoResponseRules(status);
    const retained=new Set(rules.map(rule=>rule.id));
    this.ruleSelection.ids=new Set([...this.ruleSelection.ids].filter(id=>retained.has(id)));
    if(this.ruleSelection.focused && !retained.has(this.ruleSelection.focused))this.ruleSelection.focused=null;
    if(this.ruleSelection.anchor && !retained.has(this.ruleSelection.anchor))this.ruleSelection.anchor=null;
    this.rules = rules.map((rule,index) => {
      const headers = rule.matcher.requestHeaders.length;
      const enabled = rule.enabled ?? true;
      const diagnostic=status.diagnostics?.find(item=>item.ruleId===rule.id);const earlier=status.rules.find(item=>item.id===diagnostic?.supersededBy);const asset=this.assetsIndex.get(rule.request.responseAsset??'');
      return { ...rule, enabled, order: index + 1, name: rule.displayName ?? 'Auto-response',
        criteria: `${rule.matcher.method ?? 'ANY'} ${rule.matcher.url?.kind==='exact'?rule.matcher.url.value:rule.matcher.url?.kind==='pattern'?rule.matcher.url.value.address:rule.matcher.url?.kind==='regex'?rule.matcher.url.value.pattern:'any URL'}${headers === 0 ? '' : ` · ${headers} header condition${headers === 1 ? '' : 's'}`} · saved response`,
        state: enabled ? status.autoresponsesEnabled===false?'Paused':'Enabled' : 'Disabled', toggleLabel: enabled ? 'Disable' : 'Enable', first: index === 0, last: index === rules.length - 1,
        selected:this.ruleSelection.ids.has(rule.id),selectionState:String(this.ruleSelection.ids.has(rule.id)),tabIndex:this.ruleSelection.focused===rule.id?'0':'-1',shadowedName:earlier?.displayName??earlier?.id??'',shadowedBy:diagnostic?.supersededBy??'',diagnostic:diagnostic?diagnostic.duplicateResponse?'Duplicate':'Shadowed':'',responseText:asset?.linkedBody?`${asset.status} · File`:asset?`${asset.status} · ${formatBytes(asset.bodyBytes)}`:'Saved response'};
    });
    this.renderRuleSelection();const selected=this.rules.find(rule=>rule.id===this.editingAutoResponseId);
    this.ruleSupersededBy=selected?.shadowedBy??'';this.ruleDiagnosticText=selected?.shadowedBy?`${selected.enabled===false?'If enabled, this':'This'} rule is always superseded by “${selected.shadowedName}”, which has equivalent matching conditions${selected.diagnostic==='Duplicate'?' and the same response':''}.`:'';
    if(this.existingResponse)this.renderRuleUsage(status);
  }
  private renderRuleUsage(status:AutomationStatus):void {
    const usage=status.usage?.find(item=>item.ruleId===this.editingAutoResponseId);
    this.ruleUsageText=usage?`${usage.matches} retained match${usage.matches===1?'':'es'} · Last matched ${new Date(usage.lastMatchedAt).toLocaleString()}`:'No matches in retained Traffic.';
  }
  ruleClick(rule:AutomationRule,event:MouseEvent):void {if(!this.ignoreRuleClick)void this.selectRule(rule,event);}
  beginRulePointer(id:string,event:PointerEvent):void {
    if(event.button!==0||!event.isPrimary||this.savingAutoResponse||this.changingAutoResponse)return;
    const target=event.target as HTMLElement;
    if(target.closest('input,button,a,select,textarea')||(event.pointerType==='touch'&&!target.closest('.rule-drag-grip')))return;
    event.preventDefault();
    this.finishRuleDrag();this.rulePointer={id,event,x:event.clientX,y:event.clientY};
    this.autoResponseList.setPointerCapture(event.pointerId);(event.currentTarget as HTMLElement).focus({preventScroll:true});
  }
  moveRulePointer(event:PointerEvent):void {
    const pointer=this.rulePointer;if(!pointer||pointer.event.pointerId!==event.pointerId)return;
    if(!this.draggedRuleId){
      if(Math.hypot(event.clientX-pointer.x,event.clientY-pointer.y)<6)return;
      if(!this.startRuleDrag(pointer.id))return;
    }
    event.preventDefault();this.dragPointer={x:event.clientX,y:event.clientY};this.ruleDragX=event.clientX+12+'px';this.ruleDragY=event.clientY+12+'px';this.updateRuleDrop();
    if(this.dragScrollTimer===undefined)this.dragScrollTimer=window.setInterval(()=>this.scrollRuleDrag(),30);
  }
  async endRulePointer(event:PointerEvent):Promise<void> {
    const pointer=this.rulePointer;if(!pointer||pointer.event.pointerId!==event.pointerId)return;
    this.ignoreRuleClick=true;window.setTimeout(()=>{this.ignoreRuleClick=false;},0);
    if(this.draggedRuleId)await this.dropRule(event);
    else {const rule=this.rules.find(rule=>rule.id===pointer.id);this.finishRuleDrag();if(rule)await this.selectRule(rule,pointer.event);}
  }
  private startRuleDrag(id:string):boolean {
    if (this.savingAutoResponse || this.changingAutoResponse)return false;
    const originalSelection=this.ruleSelection.clone();
    if(!this.ruleSelection.ids.has(id)){if(this.draftDirty){this.finishRuleDrag();this.ruleDragText='Save or revert your properties before dragging another rule.';return false;}this.ruleSelection.replace(id);this.renderRuleSelection();}
    this.dragOriginalSelection=originalSelection;
    this.ruleSelection.focused=id;this.renderRuleSelection();
    this.draggedRuleId = id;
    this.draggedRuleIds=this.rules.filter(rule=>this.ruleSelection.ids.has(rule.id)).map(rule=>rule.id);
    this.dragGeneration=this.automationStatus?.generation??null;this.ruleDragging=true;
    this.ruleDragText=`Moving ${this.draggedRuleIds.length} rule${this.draggedRuleIds.length===1?'':'s'} · Hold at an edge to scroll.`;
    this.ruleDragLabel=this.draggedRuleIds.length===1?this.rules.find(rule=>rule.id===id)?.name??'Auto-response':`Moving ${this.draggedRuleIds.length} rules`;
    return true;
  }
  private updateRuleDrop():void {
    this.ruleDropId='';const pointer=this.dragPointer;if(!pointer)return;
    const list=this.autoResponseList.getBoundingClientRect();
    if(pointer.x<list.left||pointer.x>list.right||pointer.y<list.top||pointer.y>list.bottom)return;
    const rows=Array.from(this.autoResponseList.querySelectorAll<HTMLElement>('[data-rule-id]'));
    const row=rows.find(row=>row.getBoundingClientRect().bottom>pointer.y)??rows.at(-1);
    if(!row||this.draggedRuleIds.includes(row.dataset.ruleId??''))return;
    const rect=row.getBoundingClientRect();this.ruleDropId=row.dataset.ruleId??'';this.ruleDropAfter=pointer.y>=rect.top+rect.height/2;
  }
  private scrollRuleDrag():void {
    const pointer=this.dragPointer;if(!pointer||!this.draggedRuleId||!this.isConnected||this.view!=='automation'){this.finishRuleDrag();return;}
    const list=this.autoResponseList,rect=list.getBoundingClientRect(),edge=Math.min(48,rect.height/4);
    if(pointer.x<rect.left||pointer.x>rect.right||pointer.y<rect.top||pointer.y>rect.bottom)return;
    const direction=pointer.y<rect.top+edge?-(1-(pointer.y-rect.top)/edge):pointer.y>rect.bottom-edge?1-(rect.bottom-pointer.y)/edge:0;
    if(direction){list.scrollTop+=direction*18;this.updateRuleDrop();}
  }
  finishRuleDrag(): void {
    const pointer=this.rulePointer;this.rulePointer=null;
    if(pointer&&this.autoResponseList.hasPointerCapture(pointer.event.pointerId))this.autoResponseList.releasePointerCapture(pointer.event.pointerId);
    window.clearInterval(this.dragScrollTimer);this.dragScrollTimer=undefined;this.dragPointer=null;this.draggedRuleId=null;
    this.draggedRuleIds=[];this.dragGeneration=null;this.ruleDropId='';this.ruleDropAfter=false;this.ruleDragging=false;this.ruleDragText='';
    if(this.dragOriginalSelection){this.ruleSelection=this.dragOriginalSelection;this.dragOriginalSelection=null;this.renderRuleSelection();}
  }
  lostRulePointer(event:PointerEvent):void {if(this.rulePointer?.event.pointerId===event.pointerId&&!this.autoResponseList.hasPointerCapture(event.pointerId))this.finishRuleDrag();}
  cancelRulePointer():void {const focused=this.dragOriginalSelection?.focused;this.finishRuleDrag();if(focused&&this.isConnected&&this.view==='automation')this.focusRule(focused);}
  private async dropRule(event: PointerEvent): Promise<void> {
    event.preventDefault();
    this.dragPointer={x:event.clientX,y:event.clientY};this.updateRuleDrop();
    const dragged=this.draggedRuleId,selected=this.draggedRuleIds,id=this.ruleDropId,after=this.ruleDropAfter,generation=this.dragGeneration,selection=this.ruleSelection.clone();
    this.finishRuleDrag();
    if (this.savingAutoResponse || this.changingAutoResponse) return;
    const current = this.automationStatus;
    if (!dragged || !id || current === null) return;
    if(current.generation!==generation){this.automationText='Rules changed while dragging. Try the move again.';this.automationKind='error';return;}
    const before = this.autoResponseRules(current);
    const group=before.filter(rule=>selected.includes(rule.id));const ordered=before.filter(rule=>!selected.includes(rule.id));const to=ordered.findIndex(rule=>rule.id===id);
    if(!group.length || to<0)return;ordered.splice(to+(after?1:0),0,...group);
    if(ordered.every((rule,index)=>rule.id===before[index]?.id))return;
    this.changingAutoResponse = true;
    try {
      const status=await this.activateAutoResponseOrder(current,ordered);this.ruleSelection=selection;this.rememberRuleChange(before,this.autoResponseRules(status),'Changed rule priority.');this.renderAutomation(status);
      this.changingAutoResponse=false;
      if(!this.draftDirty&&this.ruleSelection.ids.size===1&&this.editingAutoResponseId!==dragged){const rule=status.rules.find(rule=>rule.id===dragged);if(rule)await this.editAutoResponse(rule,true);}
      if(!this.rulePointer&&this.ruleSelection.focused===dragged&&this.isConnected&&this.view==='automation')this.focusRule(dragged);
    }
    catch (error: unknown) { this.automationText = `Auto-response reorder failed: ${describeError(error)}`; this.automationKind = 'error'; }
    finally { this.changingAutoResponse = false; }
  }
  async showMatchedRule(id: string): Promise<void> {
    const rule=this.automationStatus?.rules.find(rule=>rule.id===id);
    if(!rule){this.showNotice('Historical rule','The response records which rule won, but that rule is no longer in the active list.',null,null);return;}
    if(!await this.canLeaveEditor())return;
    this.ruleSearch='';this.ruleFilter='all';this.ruleSelection.replace(id);this.renderRuleSelection();await this.editAutoResponse(rule);
    this.highlightedRuleId = id;
    this.$flushUpdates();
    const card = this.getRootNode() as ShadowRoot;
    const element = card.getElementById(`rule-${id}`);
    if (!element) { this.showNotice('Historical rule','The response records which rule won, but that rule is no longer in the active list.',null,null); return; }
    element.scrollIntoView({block:'center'});
    element.focus({preventScroll:true});
  }
  async refreshAutomation(): Promise<void> {
    try {
      const [status,assets]=await Promise.all([invoke<AutomationStatus>('automation_status'),invoke<ResponseAsset[]>('response_assets')]);
      this.assetsIndex=new Map(assets.map(asset=>[`${asset.id}@${asset.revision}`,asset]));this.renderAutomation(status);
    } catch (error: unknown) {
      this.automationText = `Automation query failed: ${describeError(error)}`;
    }
  }

  async beginCapturedResponseFlow():Promise<void> {
    if(this.savingAutoResponse || this.changingAutoResponse)return;
    if(this.selection?.reusable)await this.beginAutoResponseFromSelected();
    else this.activateView('traffic');
  }

  async beginAutoResponseFromSelected(): Promise<void> {
    if (this.savingAutoResponse) return;
    if (this.selection === null) {
      this.showNotice('Select a completed response', 'Choose a request in Traffic whose client response body was retained, then try again.', null, null);
      return;
    }
    await this.populateCapturedAutoResponse(this.selection.sessionId, this.selection.detail);
  }

  async populateCapturedAutoResponse(sessionId: string, detail: SessionDetail, restored?:CapturedDraft): Promise<void> {
    if (this.savingAutoResponse) return;
    const source = clientResponseSource(detail);
    if (source === null || source.request.method === null || source.request.target === null || source.response.status === null) {
      this.showNotice(
        'Response cannot be reused',
        autoResponseUnavailableReason(detail),
        null,
        null,
      );
      return;
    }
    if(!await this.canLeaveEditor() || this.savingAutoResponse)return;
    this.editingAutoResponseId = null;
    this.prepareEditor();
    this.capturedOrigin={sessionId,detail:structuredClone(detail)};
    const elements = this.autoResponseForm.elements;
    const url=implicitDefaultPort(source.request.target);
    (elements.namedItem('name') as HTMLInputElement).value = `${source.request.method} ${shortUrl(url)}`;
    this.setEditorMethod(source.request.method.toUpperCase());
    (elements.namedItem('url') as HTMLInputElement).value = url;
    this.matcherCondition={kind:'exact',value:url};this.matchTestUrl.value=url;
    this.matchTestMethod.value=source.request.method;
    this.matchTestHeaders.value=source.request.headers.filter(header=>!header.sensitive && !header.binary).map(header=>header.name+': '+header.value).join('\n');
    this.autoResponseStatus.value = String(source.response.status);
    this.responseReadOnly = false;
    this.autoResponseMediaType.value = source.body.mediaType ?? '';
    this.responseFieldsHidden = false;
    (elements.namedItem('responseHeaders') as HTMLTextAreaElement).value = '';
    this.responseHeadersHidden = false;
    this.capturedOptionsHidden = false;
    this.autoResponseEditBody.checked = false;
    this.autoResponsePreserveEncoding.checked = true;
    this.preserveEncodingDisabled = source.body.contentCodings.length === 0;
    this.encodingOptionsHidden = source.body.contentCodings.length === 0;
    this.autoResponseBody.value = '';
    this.bodyReadOnly = true;
    this.editorTitle = 'New rule from captured response';
    this.sourceText = `${source.response.status} · ${formatBytes(source.body.retainedBytes)} · ${source.body.mediaType ?? 'Unknown content type'}${source.body.contentCodings.length === 0 ? '' : ` · ${source.body.contentCodings.join(', ')}`}`;
    this.sourceSessionId = sessionId;
    this.sourceLabel = `${source.request.method} ${shortUrl(source.request.target)}`;
    this.sourceAvailabilityText = 'Checking source in Traffic…';
    const captured: AutoResponseSource = {
      kind: 'captured',
      sessionId,
      boundary: 'client-response',
      contentCodings: source.body.contentCodings,
      bodyEditable: false,
      textEncoding: editableCharacterEncoding(source.body.charset),
    };
    this.autoResponseSourceState = captured;
    const revision = this.editorRevision;
    this.updateAutoResponseHeaderFilterState();
    this.updateAutoResponseBodyEditState();
    this.openEditor();
    void this.refreshSourceAvailability();

    this.loadingSavedResponse=true;
    this.responseReadOnly=true;
    try {
      const headers=restored?.headers??await invoke<ResponseAsset['headers']>('autoresponse_source_headers',{id:sessionId});
      if(revision!==this.editorRevision || !this.isConnected)return;
      this.savedHeaderText=this.editableHeaders({headers});
      (elements.namedItem('responseHeaders') as HTMLTextAreaElement).value=this.savedHeaderText;
      this.capturedDraft={sessionId,detail:structuredClone(detail),headers,inspection:restored?.inspection??null};
      this.responseReadOnly=false;
    }catch(error:unknown){if(revision===this.editorRevision){this.autoResponseError=describeError(error);this.responseReadOnly=true;}}
    finally{if(revision===this.editorRevision)this.loadingSavedResponse=false;}
    if(revision!==this.editorRevision || !this.isConnected)return;

    if (source.body.retainedBytes === 0) {
      captured.bodyEditable = captured.textEncoding !== null;
      this.bodyStatusText = 'Empty response body. Enable editing to replace it.';
      this.updateAutoResponseBodyEditState();
      return;
    }
    if (source.body.retainedBytes > MAX_AUTORESPONSE_EDIT_BYTES) {
      this.bodyStatusText = 'This body is too large to edit here. The complete saved response can still be replayed.';
      return;
    }
    this.bodyStatusText = 'Loading response body…';
    try {
      const inspection = restored?.inspection??await invoke<BodyInspection>('inspect_body', {
        request: {
          sessionId,
          boundary: 'client-response',
          representation: 'original-text',
          decodeContent: true,
          offset: 0,
          maxBytes: MAX_AUTORESPONSE_EDIT_BYTES,
        },
      });
      if (revision !== this.editorRevision || this.autoResponseSourceState !== captured || !this.isConnected) return;
      if(this.capturedDraft)this.capturedDraft.inspection=inspection;
      if (!inspection.truncated && inspection.representation === 'original-text' && inspection.textEncoding !== null) {
        this.autoResponseBody.value = inspection.display;
        captured.bodyEditable = true;
        captured.textEncoding = inspection.textEncoding;
        this.bodyStatusText = `Read-only preview. Enable editing to replace the body; text uses ${inspection.textEncoding}.`;
        this.updateAutoResponseBodyEditState();
      } else {
        this.bodyStatusText = 'A complete text preview is unavailable. The saved response can still be replayed.';
      }
    } catch {
      if (revision === this.editorRevision && this.autoResponseSourceState === captured) this.bodyStatusText = 'A text preview is unavailable. The saved response can still be replayed.';
    }
  }

  async beginScratchAutoResponse(): Promise<void> {
    if (this.savingAutoResponse) return;
    if(!await this.canLeaveEditor() || this.savingAutoResponse)return;
    this.prepareEditor();
    this.editingAutoResponseId = null;
    this.autoResponseSourceState = { kind: 'scratch' };
    this.editorTitle = 'New response from scratch';
    this.sourceText = 'Custom response. Matching requests receive the status, headers, and body below.';
    this.responseReadOnly = false;
    this.responseFieldsHidden = false;
    this.responseHeadersHidden = false;
    this.capturedOptionsHidden = true;
    this.bodyReadOnly = false;this.editBodyDisabled=false;
    this.autoResponseBody.value = '';
    this.updateAutoResponseHeaderFilterState();
    this.openEditor();
  }

  async cancelAutoResponseEdit(): Promise<void> {
    if (this.savingAutoResponse) return;
    if(!await this.canLeaveEditor())return;
    this.editorRevision++;
    this.editorHidden = true;
    this.autoResponseSourceState = null;
    this.editingAutoResponseId = null;
    this.ruleSelection.clear();this.renderRuleSelection();this.draftDirty=false;
    this.$flushUpdates();
    if (this.editorOpener?.isConnected && this.editorOpener.getClientRects().length) this.editorOpener.focus();
    else this.newResponseButton.focus();
  }

  updateAutoResponseHeaderFilterState(): void {
    this.requestHeadersHidden = false;
    this.requestHeadersOpen=this.autoResponseMethod.value==='POST' || Boolean((this.autoResponseForm.elements.namedItem('requestHeaders') as HTMLTextAreaElement).value);
    this.customMethodHidden=this.autoResponseMethod.value!=='CUSTOM';
  }

  updateAutoResponseBodyEditState(): void {
    const source = this.autoResponseSourceState;
    const editable = source?.kind === 'captured' && source.bodyEditable;
    if (!editable) this.autoResponseEditBody.checked = false;
    this.editBodyDisabled = !editable;
    this.bodyReadOnly = !this.autoResponseEditBody.checked;
    this.updateCompressionChoice();
    if (editable) this.bodyStatusText = this.autoResponseEditBody.checked
      ? `Edited text will replace the captured body using ${source.textEncoding}.`
      : `Read-only preview. Enable editing to replace the body; text uses ${source.textEncoding}.`;
  }

  private updateCompressionChoice():void {
    const source=this.autoResponseSourceState;
    const codings=source?.kind==='captured'?source.contentCodings:this.savedResponse?.contentCodings??[];
    this.compressionType=formatContentCodings(codings);
    this.encodingOptionsHidden=codings.length===0;
    const editable=source?.kind==='captured'?this.autoResponseEditBody.checked:!this.bodyReadOnly;
    const unsupported=!supportedContentCodings(codings);
    this.compressionUnavailableText=unsupported?'Original compression is unsupported. Replacement bodies will be served without compression.':'';
    if(unsupported && (this.responseBodyPath || editable))this.autoResponsePreserveEncoding.checked=false;
    this.preserveEncodingDisabled=codings.length===0 || unsupported || (!this.responseBodyPath && !editable);
  }

  async saveAutoResponse(event: Event): Promise<boolean> {
    event.preventDefault();
    if (this.savingAutoResponse || this.changingAutoResponse || this.fileChoiceBusy) return false;
    if(this.autoResponseSourceState?.kind==='captured' && !this.capturedDraft){this.autoResponseError='Original response headers could not be loaded. Choose another response in Traffic.';return false;}
    if(this.editingAutoResponseId && !this.savedResponse){this.autoResponseError='Wait for the saved response to load before saving.';return false;}
    const data = new FormData(this.autoResponseForm);
    const source = this.autoResponseSourceState;
    if (source === null) return false;
    const editingId = this.editingAutoResponseId;
    const replaceBody = this.autoResponseEditBody.checked && !this.responseBodyPath;
    const preserveEncoding = this.autoResponsePreserveEncoding.checked;
    this.savingAutoResponse = true;
    this.autoResponseError = ''; this.requestHeadersError = ''; this.responseHeadersError = '';
    this.automationText = 'Validating and saving the auto-response…';
    this.automationKind = 'progress';
    try {
      const displayName = String(data.get('name') ?? '').trim();
      if (!displayName) throw new Error('Enter a rule name.');
      const editableResponseHeaders=this.parseEditorHeaders('responseHeaders',data).filter(header=>!['content-length','content-encoding','content-type'].includes(header.name.toLowerCase()));
      const responseHeaders=this.mergeResponseHeaders(String(data.get('responseHeaders')??''),editableResponseHeaders);
      const editedBody = source.kind === 'captured' && replaceBody
        ? encodeEditedText(String(data.get('body') ?? ''), source.textEncoding) : null;
      const current = await invoke<AutomationStatus>('automation_status');
      const existing = editingId === null ? undefined : current.rules.find((candidate) => candidate.id === editingId);
      if (editingId !== null && existing === undefined) throw new Error('This rule was removed. Cancel editing and create a new rule.');
      const matcher=this.currentMatcher(data,existing);
      await invoke<MatchTestResult>('test_autoresponse_match',{input:{matcher,method:matcher.method??'GET',url:'https://example.test/',headers:[],ruleId:editingId,enabled:this.autoResponseEnabled.checked}});
      let assetReference: string;
      if (source.kind === 'existing') {
        assetReference = source.assetReference;
        const saved=this.savedResponse;
        if(!saved)throw new Error('Wait for the saved response to load before saving.');
        const bodyChanged=this.autoResponseBody.value!==saved.display || (this.bodyFileTouched && !this.responseBodyPath) || (!preserveEncoding && saved.contentCodings.length>0 && saved.textEncoding!==null);
        const compressionChanged=Boolean(saved.asset.linkedBody) && Boolean(saved.asset.linkedBody?.applyOriginalCompression)!==preserveEncoding;
        if(bodyChanged || this.bodyFileTouched || compressionChanged || String(data.get('responseHeaders')??'')!==this.savedHeaderText || Number(data.get('status'))!==saved.asset.status || String(data.get('mediaType')??'')!==(saved.asset.mediaType??'')) {
          const textEncoding=saved.asset.linkedBody?responseTextEncoding(optionalText(data.get('mediaType'))):saved.textEncoding;
          if(bodyChanged && !this.responseBodyPath && saved.asset.linkedBody && textEncoding===null)throw new Error('Content type has an unsupported charset. Choose UTF-8 or serve the response body from a file.');
          const asset=await invoke<ResponseAsset>('edit_response_asset',{input:{assetReference,status:Number(data.get('status')),headers:responseHeaders,mediaType:optionalText(data.get('mediaType')),decodedBody:bodyChanged && !this.responseBodyPath?encodeEditedText(this.autoResponseBody.value,textEncoding):null,bodyPath:this.bodyFileTouched?this.responseBodyPath:null,preserveContentEncoding:preserveEncoding}});
          assetReference=`${asset.id}@${asset.revision}`;
        }
      } else {
        const suffix = crypto.randomUUID().replaceAll('-', '');
        const assetId = `autoresponse-${suffix}`;
        let asset = source.kind === 'captured'
          ? await invoke<ResponseAsset>('create_response_asset_from_session', {
              input: {
                id: assetId,
                revision: 1,
                exchangeId: source.sessionId,
                boundary: source.boundary,
                decodedBody: editedBody,
                bodyPath: this.responseBodyPath,
                preserveContentEncoding: preserveEncoding,
              },
            })
          : this.responseBodyPath ? await invoke<ResponseAsset>('import_response_asset',{input:{id:assetId,revision:1,status:Number(data.get('status')??200),headers:responseHeaders,bodyPath:this.responseBodyPath,mediaType:optionalText(data.get('mediaType'))}}) : await invoke<ResponseAsset>('create_response_asset', {
              input: {
                id: assetId,
                revision: 1,
                status: Number(data.get('status') ?? 200),
                headers: responseHeaders,
                body: Array.from(new TextEncoder().encode(String(data.get('body') ?? ''))),
                mediaType: optionalText(data.get('mediaType')),
              },
            });
        if(source.kind==='captured' && (String(data.get('responseHeaders')??'')!==this.savedHeaderText || Number(data.get('status'))!==asset.status || String(data.get('mediaType')??'')!==(asset.mediaType??''))) {
          asset=await invoke<ResponseAsset>('edit_response_asset',{input:{assetReference:`${asset.id}@${asset.revision}`,status:Number(data.get('status')),headers:responseHeaders,mediaType:optionalText(data.get('mediaType')),decodedBody:null,bodyPath:null,preserveContentEncoding:preserveEncoding}});
        }
        assetReference = `${asset.id}@${asset.revision}`;
      }

      const id = existing?.id ?? `autoresponse-rule-${crypto.randomUUID().replaceAll('-', '')}`;
      const rule: AutomationRule = {
        id,
        displayName,
        enabled: this.autoResponseEnabled.checked,
        revision: (existing?.revision ?? 0) + 1,
        priority: existing?.priority ?? AUTORESPONSE_PRIORITY_BASE,
        matcher,
        request: {
          headers: [], replaceBody: null, discardBody: false, abortReason: null,
          responseAsset: assetReference, allowNonIdempotentBodyReplacement: false,
        },
        response: existing?.response??{ headers: [], replaceBody: null, discardBody: false, abortReason: null },
      };
      const ordered = this.autoResponseRules(current).filter((candidate) => candidate.id !== id);
      if (existing === undefined) ordered.unshift(rule);
      else ordered.splice(Math.max(0, this.autoResponseRules(current).findIndex((candidate) => candidate.id === id)), 0, rule);
      const status = await this.activateAutoResponseOrder(current, ordered);
      this.rememberRuleChange(this.autoResponseRules(current),this.autoResponseRules(status),existing?'Saved rule properties.':'Created a new rule.');
      this.renderAutomation(status);
      this.savingAutoResponse = false;
      this.draftDirty=false;await this.cancelAutoResponseEdit();
      this.ruleSelection.replace(id);this.renderRuleSelection();
      this.automationText = `Saved “${rule.displayName}”${existing === undefined ? ' at the top' : ''}. The first enabled matching rule will win.`;
      this.automationKind = 'success';
      this.focusRule(id);
      const saved=status.rules.find(rule=>rule.id===id);if(saved)await this.editAutoResponse(saved,true);
      return true;
    } catch (error: unknown) {
      this.automationText = `Auto-response could not be saved: ${describeError(error)}`;
      this.automationKind = 'error';
      this.autoResponseError = describeError(error);
      return false;
    } finally {
      this.savingAutoResponse = false;
      this.$flushUpdates();
      if (this.requestHeadersError) (this.autoResponseForm.elements.namedItem('requestHeaders') as HTMLTextAreaElement).focus();
      else if (this.responseHeadersError) (this.autoResponseForm.elements.namedItem('responseHeaders') as HTMLTextAreaElement).focus();
    }
  }

  private autoResponseRules(status: AutomationStatus): AutomationRule[] {
    return status.rules
      .filter((rule) => rule.request.responseAsset !== null)
      .sort((left, right) => left.priority - right.priority || left.id.localeCompare(right.id));
  }

  private async activateAutoResponseOrder(current: AutomationStatus, ordered: AutomationRule[]): Promise<AutomationStatus> {
    const normalized = ordered.map((rule, index) => {
      const priority = AUTORESPONSE_PRIORITY_BASE + index;
      return rule.priority === priority ? rule : { ...rule, revision: rule.revision + 1, priority };
    });
    const ids = new Set(normalized.map((rule) => rule.id));
    const other = current.rules.filter((rule) => rule.request.responseAsset === null && !ids.has(rule.id));
    return this.activateRuleSet([...normalized, ...other], current.generation);
  }

  async mutateAutoResponse(ruleId: string, mutation: 'toggle' | 'remove' | 'up' | 'down'): Promise<void> {
    this.ruleSelection.replace(ruleId);this.renderRuleSelection();
    const enabled=this.automationStatus?.rules.find(rule=>rule.id===ruleId)?.enabled??true;
    await this.mutateSelectedRules(mutation==='toggle'?enabled?'disable':'enable':mutation);
  }

  private focusRule(id:string): void {
    this.$flushUpdates();
    const element = (this.getRootNode() as ShadowRoot).getElementById('rule-'+id);
    element?.scrollIntoView({block:'nearest'}); element?.focus({preventScroll:true});
  }

  async undoRemoveAutoResponse(): Promise<void> {
    const action=this.ruleHistory.at(-1);
    if (!action || this.savingAutoResponse || this.changingAutoResponse || !await this.canLeaveEditor()) return;
    this.changingAutoResponse = true;
    try {
      const current = await invoke<AutomationStatus>('automation_status');
      if(JSON.stringify(this.autoResponseRules(current))!==JSON.stringify(action.after))throw new Error('Rules changed outside this action. Refresh before undoing.');
      const ordered=action.before.map(rule=>({...rule,revision:Math.max(rule.revision,current.rules.find(item=>item.id===rule.id)?.revision??0)+1}));
      const status=await this.activateAutoResponseOrder(current,ordered);this.ruleHistory.pop();
      const earlier=this.ruleHistory.at(-1);if(earlier)earlier.after=structuredClone(this.autoResponseRules(status));
      this.undoRemoveText=earlier?.label??'';this.renderAutomation(status);
      const restored=ordered.filter(rule=>!action.after.some(item=>item.id===rule.id)).map(rule=>rule.id);
      const selected=restored.length?restored:ordered.filter(rule=>this.ruleSelection.ids.has(rule.id)).map(rule=>rule.id);
      this.ruleSelection.ids=new Set(selected);this.renderRuleSelection();this.changingAutoResponse=false;
      if(selected.length===1){const rule=status.rules.find(rule=>rule.id===selected[0]);if(rule)await this.editAutoResponse(rule,true);}
      if(selected[0])this.focusRule(selected[0]);
    } catch (error:unknown) {
      this.automationText = 'Rule could not be restored: '+describeError(error); this.automationKind = 'error';
    } finally { this.changingAutoResponse = false; }
  }

  async editAutoResponse(rule: AutomationRule,force=false): Promise<void> {
    if (this.savingAutoResponse) return;
    if(!force && rule.id===this.editingAutoResponseId && !this.editorHidden)return;
    if(!force && !await this.canLeaveEditor())return;
    this.prepareEditor();
    const revision = this.editorRevision;
    const url = rule.matcher.url?.kind === 'exact' ? rule.matcher.url.value : rule.matcher.url?.kind==='pattern'?rule.matcher.url.value.address:'https://'+(rule.matcher.host??'example.test')+(rule.matcher.pathPrefix??'/');
    this.editingAutoResponseId = rule.id;
    this.autoResponseSourceState = { kind: 'existing', assetReference: rule.request.responseAsset ?? '' };
    const elements = this.autoResponseForm.elements;
    (elements.namedItem('name') as HTMLInputElement).value = rule.displayName ?? 'Auto-response';
    this.setEditorMethod(rule.matcher.method);
    (elements.namedItem('url') as HTMLInputElement).value = url;
    this.matcherCondition=rule.matcher.url??null;this.matchExamples=rule.matcher.examples??[];this.matchTestUrl.value=url;this.autoResponseEnabled.checked=rule.enabled??true;
    this.matchTestMethod.value=rule.matcher.method??'GET';
    const extras=[rule.matcher.scheme?`Scheme: ${rule.matcher.scheme}`:'',rule.matcher.host?`Host: ${rule.matcher.host}`:'',rule.matcher.port?`Port: ${rule.matcher.port}`:'',rule.matcher.pathPrefix?`Path prefix: ${rule.matcher.pathPrefix}`:'',rule.matcher.query!==null?`Exact query: ${rule.matcher.query}`:'',...rule.matcher.requestHeaders.filter(header=>header.condition.kind!=='exact').map(header=>`${header.name}: ${header.condition.kind}`)].filter(Boolean);
    this.advancedMatcherText=extras.join('\n');
    this.ruleSelection.replace(rule.id);this.renderRuleSelection();
    (elements.namedItem('requestHeaders') as HTMLTextAreaElement).value = rule.matcher.requestHeaders
      .filter((header) => header.condition.kind === 'exact')
      .map((header) => `${header.name}: ${new TextDecoder().decode(new Uint8Array(Array.isArray(header.condition.value)?header.condition.value:[]))}`)
      .join('\n');
    this.matchTestHeaders.value=(elements.namedItem('requestHeaders') as HTMLTextAreaElement).value;
    this.editorTitle = `Edit “${rule.displayName ?? 'Auto-response'}”`;
    this.sourceText = 'Loading saved response…';
    this.responseReadOnly = true;
    this.responseFieldsHidden = true;
    this.responseHeadersHidden = true;
    this.capturedOptionsHidden = true;
    this.bodyHidden = true;
    this.autoResponseBody.value = '';
    this.bodyReadOnly = true;
    this.loadingSavedResponse=true;
    this.updateAutoResponseHeaderFilterState();
    this.openEditor(false);
    const initialIdentity=JSON.parse(this.nonBodySnapshot()) as Record<string,unknown>;
    try {
      const inspection = await invoke<ResponseAssetInspection>('inspect_response_asset',{reference:rule.request.responseAsset});
      if (revision !== this.editorRevision || !this.isConnected) return;
      const asset=inspection.asset;this.savedResponse=inspection;this.existingResponse=true;
      this.assetsIndex.set(`${asset.id}@${asset.revision}`,asset);this.renderAutoResponseRules(this.automationStatus??{generation:0,rules:[],autoresponsesEnabled:true,diagnostics:[],candidateCount:0,historyCount:0});
      this.sourceText = asset.provenance.kind==='session' ? 'Saved response created from Traffic. Original details are available through the source link while retained.' : `${asset.status} · ${formatBytes(asset.bodyBytes)} · ${asset.mediaType ?? 'Unknown content type'} · Saved response`;
      this.autoResponseStatus.value = String(asset.status);
      this.autoResponseMediaType.value = asset.mediaType ?? '';
      this.responseReadOnly=false;this.responseHeadersHidden=false;this.bodyHidden=false;
      this.autoResponseBody.value=inspection.display;this.bodyReadOnly=inspection.textEncoding===null;this.editBodyDisabled=inspection.textEncoding===null;
      if(asset.linkedBody){this.setResponseFile({path:asset.linkedBody.path,bytes:asset.bodyBytes,linked:true,error:null});void this.refreshResponseFile();}
      this.bodyStatusText=inspection.explanation;this.savedHeaderText=this.editableHeaders(asset);
      (elements.namedItem('responseHeaders') as HTMLTextAreaElement).value=this.savedHeaderText;
      this.encodingOptionsHidden=inspection.contentCodings.length===0;this.preserveEncodingDisabled=inspection.textEncoding===null;
      this.autoResponsePreserveEncoding.checked=asset.linkedBody?Boolean(asset.linkedBody.applyOriginalCompression):true;
      this.updateCompressionChoice();
      if(this.automationStatus)this.renderRuleUsage(this.automationStatus);
      this.responseFieldsHidden = false;
      const baseline=JSON.parse(this.nonBodySnapshot()) as Record<string,unknown>;
      for(const key of ['name','method','url','requestHeaders','retainAdvanced','examples'])baseline[key]=initialIdentity[key];
      this.savedNonBodySnapshot=JSON.stringify(baseline);
      if (asset.provenance.kind === 'session') {
        this.sourceSessionId = asset.provenance.exchange_id;
        this.sourceLabel = 'Original captured request';
        this.sourceAvailabilityText = 'Checking source in Traffic…';
        await this.refreshSourceAvailability();
      }
    } catch (error:unknown) {
      if (revision === this.editorRevision) this.sourceText = 'Saved response metadata could not be loaded: '+describeError(error);
    } finally {if(revision===this.editorRevision)this.loadingSavedResponse=false;}
  }

  async disableBuiltInAutomation(id: string): Promise<void> {
    try {
      const current = await invoke<AutomationStatus>('automation_status');
      const rules = current.rules.filter((rule) => rule.id !== id);
      if (rules.length === current.rules.length) {
        this.renderAutomation(current);
        return;
      }
      const status = await this.activateRuleSet(rules, current.generation);
      this.renderAutomation(status);
      this.diagnostic = 'Traffic action disabled for new requests.';
    } catch (error: unknown) {
      this.automationText = `Traffic action could not be disabled: ${describeError(error)}`;
      this.automationKind = 'error';
    }
  }

  private async activateRuleSet(
    rules: AutomationStatus['rules'],
    generation: number,
  ): Promise<AutomationStatus> {
    const current=await invoke<AutomationStatus>('automation_status');
    if (current.generation !== generation) throw new Error('Rules changed while you were editing. Refresh before saving.');
    const candidate = await invoke<AutomationCandidate>('validate_automation', {
      document: { schemaVersion: 1, generation, rules, autoresponsesEnabled:current.autoresponsesEnabled ?? true },
    });
    return invoke<AutomationStatus>('activate_automation', { candidateId: candidate.candidateId });
  }

  private renderAutomation(status: AutomationStatus): void {
    if(this.automationStatus && status.generation<this.automationStatus.generation)return;
    this.automationStatus = status;
    this.headerOverrideCount=status.rules.filter(rule=>rule.enabled!==false && !rule.request.responseAsset && (rule.request.userAgent || rule.request.headers.length || rule.response.headers.length)).length;
    this.$emit('automation-state-change',status);
    this.renderAutoResponseRules(status);
    const actions: string[] = [];
    const uaCount=status.rules.filter(rule=>rule.request.userAgent && rule.enabled!==false).length;
    if(uaCount)actions.push(`${uaCount} User-Agent / client-hint override${uaCount===1?'':'s'} enabled`);
    const autoResponses = this.autoResponseRules(status);
    const enabled=autoResponses.filter(rule=>rule.enabled!==false).length;
    if (autoResponses.length > 0) actions.push(status.autoresponsesEnabled===false?`Autoresponses paused (${enabled} enabled rules)`:`${enabled} of ${autoResponses.length} autoresponse rules enabled`);
    const other = status.rules.filter((rule) => rule.enabled!==false && !rule.request.userAgent && !rule.id.startsWith('desktop-') && rule.request.responseAsset === null).length;
    if (other > 0) actions.push(`${other} advanced rule${other === 1 ? '' : 's'}`);
    this.automationText = actions.length === 0
      ? 'No built-in traffic actions are active.'
      : `${actions.join(' · ')}.`;
    this.automationKind = 'success';
  }

}

AutomationWorkspace.define('automation-workspace');
