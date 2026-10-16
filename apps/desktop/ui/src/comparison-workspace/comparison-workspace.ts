import {observable} from '@microsoft/webui-framework';
import {invoke} from '@tauri-apps/api/core';
import {WorkspaceElement} from '../workspace-element.js';
import {describeError} from '../utilities.js';
import type {ComparisonOpen,ComparisonSubject,ComparisonOptions,ComparisonRow,ComparisonResult,ScopeChoice,CaptureOpen,PairingOptions,PairingRow,CaptureComparison} from '../comparison-models.js';

export class ComparisonWorkspace extends WorkspaceElement {
  comparisonDialog!:HTMLDialogElement;
  comparisonScroll!:HTMLElement;
  private cancellationRequested=false;
  private captureCancellationRequested=false;
  private pageRevision=0;
  private byteRevision=0;
  private changeRevision=0;
  @observable comparisonCaseAvailable=false;
  @observable comparisonLeftTime="";
  @observable comparisonRightTime="";
  @observable comparisonRange="";
  @observable captureRange="";
  captureDialog!:HTMLDialogElement;
  excludedHeaders!:HTMLInputElement;
  queryNames!:HTMLInputElement;
  pairingHeaders!:HTMLInputElement;
  counterpartSearch!:HTMLInputElement;
  @observable comparisonResult:ComparisonResult|null=null;
  @observable comparisonRows:ComparisonRow[]=[];
  @observable comparisonBusy=false;
  @observable comparisonStatus='';
  @observable comparisonSummary='';
  @observable comparisonPairingNote='';
  @observable comparisonSection='All';
  @observable comparisonSections=['All','Overview','Request','Response','Timings','Source/fidelity'];
  @observable comparisonOffset=0;
  @observable comparisonNext=false;
  @observable comparisonNormalize=false;
  @observable comparisonChangesOnly=false;
  @observable comparisonChangePosition='';
  @observable comparisonActive=-1;
  @observable comparisonByteSection='Response';
  @observable comparisonByteOffset=0;
  @observable comparisonByteLeft='';
  @observable comparisonByteRight='';
  @observable comparisonByteNote='';
  @observable comparisonByteNext=false;
  @observable comparisonLeftName='Left';
  @observable comparisonRightName='Right';
  @observable comparisonStale=false;
  @observable captureChoices:ScopeChoice[]=[];
  @observable captureLeft='';
  @observable captureRight='';
  @observable captureResult:CaptureComparison|null=null;
  @observable captureStatus='';
  @observable captureSummary='';
  @observable captureBusy=false;
  @observable captureOffset=0;
  @observable captureNext=false;
  @observable counterpartChoices:Array<{id:string;label:string}>=[];
  @observable counterpartRow:PairingRow|null=null;
  @observable counterpartValue='';
  @observable counterpartSide='right';
  @observable counterpartOffset=0;
  @observable counterpartMore=false;
  @observable pairingBodyDigest=false;
  private subjects:ComparisonOpen|null=null;
  private pageRows:ComparisonRow[]=[];
  private revision=0;
  private captureRevision=0;
  private operation='';
  private captureOperation='';
  private returnFocus:HTMLElement|null=null;
  private captureReturnFocus:HTMLElement|null=null;
  private staleTimer:number|undefined;
  private choiceRevision=0;
  private options():ComparisonOptions{return {excludedHeaders:this.excludedHeaders.value.split(',').map(s=>s.trim()).filter(Boolean),normalizeNewlines:this.comparisonNormalize};}
  private pairOptions():PairingOptions{return {excludedQueryNames:this.queryNames.value.split(',').map(s=>s.trim()).filter(Boolean),requestHeaders:this.pairingHeaders.value.split(',').map(s=>s.trim()).filter(Boolean),requestBodyDigest:this.pairingBodyDigest};}
  private activeElement():HTMLElement|null{return (this.getRootNode() as ShadowRoot).activeElement as HTMLElement|null;}
  async open(input:ComparisonOpen):Promise<void>{
    if(!this.comparisonDialog.open){this.returnFocus=this.activeElement();this.comparisonDialog.showModal();this.comparisonSection='All';}
    this.comparisonPairingNote="";this.subjects={left:structuredClone(input.left),right:structuredClone(input.right)};this.comparisonCaseAvailable=input.left.kind==='exchange'&&input.right.kind==='exchange';this.excludedHeaders.value=input.options?.excludedHeaders.join(', ')??"";this.comparisonNormalize=input.options?.normalizeNewlines??false;this.comparisonLeftName="Left";this.comparisonRightName="Right";await this.refreshComparison(false);
  }
  comparisonMore!:HTMLElement;
  comparisonMoreButton!:HTMLButtonElement;
  captureMore!:HTMLElement;
  captureMoreButton!:HTMLButtonElement;
  saveCase(captures=false):void {
    const menu=captures?this.captureMore:this.comparisonMore,button=captures?this.captureMoreButton:this.comparisonMoreButton;
    menu.hidePopover();button.focus();
    const id=captures?this.captureResult?.id:this.comparisonResult?.id;
    if(id)this.$emit('save-comparison-case',{id,returnFocus:button});
  }
  async openCaseCaptures(id:string):Promise<void>{
    this.captureReturnFocus=this.activeElement();this.captureChoices=[];this.captureDialog.showModal();this.captureBusy=true;
    const revision=++this.captureRevision,operationId=crypto.randomUUID();this.captureOperation=operationId;this.captureCancellationRequested=false;this.captureStatus='Recomputing the saved case scopes and pairing corrections…';
    try{const result=await invoke<CaptureComparison>('reopen_case_capture_comparison',{id,operationId});if(revision!==this.captureRevision||!this.isConnected||this.captureCancellationRequested){void invoke('close_capture_comparison',{id:result.id}).catch(()=>{});return;}const previous=this.captureResult?.id;this.queryNames.value=result.options.excludedQueryNames.join(', ');this.pairingHeaders.value=result.options.requestHeaders.join(', ');this.pairingBodyDigest=result.options.requestBodyDigest;this.updateCapture(result,0);if(previous)void invoke('close_capture_comparison',{id:previous}).catch(()=>{});}
    catch(error:unknown){if(revision===this.captureRevision)this.captureStatus=describeError(error);}finally{if(revision===this.captureRevision){this.captureBusy=false;this.captureOperation='';}}
  }
  async refreshComparison(preserve=true):Promise<void>{
    if(!this.subjects)return;
    const anchor=preserve?this.pageRows.find(row=>row.index===this.comparisonActive):undefined;
    this.cancelPending();const revision=++this.revision,operation=crypto.randomUUID();this.operation=operation;this.cancellationRequested=false;this.comparisonBusy=true;this.comparisonStatus='Acquiring evidence and comparing…';
    let pendingId='';
    try{
      const result=await invoke<ComparisonResult>('compare_evidence',{input:{operationId:operation,...this.subjects,options:this.options()}});pendingId=result.id;
      let rows=result.rows,offset=0,active:ComparisonRow|undefined;
      if(anchor){
        for(let page=0;page<8;page++){
          active=rows.find(row=>row.section===anchor.section&&row.field===anchor.field);if(active||offset+64>=result.totalRows)break;
          if(revision!==this.revision||this.cancellationRequested)break;
          offset+=64;rows=(await invoke<{rows:ComparisonRow[]}>('comparison_page',{id:result.id,offset})).rows;
        }
        if(!active){offset=0;rows=result.rows;}
      }
      if(revision!==this.revision||!this.isConnected||this.cancellationRequested){void invoke('close_comparison',{id:result.id}).catch(()=>{});pendingId='';if(revision===this.revision)this.comparisonStatus='Comparison canceled; previous completed result preserved.';return;}
      const previous=this.comparisonResult?.id;this.comparisonResult=result;pendingId='';
      this.comparisonSummary=(result.different?'Differences found':'No differences in compared evidence')+` · ${result.changed} change findings · ${result.unknown} unavailable/not compared · ${result.excluded} excluded`+(result.moreChanges?' · Change output limited':'');
      this.comparisonLeftTime=new Date(result.left.acquiredAt).toISOString();this.comparisonRightTime=new Date(result.right.acquiredAt).toISOString();
      this.comparisonStale=false;this.comparisonOffset=offset;this.comparisonActive=active?.index??-1;this.comparisonChangePosition=active?`Change at evidence row ${active.index+1} of ${result.totalRows}`:'';
      this.comparisonByteLeft='';this.comparisonByteRight='';this.comparisonByteNext=false;this.comparisonByteOffset=0;this.comparisonByteSection=result.bodySections.includes('Response')?'Response':result.bodySections[0]??'Response';
      this.pageRows=rows;this.renderRows();this.comparisonNext=offset+64<result.totalRows;this.comparisonStatus=result.complete?'All applicable evidence compared.':'Coverage is limited; inspect unavailable, excluded and not-compared evidence.';
      this.$flushUpdates();if(!active)this.comparisonScroll.scrollTop=0;
      if(previous)void invoke('close_comparison',{id:previous}).catch(()=>{});this.monitorSource();
    }catch(error:unknown){if(pendingId)void invoke('close_comparison',{id:pendingId}).catch(()=>{});if(revision===this.revision){this.comparisonStatus=describeError(error);if(!this.comparisonStatus.toLowerCase().includes('cancel'))this.$emit('operation-error',{title:'Comparison failed',message:this.comparisonStatus});}}
    finally{if(revision===this.revision){this.operation='';this.comparisonBusy=false;}}
  }
  private monitorSource():void{window.clearTimeout(this.staleTimer);const id=this.comparisonResult?.id;if(!id||!this.comparisonDialog.open)return;this.staleTimer=window.setTimeout(()=>{void invoke<boolean>('comparison_stale',{id}).then(stale=>{if(this.comparisonResult?.id===id)this.comparisonStale=stale;}).catch(()=>{if(this.comparisonResult?.id===id)this.comparisonStale=true;}).finally(()=>this.monitorSource());},2000);}
  cancelPending():void {if(this.operation){void invoke('cancel_comparison',{operationId:this.operation}).catch(()=>{});}}
  cancelComparison():void {this.cancellationRequested=true;this.cancelPending();this.comparisonStatus='Canceling comparison; the previous completed result is preserved.';}
  comparisonClosed():void{if(this.comparisonDialog?.open&&this.isConnected)return;this.cancelPending();this.revision++;this.comparisonBusy=false;this.operation='';window.clearTimeout(this.staleTimer);const id=this.comparisonResult?.id;this.comparisonResult=null;this.comparisonRows=[];this.pageRows=[];this.subjects=null;this.comparisonPairingNote="";this.comparisonSummary="";this.comparisonChangePosition="";this.comparisonActive=-1;this.comparisonNext=false;this.comparisonByteLeft="";this.comparisonByteRight="";this.comparisonByteNote="";this.comparisonByteNext=false;if(id)void invoke('close_comparison',{id}).catch(()=>{});if(this.returnFocus?.isConnected)this.returnFocus.focus({preventScroll:true});this.returnFocus=null;}
  closeComparison():void{this.comparisonDialog.close();}
  swapSides():void{if(!this.subjects)return;this.subjects={left:this.subjects.right,right:this.subjects.left};const name=this.comparisonLeftName;if(name!=="Left"||this.comparisonRightName!=="Right"){this.comparisonLeftName=this.comparisonRightName;this.comparisonRightName=name;}void this.refreshComparison(false);}
  renameSide(side:string,event:Event):void{const value=(event.currentTarget as HTMLInputElement).value;if(side==='left')this.comparisonLeftName=value;else this.comparisonRightName=value;}
  setSection(event:Event):void{this.comparisonSection=(event.currentTarget as HTMLSelectElement).value;this.renderRows();}
  setChangesOnly(event:Event):void{this.comparisonChangesOnly=(event.currentTarget as HTMLInputElement).checked;this.renderRows();}
  setNormalize(event:Event):void{this.comparisonNormalize=(event.currentTarget as HTMLInputElement).checked;}
  boundary(requestBoundary:string,responseBoundary:string):void{if(!this.subjects)return;const apply=(s:ComparisonSubject):ComparisonSubject=>s.kind==='exchange'?{...s,requestBoundary,responseBoundary}:s;this.subjects={left:apply(this.subjects.left),right:apply(this.subjects.right)};void this.refreshComparison();}
  private renderRows():void{this.comparisonRange=this.comparisonResult?`Evidence rows ${this.comparisonOffset+1}–${Math.min(this.comparisonOffset+64,this.comparisonResult.totalRows)} of ${this.comparisonResult.totalRows}`:'';this.comparisonRows=this.pageRows.filter(r=>(this.comparisonSection==='All'||r.section===this.comparisonSection||r.section===this.comparisonSection+' trailers')&&(!this.comparisonChangesOnly||r.state!=='equal'));}
  async page(direction:number):Promise<void>{const result=this.comparisonResult;if(!result)return;const offset=Math.max(0,this.comparisonOffset+direction*64),revision=this.revision,pageRevision=++this.pageRevision;try{const page=await invoke<{rows:ComparisonRow[];nextOffset:number|null}>('comparison_page',{id:result.id,offset});if(revision!==this.revision||pageRevision!==this.pageRevision||this.comparisonResult?.id!==result.id)return;this.pageRows=page.rows;this.comparisonOffset=offset;this.comparisonNext=page.nextOffset!==null;this.renderRows();}catch(error:unknown){this.comparisonStatus=describeError(error);}}
  async change(direction:number):Promise<void>{const changeRevision=++this.changeRevision;const result=this.comparisonResult;if(!result)return;let position=this.comparisonActive;if(position<0)position=direction>0?-1:result.totalRows;
    for(let scanned=0;scanned<9;scanned++){
      const candidate=this.pageRows.filter(r=>r.state==='different'&&(direction>0?r.index>position:r.index<position));const row=direction>0?candidate[0]:candidate.at(-1);
      if(row){this.comparisonSection='All';this.comparisonChangesOnly=false;this.comparisonActive=row.index;this.renderRows();this.comparisonChangePosition=`Change at evidence row ${row.index+1} of ${result.totalRows}`;this.$flushUpdates();const root=this.getRootNode() as ShadowRoot;const target=root.getElementById('comparison-change-'+row.index);target?.focus({preventScroll:true});target?.scrollIntoView({block:'start'});return;}
      if(direction>0?!this.comparisonNext:this.comparisonOffset===0)break;await this.page(direction);if(changeRevision!==this.changeRevision||this.comparisonResult?.id!==result.id)return;
    }this.comparisonChangePosition=direction>0?'No further changes in retained result output.':'No earlier changes in retained result output.';
  }
  comparisonKeyboard(event:KeyboardEvent):void{if(event.altKey&&['ArrowDown','ArrowUp'].includes(event.key)){event.preventDefault();void this.change(event.key==='ArrowDown'?1:-1);}}
  showSource(id:string|null):void{if(id){this.comparisonStatus='Opening original source…';this.$emit('comparison-source-request',id);}}
  sourceOpened():void{this.returnFocus=null;this.captureReturnFocus=null;this.closeComparison();if(this.captureDialog.open)this.captureDialog.close();}
  setByteSection(event:Event):void{this.comparisonByteSection=(event.currentTarget as HTMLSelectElement).value;void this.readBytes(0);}
  async readBytes(direction:number):Promise<void>{const id=this.comparisonResult?.id;if(!id)return;const offset=direction===0?0:Math.max(0,this.comparisonByteOffset+direction*256),revision=this.revision,byteRevision=++this.byteRevision;try{const bytes=await invoke<{left:string;right:string;offset:number;nextOffset:number|null;note:string}>('comparison_bytes',{id,section:this.comparisonByteSection,offset});if(revision!==this.revision||byteRevision!==this.byteRevision)return;this.comparisonByteOffset=bytes.offset;this.comparisonByteLeft=bytes.left;this.comparisonByteRight=bytes.right;this.comparisonByteNext=bytes.nextOffset!==null;this.comparisonByteNote=bytes.note;}catch(error:unknown){this.comparisonStatus=describeError(error);}}
  async openCaptures(input:CaptureOpen):Promise<void>{this.captureReturnFocus=this.activeElement();this.captureChoices=input.choices;this.captureLeft=input.choices[0]?.value??'';this.captureRight=input.choices[1]?.value??this.captureLeft;this.queryNames.value='';this.pairingHeaders.value='';this.pairingBodyDigest=false;this.captureStatus='Choose the two scopes. New traffic is excluded once comparison starts.';if(!this.captureDialog.open)this.captureDialog.showModal();}
  selectScope(side:string,event:Event):void{if(side==='left')this.captureLeft=(event.currentTarget as HTMLSelectElement).value;else this.captureRight=(event.currentTarget as HTMLSelectElement).value;}
  async runCaptures():Promise<void>{const left=this.captureChoices.find(c=>c.value===this.captureLeft)?.scope,right=this.captureChoices.find(c=>c.value===this.captureRight)?.scope;if(!left||!right)return;this.cancelCapture();const revision=++this.captureRevision,operation=crypto.randomUUID();this.captureOperation=operation;this.captureCancellationRequested=false;this.captureBusy=true;this.captureStatus='Snapshotting scope metadata and pairing requests…';try{const result=await invoke<CaptureComparison>('compare_captures',{operationId:operation,left,right,options:this.pairOptions()});if(revision!==this.captureRevision||!this.isConnected||this.captureCancellationRequested){if(revision===this.captureRevision)this.captureStatus="Comparison canceled; previous completed result preserved.";void invoke('close_capture_comparison',{id:result.id}).catch(()=>{});return;}const previous=this.captureResult?.id;this.updateCapture(result,0);if(previous)void invoke('close_capture_comparison',{id:previous}).catch(()=>{});}catch(error:unknown){if(revision===this.captureRevision)this.captureStatus=describeError(error);}finally{if(revision===this.captureRevision){this.captureBusy=false;this.captureOperation='';}}}
  cancelCapture():void{this.captureCancellationRequested=true;if(this.captureOperation)void invoke('cancel_comparison',{operationId:this.captureOperation}).catch(()=>{});}
  private updateCapture(result:CaptureComparison,offset:number):void{if(this.captureResult?.id!==result.id){this.choiceRevision++;this.counterpartRow=null;this.counterpartChoices=[];this.counterpartValue="";}this.captureResult=result;this.captureOffset=offset;this.captureRange=`Pair rows ${offset+1}–${Math.min(offset+100,result.total)} of ${result.total}`;this.captureNext=offset+100<result.total;this.captureSummary=`${result.leftCount} left / ${result.rightCount} right · ${result.changed} changed metadata · ${result.equalMetadata} equal metadata · ${result.unresolved} unresolved · ${result.leftOnly} left only · ${result.rightOnly} right only · ${result.unavailable} unavailable`;this.captureStatus=result.note;}
  async capturePage(direction:number,action='',left:string|null=null,right:string|null=null,criteria=false):Promise<void>{const id=this.captureResult?.id;if(!id)return;const revision=++this.captureRevision,offset=criteria?0:Math.max(0,this.captureOffset+direction*100);this.captureBusy=true;try{const result=await invoke<CaptureComparison>('capture_comparison_page',{id,offset,options:criteria?this.pairOptions():null,left,right,action});if(revision!==this.captureRevision)return;this.updateCapture(result,offset);}catch(error:unknown){if(revision===this.captureRevision)this.captureStatus=describeError(error);}finally{if(revision===this.captureRevision)this.captureBusy=false;}}
  async openPair(row:PairingRow):Promise<void>{if(!row.left||!row.right)return;const options=this.captureResult?.options;await this.open({left:{kind:'exchange',id:row.left},right:{kind:'exchange',id:row.right}});if(this.subjects?.left.kind!=='exchange'||this.subjects.left.id!==row.left)return;this.comparisonPairingNote=row.reason+' · Pairing ignores query names: '+(options?.excludedQueryNames.join(', ')||'none')+'; requires request headers: '+(options?.requestHeaders.join(', ')||'none')+(options?.requestBodyDigest?'; requires complete request-body digest':'')+'. These criteria choose counterparts; original URL/header differences below remain visible.';}
  keepUnmatched(row:PairingRow):void{void this.capturePage(0,'unmatched',row.left,row.right);}
  async chooseCounterpart(row:PairingRow):Promise<void>{this.counterpartRow=row;this.counterpartSide=row.left?'right':'left';this.$flushUpdates();this.counterpartSearch.value='';this.counterpartOffset=0;await this.loadCounterparts(0);this.counterpartSearch.focus();}
  async loadCounterparts(direction:number):Promise<void>{const id=this.captureResult?.id;if(!id)return;const revision=++this.choiceRevision,offset=Math.max(0,this.counterpartOffset+direction*100);try{const choices=await invoke<Array<{id:string;label:string}>>('capture_comparison_entries',{id,side:this.counterpartSide,search:this.counterpartSearch.value,offset});if(revision!==this.choiceRevision)return;this.counterpartOffset=offset;this.counterpartChoices=choices;this.counterpartValue=choices[0]?.id??'';this.counterpartMore=choices.length===100;}catch(error:unknown){this.captureStatus=describeError(error);}}
  counterpartChanged(event:Event):void{this.counterpartValue=(event.currentTarget as HTMLSelectElement).value;}
  async applyCounterpart():Promise<void>{const row=this.counterpartRow;if(!row||!this.counterpartValue)return;await this.capturePage(0,'pair',row.left??this.counterpartValue,row.left?this.counterpartValue:row.right);this.counterpartRow=null;}
  closeCounterpart():void{this.choiceRevision++;this.counterpartRow=null;}
  setPairBody(event:Event):void{this.pairingBodyDigest=(event.currentTarget as HTMLInputElement).checked;}
  captureClosed():void{if(this.captureDialog?.open&&this.isConnected)return;this.cancelCapture();this.captureRevision++;this.choiceRevision++;this.captureBusy=false;this.captureOperation='';const id=this.captureResult?.id;this.captureResult=null;this.counterpartRow=null;this.captureChoices=[];this.counterpartChoices=[];this.counterpartValue="";this.captureSummary="";if(id)void invoke('close_capture_comparison',{id}).catch(()=>{});if(this.captureReturnFocus?.isConnected)this.captureReturnFocus.focus({preventScroll:true});this.captureReturnFocus=null;}
  closeCaptures():void{this.captureDialog.close();}
  override disconnectedCallback():void{this.comparisonClosed();this.captureClosed();super.disconnectedCallback();}
}
ComparisonWorkspace.define('comparison-workspace');
