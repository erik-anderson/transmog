import {timingView, type TimingRow, type TimelineRow, type TransportView, type WaterfallRow} from '../timings.js';
import { attr, observable } from '@microsoft/webui-framework';
import { Channel, invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { WorkspaceElement } from '../workspace-element.js';
import type { AutomationStatus, ColumnId, Lifecycle, SessionSummary, SessionPage, SessionHint, SessionDetail, TrafficFilter, TrafficSort, WorkspacePreferences, RequestCommand, RequestCommandFormat } from '../models.js';
import { describeError, loadSessionDetail, clientResponseSource, autoResponseUnavailableReason } from '../utilities.js';
import { cellText, columnDefinitions, defaultWorkspace, displayColumns, statusTone } from '../table-model.js';
import {ListSelection,isTextEditing} from '../list-selection.js';
import type { TraceMetadata, TraceImportResult, TraceImportProgress, TrafficSearchResult, TrafficSearchProgress, TrafficSearchEntry, TrafficSearchMatch, CapturedPageReport } from '../models.js';

type Column = ReturnType<typeof displayColumns>[number] & {sortDirection:string;sortArrow:string};
type Row = SessionSummary & {tone:string;selectionState:string;rowLabel:string;cells:Array<{id:ColumnId;text:string;title:string;pinned:boolean;numeric:boolean;offsetCss:string;tone:string}>};

export class TrafficWorkspace extends WorkspaceElement {
  @attr({attribute:'viewer-mode',mode:'boolean'}) viewerMode = false;
  @observable timingTitle="Timings and transport";
  @observable timingSummary="";
  @observable timingError="";
  @observable timingBusy=false;
  @observable timingPhases:TimingRow[]=[];
  @observable timingWaterfall:WaterfallRow[]=[];
  @observable timingMinor:WaterfallRow[]=[];
  @observable timingWork:TimingRow[]=[];
  @observable timingRange="";
  @observable timingReportText="";
  timingReportPreview!:HTMLTextAreaElement;
  @observable timingReportVisible=false;
  @observable timingStatus="";
  @observable timingRows:TimelineRow[]=[];
  @observable timingTransports:TransportView[]=[];
  @observable timingSaved:TimingRow[]=[];
  timingDialog!:HTMLDialogElement;
  private timingId="";
  private timingGeneration=0;
  @observable previewPageAvailable=false;
  @observable previewPageBusy=false;
  @observable previewPageUrl='';
  @observable previewPageStatus='';
  @observable previewPageScope='original-trace';
  @observable previewReportAvailable=false;
  @observable previewReportBusy=false;
  @observable previewReportError='';
  @observable previewReportSummary='';
  @observable previewReportUrl='';
  @observable previewResourceRows:Array<{id:string;sourceId:string;sourceAvailable:boolean;method:string;url:string;source:string;size:string;time:string;decision:string}>=[];
  @observable previewRequestRows:Array<{id:string;method:string;url:string;outcome:string;reason:string}>=[];
  previewReportDialog!:HTMLDialogElement;
  private previewReportLabel='';
  private previewReportGeneration=0;
  private previewReportFocus:HTMLElement|null=null;

  previewPageDialog!:HTMLDialogElement;
  previewPageScripts!:HTMLInputElement;
  private previewPageId='';
  private previewPageOperation='';
  @observable savingTrace=false;
  @observable saveTraceStatus='';
  saveTraceDialog!:HTMLDialogElement;
  saveTraceForm!:HTMLFormElement;
  @observable importingTrace=false;
  @observable importStatus='';
  importButton!:HTMLButtonElement;
  @observable importPercent=0;
  @observable openedTraceName='';
  @observable traceMetadataRows:TraceMetadata[]=[];
  @observable metadataTraceOptions:Array<{id:string;label:string}>=[];
  @observable metadataTraceId='';
  @observable metadataTraceName='';
  @observable metadataSummary='';
  @observable metadataContext='';
  @observable metadataNetworkTitle='';
  @observable metadataNetworkSummary='';
  @observable metadataNetworkContext='';
  @observable metadataNotes:Array<{id:string;text:string}>=[];
  @observable metadataError='';
  @observable metadataBusy=false;
  openTraceDialog!:HTMLDialogElement;
  metadataDialog!:HTMLDialogElement;
  metadataSelector!:HTMLSelectElement;
  private metadataGeneration=0;
  private openedTracePath='';
  private importOperation='';
  private openedFileQueue:Array<{path:string;ask:boolean}>=[];
  private nativeUnlisteners:UnlistenFn[]=[];
  @attr view = 'traffic';
  @attr({attribute:'page-size'}) pageSize = '100';
  @attr lifecycle:Lifecycle = 'stopped';
  @attr pending = '';
  @observable preferences = defaultWorkspace();
  @observable autoresponseState:AutomationStatus|null=null;
  @observable autoresponsePending=false;
  @observable sessions:Row[] = [];
  @observable columns:Column[] = [];
  @observable settingsColumns:Array<{id:ColumnId;label:string;visible:boolean}> = [];
  @observable selectedSessionId:string | null = null;
  @observable selectedTrafficCount=0;
  @observable trafficSelectionText='';
  @observable trafficCreateLabel='Create auto-responses…';
  @observable removingTraffic=false;
  @observable trafficUndoText='';
  @observable trafficMenuX='12px';
  @observable trafficMenuY='80px';
  @observable windowsCommands=false;
  @observable commandBusy=false;
  @observable commandTitle='Copy request';
  @observable commandText='';
  @observable commandNotices:Array<{id:string;text:string}>=[];
  @observable commandStatus='';
  @observable commandFileRequired=false;
  @observable commandFileAvailable=false;
  commandDialog!:HTMLDialogElement;
  commandPreview!:HTMLTextAreaElement;
  private commandSourceId='';
  private commandFormat:RequestCommandFormat='curl';
  trafficMenu!:HTMLElement;
  trafficUndoButton!:HTMLButtonElement;
  private trafficSelection=new ListSelection();
  private selectedTraffic=new Map<string,SessionSummary>();
  private trafficUndoTimer:number|undefined;
  @observable private trafficUndo:Array<{ids:string[];rows:SessionSummary[];expiresAt?:number|undefined}>=[];
  @observable selectedDetail:SessionDetail | null = null;
  @observable selectedMethodText = '—';
  @observable selectedUrlText = 'Select a request';
  @observable selectedStatusText = 'No response';
  @observable selectedTone = 'pending';
  @observable inspectorSide = 'response';
  showInspectorSide(side:string):void {this.inspectorSide=side;}
  @observable reuseDisabled = true;
  @observable reuseTitle = '';
  @observable matchedRuleId = '';
  @observable matchedRuleLabel = 'Show matched rule';
  @observable sessionText = 'Loading captured traffic…';
  @observable sessionKind = 'progress';
  @observable followLatest = true;
  @observable followText = 'Showing captured traffic';
  @observable newTrafficCount = 0;
  @observable updatesPending = false;
  @observable totalMatched = 0;
  @observable pageIndex = 0;
  @observable pageLimit = 100;
  @observable pageEnd = 0;
  @observable sort:TrafficSort = {column:'started-at',direction:'descending'};
  @observable sortLabel = 'Newest first';
  @observable filters:TrafficFilter[] = [];
  @observable searchText = '';
  @observable searchingTraffic=false;
  @observable searchMode='text';
  @observable searchCaseSensitive=false;
  @observable searchIgnoreAccents=false;
  @observable searchMetadata=true;
  @observable searchRequestHeaders=true;
  @observable searchResponseHeaders=true;
  @observable searchRequestBodies=false;
  @observable matchBusy=false;
  @observable matchHasLocation=false;
  @observable matchError='';
  @observable matchEntryLabel='';
  @observable matchPosition='';
  @observable matchField='';
  @observable matchBefore='';
  @observable matchText='';
  @observable matchAfter='';
  @observable matchNote='';
  @observable matchHasPrevious=false;
  @observable matchHasNext=false;
  @observable matchHasPreviousEntry=false;
  @observable matchHasNextEntry=false;
  matchDialog!:HTMLDialogElement;
  private matchGeneration=0;
  private matchLocations:TrafficSearchMatch[]=[];
  private matchIndex=0;
  private matchEntryId='';
  private matchMore=false;
  private matchReturnFocus:HTMLElement|null=null;
  private searchCancelRequested=false;
  @observable searchBodies=true;
  @observable selectSearchMatches=false;
  @observable contentSearchStatus='';
  @observable contentSearchLabel='';
  @observable contentMatchCount=0;
  @observable contentSearchActive=false;
  private searchOperation='';
  private searchResultId:string|null=null;
  private searchMatchIds:string[]=[];
  private matchRevision=-1;
  @observable tableWidth = '1000px';
  @observable listShare = '45%';
  @observable requestShare = '45%';
  @observable dividerOrientation = 'horizontal';
  @observable menuColumn = columnDefinitions[0]!;
  @observable menuX = '12px';
  @observable menuY = '80px';
  @observable ascendingLabel = 'Sort A–Z';
  @observable descendingLabel = 'Sort Z–A';
  @observable pinLabel = 'Pin column';
  @observable draggingColumn = '';
  @observable dropColumnId = '';
  @observable dropPlacement = '';
  @observable filterDraft = '';
  filterForm!:HTMLFormElement;
  sessionScroller!:HTMLElement;
  trafficTable!:HTMLTableElement;
  columnMenu!:HTMLElement;
  filterInput!:HTMLInputElement;
  filterOperator!:HTMLSelectElement;
  searchInput!:HTMLInputElement;
  private watching = false;
  private sessionUpdates:Channel<SessionHint> | null = null;
  private queryPending:Promise<void> | null = null;
  private queryAgain = false;
  private forceUpdate = false;
  private pendingRows:SessionSummary[] = [];
  private inspectionGeneration = 0;
  private queryRevision = 0;
  private resize: {id:ColumnId;startX:number;width:number;previous:number;pointer:number} | null = null;
  private columnDrag: {id:ColumnId;startX:number;startY:number;pointer:number;target:ColumnId|null;active:boolean;handle:HTMLElement} | null = null;
  private suppressMenuUntil = 0;
  private searchTimer:number | undefined;
  private layoutObserver:ResizeObserver | null = null;
  private displayedMatched = 0;
  private liveInspection:Promise<void> | null = null;
  private inspectionAgain = false;
  private queryLoaded = false;
  private queryError = '';
  private watchError = '';

  protected hydratedCallback():void {
    this.layoutObserver = new ResizeObserver(() => { this.measurePinnedColumns(); });
    this.layoutObserver.observe(this.sessionScroller);
    void this.watchSessions();
    void this.watchOpenedTraces();
    void invoke<{windows:boolean}>('desktop_bootstrap').then(bootstrap=>{if(this.isConnected)this.windowsCommands=bootstrap.windows;}).catch(()=>{});
  }
  lifecycleChanged():void { this.renderSessionState(); this.renderFollowState(); }
  viewerModeChanged():void {this.renderSessionState();this.renderFollowState();}
  traceMetadataRowsChanged():void {this.metadataTraceOptions=this.traceMetadataRows.map((trace,index)=>({id:trace.id,label:`${index+1}. ${trace.name} · ${trace.sessions} ${trace.sessions===1?'entry':'entries'}`}));}

  private async watchOpenedTraces():Promise<void> {
    try {
      const opened=await listen('trace-open-request',()=>{void this.takeOpenedTraces();});
      if(!this.isConnected){opened();return;}this.nativeUnlisteners.push(opened);
      const dropped=await getCurrentWebview().onDragDropEvent(event=>{
        if(event.payload.type!=='drop'||this.view!=='traffic')return;
        const point=event.payload.position, bounds=this.sessionScroller.getBoundingClientRect(), scale=window.devicePixelRatio;
        if(point.x/scale<bounds.left||point.x/scale>bounds.right||point.y/scale<bounds.top||point.y/scale>bounds.bottom)return;
        const paths=event.payload.paths;
        this.openedFileQueue.push(...paths.slice(0,16).map(path=>({path,ask:false})));
        void this.processOpenedTraces();
      });
      if(!this.isConnected){dropped();return;}this.nativeUnlisteners.push(dropped);
    } catch { /* Browser-hosted verification has no native file events. */ }
    await this.takeOpenedTraces();
  }
  private async takeOpenedTraces():Promise<void> {
    try {const paths=await invoke<string[]>('take_opened_traces');if(!this.isConnected)return;this.openedFileQueue.push(...paths.map(path=>({path,ask:!this.viewerMode})));await this.processOpenedTraces();}
    catch(error:unknown){this.importStatus='Opened capture could not be read: '+describeError(error);await this.showError('Could not open capture',describeError(error));}
  }
  private async processOpenedTraces():Promise<void> {
    while(this.isConnected&&!this.importingTrace&&!this.openTraceDialog.open&&this.openedFileQueue.length){
      const file=this.openedFileQueue.shift()!;
      if(file.ask){this.openedTracePath=file.path;this.openedTraceName=file.path.split(/[\\/]/).pop()??file.path;this.openTraceDialog.showModal();return;}
      await this.importTrace(file.path);
    }
  }
  async chooseOpenedTrace(separate:boolean):Promise<void> {
    const path=this.openedTracePath;this.openedTracePath='';this.openTraceDialog.close();
    if(separate){try{await invoke('open_trace_viewer',{paths:[path]});}catch(error:unknown){this.importStatus='Capture viewer could not be opened: '+describeError(error);await this.showError('Could not open capture viewer',describeError(error));}}
    else await this.importTrace(path);
    await this.processOpenedTraces();
  }
  cancelOpenedTrace(event?:Event):void {event?.preventDefault();this.openedTracePath='';this.openTraceDialog.close();void this.processOpenedTraces();}
  async openSavedTrace(separate:boolean):Promise<void> {
    if(this.importingTrace)return;
    try {const path=await invoke<string|null>('pick_trace_path');if(!path||!this.isConnected)return;if(separate)await invoke('open_trace_viewer',{paths:[path]});else{this.openedFileQueue.push({path,ask:false});await this.processOpenedTraces();}}
    catch(error:unknown){this.importStatus='Capture could not be opened: '+describeError(error);await this.showError('Could not open capture',describeError(error));}
  }
  private async importTrace(path:string):Promise<void> {
    this.importingTrace=true;this.importPercent=0;this.importStatus='Indexing '+(path.split(/[\\/]/).pop()??'capture')+'…';
    const operationId=crypto.randomUUID();this.importOperation=operationId;
    let failure:string|null=null;
    const onProgress=new Channel<TraceImportProgress>();onProgress.onmessage=progress=>{if(this.isConnected&&this.importOperation===progress.operationId)this.importPercent=progress.total?Math.min(99,Math.floor(progress.completed/progress.total*100)):0;};
    try {
      const result=await this.withTracePassword('Open '+(path.split(/[\\/]/).pop()??'capture'),password=>invoke<TraceImportResult>('import_trace',{request:{path,password,operationId},onProgress}));
      if(result===null){this.importStatus='Import canceled. Traffic is unchanged.';return;}
      if(!this.isConnected||this.importOperation!==operationId)return;
      this.importPercent=100;this.importStatus=`Imported ${result.trace.sessions} ${result.trace.sessions===1?'entry':'entries'} from ${result.trace.name}.${result.issues.length?' Some saved evidence is incomplete; see Trace metadata.':''}`;
      this.traceMetadataRows=[...this.traceMetadataRows,result.trace];this.queryRevision++;await this.refreshSessions(undefined,true);
    }catch(error:unknown){if(this.isConnected&&this.importOperation===operationId){const message=describeError(error);if(message.includes('canceled'))this.importStatus='Import canceled. Traffic is unchanged.';else{failure=message;this.importStatus='Import failed: '+message;}}}
    finally{onProgress.onmessage=()=>{};if(this.importOperation===operationId){this.importOperation='';this.importingTrace=false;}}
    if(failure&&this.isConnected){await this.showError('Import failed',(path.split(/[\\/]/).pop()??path)+'\n\n'+failure+'\n\nTraffic is unchanged. Choose another capture or try importing again.');if(this.isConnected&&this.view==='traffic')this.importButton.focus();}
  }
  dismissImportStatus():void {if(this.importingTrace)return;this.importStatus='';this.importButton.focus();}
  async cancelImport():Promise<void> {if(this.importOperation){this.importStatus='Canceling import…';try{await invoke('cancel_trace_import',{operationId:this.importOperation});}catch(error:unknown){this.importStatus='Cancel could not be requested: '+describeError(error);}}}
  async showTimings():Promise<void> {
    this.trafficMenu.hidePopover();const id=this.trafficSelection.ids.size===1?[...this.trafficSelection.ids][0]:this.selectedDetail?.id;if(!id)return;
    this.timingId=id;this.timingWaterfall=[];this.timingMinor=[];this.timingWork=[];this.timingReportText='';this.timingReportVisible=false;this.timingStatus='';this.timingPhases=[];this.timingRows=[];this.timingTransports=[];this.timingSaved=[];this.timingSummary='Loading measurements…';this.timingDialog.showModal();await this.refreshTimings();
  }
  async refreshTimings():Promise<void> {
    const id=this.timingId,generation=++this.timingGeneration;this.timingBusy=true;this.timingError='';
    try {const detail=await invoke<SessionDetail>('session_detail',{id});if(!this.isConnected||generation!==this.timingGeneration)return;const view=timingView(detail);this.timingTitle='Timings and transport · '+(detail.requests[0]?.method??'Request');this.timingSummary=view.summary;this.timingPhases=view.phases;this.timingRows=view.timeline;this.timingTransports=view.transports;this.timingSaved=view.saved;this.timingWaterfall=view.waterfall;this.timingMinor=view.minor;this.timingWork=view.work;this.timingRange=view.range;this.timingReportText=view.report;this.timingReportPreview.value=view.report;}
    catch(error:unknown){if(generation===this.timingGeneration)this.timingError='Measurements unavailable: '+describeError(error);}
    finally {if(generation===this.timingGeneration)this.timingBusy=false;}
  }
  async copyTimings():Promise<void> {if(!this.timingReportText)return;try {await navigator.clipboard.writeText(this.timingReportText);this.timingStatus='Timing report copied.';}catch{this.timingReportVisible=true;this.timingStatus='Select the report text below and use Copy.';}}
  closeTimings():void {this.timingDialog.close();}
  timingsClosed():void {this.timingGeneration++;this.timingBusy=false;}
  async showTraceMetadata(id?:string):Promise<void> {
    this.metadataTraceName='Loading trace metadata…';this.metadataSummary='';this.metadataContext='';this.metadataNetworkTitle='';this.metadataNetworkContext='';this.metadataNetworkSummary='';this.metadataNotes=[];this.metadataDialog.showModal();this.metadataBusy=true;this.metadataError='';const generation=++this.metadataGeneration;
    try{const rows=await invoke<TraceMetadata[]>('trace_metadata_list');if(!this.isConnected||generation!==this.metadataGeneration)return;this.traceMetadataRows=rows.sort((a,b)=>b.importedAt-a.importedAt);this.loadTraceMetadata(id??rows[0]?.id??'');}
    catch(error:unknown){if(generation===this.metadataGeneration)this.metadataError='Trace metadata could not be loaded: '+describeError(error);}
    finally{if(generation===this.metadataGeneration)this.metadataBusy=false;}
  }
  loadTraceMetadata(id:string):void {
    this.metadataTraceId=id;const generation=this.metadataGeneration;
    const trace=this.traceMetadataRows.find(trace=>trace.id===id);this.metadataTraceName=trace?.name??'No imported traces';this.metadataSummary=trace?`${trace.sessions} ${trace.sessions===1?'entry':'entries'} · ${trace.format.toUpperCase()} · ${trace.path}`:'Import a saved capture to see its original trace metadata here.';
    this.metadataContext=trace?.context==null?'No additional trace metadata was saved.':JSON.stringify(trace.context,null,2);
    this.metadataNetworkTitle='';this.metadataNetworkContext='';this.metadataNetworkSummary='';
    if(trace?.context&&typeof trace.context==='object'&&!Array.isArray(trace.context)){
      const context=trace.context as Record<string,unknown>, network=context.networkContext;
      if(typeof network==='string')this.metadataNetworkContext=network;
      else if(network&&typeof network==='object'){
        const fields=network as Record<string,unknown>;
        if(typeof fields.output==='string')this.metadataNetworkContext=fields.output;
        if(typeof fields.command==='string')this.metadataNetworkTitle=(fields.purpose==='trace-save'?'Network configuration at save time · ':fields.purpose==='capture-start'?'Network configuration at capture start · ':'Original network configuration · ')+fields.command;
        const collected=typeof fields.collectedAt==='number'?new Date(fields.collectedAt):null;
        this.metadataNetworkSummary=[typeof fields.platform==='string'?fields.platform:'',typeof fields.collector==='string'?fields.collector:'',typeof fields.computerName==='string'?fields.computerName:'',collected&&Number.isFinite(collected.getTime())?'Collected '+collected.toISOString():''].filter(Boolean).join(' · ');
        if(Array.isArray(fields.notes))this.metadataNetworkSummary+='. '+fields.notes.filter(note=>typeof note==='string').join(' ');
      }
      if(this.metadataNetworkContext||this.metadataNetworkTitle){this.metadataNetworkTitle||='Network context';const additional={...context};delete additional.networkContext;this.metadataContext=Object.keys(additional).length?JSON.stringify(additional,null,2):'';}
    }
    this.metadataNotes=trace?.notes.map((text,index)=>({id:String(index),text}))??[];
    this.$flushUpdates();if(generation===this.metadataGeneration)this.metadataSelector.value=id;
  }
  changeMetadataTrace(event:Event):void {void this.loadTraceMetadata((event.currentTarget as HTMLSelectElement).value);}
  closeMetadata():void {this.previewReportGeneration++;this.matchGeneration++;this.metadataGeneration++;this.metadataDialog.close();}
  pendingChanged():void { this.renderSessionState(); this.renderFollowState(); }
  pageSizeChanged():void {
    this.pageLimit = Math.max(10,Math.min(200,Number(this.pageSize)||100));
    if (this.watching) { this.pageIndex = 0; void this.refreshSessions(undefined,true); }
  }
  preferencesChanged():void {
    this.listShare = this.preferences.listSplit+'%';
    this.requestShare = this.preferences.requestSplit+'%';
    this.dividerOrientation = this.preferences.layout === 'stacked' ? 'horizontal' : 'vertical';
    this.rebuildColumns();
  }
  selectedSessionIdChanged():void { this.rebuildRows(this.sessions); }
  private rebuildColumns():void {
    this.columns = displayColumns(this.preferences.columns).map((column) => ({...column,sortDirection:this.sort.column === column.id ? this.sort.direction : 'none',sortArrow:this.sort.column === column.id ? this.sort.direction === 'ascending' ? '↑' : '↓' : ''}));
    this.settingsColumns = this.preferences.columns.map((column) => ({...column,label:columnDefinitions.find((definition) => definition.id === column.id)!.label}));
    this.tableWidth = this.columns.reduce((sum,column) => sum+column.width,0)+'px';
    this.rebuildRows(this.sessions);
  }
  private rebuildRows(rows:SessionSummary[]):void {
    this.sessions = rows.map((row) => ({...row,tone:statusTone(row),selectionState:this.trafficSelection.ids.has(row.id) ? 'true' : 'false',rowLabel:row.method+' '+row.host+row.path,
      cells:this.columns.map((column) => ({id:column.id,text:column.id === 'status' && row.status === 304 ? '304' : cellText(row,column.id),title:cellText(row,column.id),pinned:column.pinned,numeric:column.numeric,offsetCss:column.offsetCss,tone:column.id === 'status' ? statusTone(row) : ''}))}));
    for(const row of rows)if(this.trafficSelection.ids.has(row.id))this.selectedTraffic.set(row.id,row);
    this.selectedTrafficCount=this.trafficSelection.ids.size;
    const outside=this.selectedTrafficCount-rows.filter(row=>this.trafficSelection.ids.has(row.id)).length;
    this.trafficSelectionText=`${this.selectedTrafficCount} selected${outside>0?` · ${outside} on other pages`:''}`;
    this.trafficCreateLabel=`Create ${this.selectedTrafficCount} auto-response${this.selectedTrafficCount===1?'':'s'}…`;
  }
  private measurePinnedColumns():void {
    if (!this.columns.length) return;
    const root = this.getRootNode() as ShadowRoot;
    let offset = 0;
    let changed = false;
    const columns = this.columns.map((column) => {
      if (!column.pinned) return column;
      const measured = root.getElementById('header-'+column.id)?.getBoundingClientRect().width ?? column.width;
      const offsetCss = Math.round(offset)+'px'; offset += measured;
      if (offsetCss === column.offsetCss) return column;
      changed = true; return {...column,offsetCss};
    });
    if (changed) { this.columns = columns; this.rebuildRows(this.sessions); }
  }
  private patchPreferences(patch:Partial<WorkspacePreferences>,committed=true):void {
    this.preferences = {...this.preferences,...patch};
    this.$emit('workspace-change',{patch,committed});
  }
  resizeList(event:CustomEvent<{value:number;committed:boolean}>):void { this.patchPreferences({listSplit:event.detail.value},event.detail.committed); }
  resizeRequest(event:CustomEvent<{value:number;committed:boolean}>):void { this.patchPreferences({requestSplit:event.detail.value},event.detail.committed); }
  resizeMessage(event:CustomEvent<{side:string;value:number;committed:boolean}>):void { this.patchPreferences(event.detail.side === 'request' ? {requestBodySplit:event.detail.value} : {responseBodySplit:event.detail.value},event.detail.committed); }
  setLayout(event:Event):void { this.patchPreferences({layout:(event.currentTarget as HTMLSelectElement).value as WorkspacePreferences['layout']}); }
  setWrap(event:Event):void { this.patchPreferences({wrapCells:(event.currentTarget as HTMLInputElement).checked}); }
  setCompact(event:Event):void { this.patchPreferences({compactRows:(event.currentTarget as HTMLInputElement).checked}); }
  setVisible(id:ColumnId,event:Event):void {
    const visible = (event.currentTarget as HTMLInputElement).checked;
    if (!visible && this.preferences.columns.filter((column) => column.visible).length === 1) { (event.currentTarget as HTMLInputElement).checked = true; return; }
    this.patchPreferences({columns:this.preferences.columns.map((column) => column.id === id ? {...column,visible} : column)});
  }
  resetLayout():void {this.$emit('reset-workspace');}
  resetColumns():void { this.patchPreferences({columns:defaultWorkspace().columns,wrapCells:false,compactRows:true}); }
  openColumnMenu(id:ColumnId,event:MouseEvent):void {
    if (performance.now() < this.suppressMenuUntil) { event.preventDefault(); return; }
    this.menuColumn = columnDefinitions.find((column) => column.id === id)!;
    const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect();
    this.menuX = Math.max(12,Math.min(window.innerWidth-300,bounds.left))+'px';
    this.menuY = Math.min(window.innerHeight-280,bounds.bottom+5)+'px';
    this.ascendingLabel = id === 'started-at' ? 'Oldest first' : id === 'duration' ? 'Shortest first' : id.includes('bytes') ? 'Smallest first' : this.menuColumn.numeric ? 'Lowest first' : 'Sort A–Z';
    this.descendingLabel = id === 'started-at' ? 'Newest first' : id === 'duration' ? 'Longest first' : id.includes('bytes') ? 'Largest first' : this.menuColumn.numeric ? 'Highest first' : 'Sort Z–A';
    this.pinLabel = this.preferences.columns.find((column) => column.id === id)!.pinned ? 'Unpin column' : 'Pin column';
    this.filterDraft = this.filters.find((filter) => filter.column === id)?.value ?? '';
    this.$flushUpdates();
    this.filterInput.value = this.filterDraft;
    this.filterOperator.value = this.menuColumn.numeric ? 'equals' : 'contains';
  }
  private closeMenu():void { if (this.columnMenu.matches(':popover-open')) this.columnMenu.hidePopover(); }
  async applySort(direction:TrafficSort['direction'],column:ColumnId = this.menuColumn.id):Promise<void> {
    this.sort = {column,direction}; this.sortLabel = columnDefinitions.find((item) => item.id === column)!.label+' · '+(direction === 'ascending' ? 'ascending' : 'descending');
    this.followLatest = false; this.pageIndex = 0; this.queryRevision++; this.rebuildColumns(); this.closeMenu(); await this.refreshSessions(undefined,true);
  }
  async clearSort():Promise<void> { this.menuColumn = columnDefinitions.find((column) => column.id === 'started-at')!; await this.applySort('descending','started-at'); this.sortLabel = 'Newest first'; }
  pinColumn():void { const id = this.menuColumn.id; this.patchPreferences({columns:this.preferences.columns.map((column) => column.id === id ? {...column,pinned:!column.pinned} : column)}); this.closeMenu(); }
  hideColumn():void { const id = this.menuColumn.id; if (this.preferences.columns.filter((column) => column.visible).length > 1) this.patchPreferences({columns:this.preferences.columns.map((column) => column.id === id ? {...column,visible:false} : column)}); this.closeMenu(); }
  moveColumn(delta:number):void {
    const visible = this.columns; const from = visible.findIndex((column) => column.id === this.menuColumn.id);
    const target = visible[from+delta]; if (target) this.reorderColumn(this.menuColumn.id,target.id); this.closeMenu();
  }
  private reorderColumn(source:ColumnId,target:ColumnId):void {
    if (source === target) return;
    const preferences = this.preferences.columns;
    const columns = [...preferences.filter((column) => column.visible && column.pinned),...preferences.filter((column) => column.visible && !column.pinned),...preferences.filter((column) => !column.visible)];
    const from = columns.findIndex((column) => column.id === source); const to = columns.findIndex((column) => column.id === target);
    const pinned = columns[to]!.pinned; const [moved] = columns.splice(from,1);
    if (moved) columns.splice(to,0,{...moved,pinned});
    this.patchPreferences({columns});
  }
  async applyFilter(event:Event):Promise<void> {
    event.preventDefault(); const value = this.filterInput.value.trim(); if (!value) return;
    const operator = this.filterOperator.value as TrafficFilter['operator'];
    const others = this.filters.filter((filter) => filter.column !== this.menuColumn.id || filter.operator !== operator);
    if (others.length >= 14) { this.diagnostic = 'Remove a filter before adding another (maximum 14).'; return; }
    this.clearTrafficSelection();
    this.filters = [...others,{column:this.menuColumn.id,operator,value,label:this.menuColumn.label+' '+operator+' '+value}];
    this.pageIndex = 0; this.queryRevision++; this.followLatest = false; this.closeMenu(); await this.refreshSessions(undefined,true);
  }
  async removeFilter(column:ColumnId,operator:string):Promise<void> { this.clearTrafficSelection();this.filters = this.filters.filter((filter) => filter.column !== column || filter.operator !== operator); this.pageIndex = 0; this.queryRevision++; await this.refreshSessions(undefined,true); }
  async clearFilters():Promise<void> {this.clearTrafficSelection();this.filters=[];this.pageIndex=0;this.queryRevision++;await this.refreshSessions(undefined,true);}
  searchChanged(event:Event):void {
    this.searchText=(event.currentTarget as HTMLInputElement).value;
    // Content decoding starts on Search/Enter, rather than on each keystroke.
  }
  setSearchOption(option:'searchCaseSensitive'|'searchIgnoreAccents'|'searchMetadata'|'searchRequestHeaders'|'searchResponseHeaders'|'searchRequestBodies'|'searchBodies'|'selectSearchMatches',event:Event):void {this[option]=(event.currentTarget as HTMLInputElement).checked;}
  setSearchMode(event:Event):void {this.searchMode=(event.currentTarget as HTMLSelectElement).value;}
  async runContentSearch(event?:Event):Promise<void> {
    event?.preventDefault();if(this.searchingTraffic)return;
    const pattern=this.searchInput.value;this.searchText=pattern;
    if(!pattern){await this.clearContentSearch();return;}
    this.searchingTraffic=true;this.searchCancelRequested=false;this.contentSearchStatus='Searching captured traffic…';
    const operationId=crypto.randomUUID();this.searchOperation=operationId;
    const onProgress=new Channel<TrafficSearchProgress>();onProgress.onmessage=progress=>{if(this.isConnected&&this.searchOperation===progress.operationId)this.contentSearchStatus=`Searching captured traffic · ${progress.completed} of ${progress.total} entries checked…`;};
    try{
      const result=await invoke<TrafficSearchResult>('search_traffic',{request:{operationId,pattern,mode:this.searchMode,caseSensitive:this.searchCaseSensitive,ignoreDiacritics:this.searchMode==='text'&&this.searchIgnoreAccents,metadata:this.searchMetadata,requestHeaders:this.searchRequestHeaders,responseHeaders:this.searchResponseHeaders,requestBodies:this.searchRequestBodies,responseBodies:this.searchBodies},onProgress});
      if(!this.isConnected||this.searchOperation!==operationId)return;
      const query={searchResultId:result.id,filters:this.filters.map(({column,operator,value})=>({column,operator,value})),sort:this.sort,offset:0,limit:this.pageLimit};
      const [page,ids]=await Promise.all([invoke<SessionPage>('query_sessions',{query}),invoke<string[]>('matching_traffic_ids',{query})]);
      if(!this.isConnected||this.searchOperation!==operationId)return;
      if(this.searchCancelRequested){this.contentSearchStatus='Search canceled. Previous results and selections are unchanged.';return;}
      this.closeMatches();this.searchResultId=result.id;this.searchMatchIds=ids;this.contentMatchCount=ids.length;this.contentSearchActive=true;this.contentSearchLabel=pattern;
      this.contentSearchStatus=ids.length+' matching '+(ids.length===1?'entry':'entries')+' in '+result.examined+' searched entries.'+(result.unavailableBodies?' '+result.unavailableBodies+' text bodies were unavailable or beyond the search limit.':'');
      this.pageIndex=0;this.followLatest=false;this.queryRevision++;this.matchRevision=this.queryRevision;
      this.totalMatched=page.totalMatched??page.sessions.length;this.pageEnd=Math.min(this.totalMatched,page.sessions.length);this.displayedMatched=this.totalMatched;this.pendingRows=[];this.updatesPending=false;this.newTrafficCount=0;
      if(this.selectSearchMatches)this.trafficSelection.ids=new Set(ids);else this.clearTrafficSelection();
      this.rebuildRows(page.sessions);this.queryLoaded=true;this.queryError='';this.renderSessionState();this.renderFollowState();this.$flushUpdates();this.measurePinnedColumns();this.$emit('traffic-refreshed');
      if(this.selectSearchMatches)this.applySearchSelection();
    }catch(error:unknown){if(this.isConnected&&this.searchOperation===operationId)this.contentSearchStatus=describeError(error).includes('canceled')?'Search canceled. Previous results and selections are unchanged.':'Search failed: '+describeError(error)+(this.contentSearchActive?' Previous results are still shown.':'');}
    finally{onProgress.onmessage=()=>{};if(this.searchOperation===operationId){this.searchOperation='';this.searchingTraffic=false;}}
  }
  async showMatches():Promise<void> {
    if(!this.contentSearchActive||!this.searchMatchIds.length)return;
    this.matchReturnFocus=(this.getRootNode() as ShadowRoot).activeElement as HTMLElement|null;
    const id=this.selectedSessionId&&this.searchMatchIds.includes(this.selectedSessionId)?this.selectedSessionId:this.searchMatchIds[0];if(!id)return;
    this.matchDialog.showModal();await this.loadMatchEntry(id);
  }
  async loadMatchEntry(id:string):Promise<void> {
    const generation=++this.matchGeneration,search=this.searchResultId;this.matchBusy=true;this.matchError='';this.matchEntryId=id;
    try {
      const [locations,detail]=await Promise.all([invoke<TrafficSearchEntry>('traffic_search_entry',{searchId:search,id}),invoke<SessionDetail>('session_detail',{id})]);
      if(generation!==this.matchGeneration||search!==this.searchResultId||!this.isConnected)return;
      this.matchLocations=locations.matches;this.matchIndex=0;this.matchMore=locations.moreMatches;this.matchEntryLabel=(detail.requests[0]?.method??'Request')+' '+(detail.requests[0]?.target??id);this.renderMatch();
    }catch(error:unknown){if(generation===this.matchGeneration){this.matchError='Matches could not be shown: '+describeError(error);this.matchLocations=[];this.renderMatch();}}finally{if(generation===this.matchGeneration)this.matchBusy=false;}
  }
  private renderMatch():void {
    const match=this.matchLocations[this.matchIndex],entry=this.searchMatchIds.indexOf(this.matchEntryId);
    this.matchHasLocation=!!match;this.matchPosition='Entry '+(entry+1)+' of '+this.searchMatchIds.length+' · '+(match?'Match '+(this.matchIndex+1)+' of '+this.matchLocations.length+(this.matchMore?'+':''):'No current occurrences');
    const boundaries:Record<string,string>={'metadata':'Entry metadata','client-request':'Client request','upstream-request':'Upstream request','upstream-response':'Upstream response','client-response':'Client response'};
    this.matchField=match?(boundaries[match.boundary]??match.boundary)+' · '+match.field:'';this.matchBefore=match?.before??'';this.matchText=match?.matched??'';this.matchAfter=match?.after??'';
    this.matchNote=match?'Original decoded text · UTF-16 offsets '+match.startUtf16+'–'+match.endUtf16+(match.shortened?' · Long match shortened for display.':'')+(this.matchMore?' · First 200 occurrences shown; refine your search for later matches.':''):'The retained evidence may have changed since the search. Search again to refresh results.';
    this.matchHasPrevious=this.matchIndex>0;this.matchHasNext=this.matchIndex+1<this.matchLocations.length;this.matchHasPreviousEntry=entry>0;this.matchHasNextEntry=entry>=0&&entry+1<this.searchMatchIds.length;
  }
  moveMatch(step:number):void {if(this.matchBusy)return;const next=this.matchIndex+step;if(next<0||next>=this.matchLocations.length)return;this.matchIndex=next;this.renderMatch();}
  async moveMatchEntry(step:number):Promise<void> {if(this.matchBusy)return;const id=this.searchMatchIds[this.searchMatchIds.indexOf(this.matchEntryId)+step];if(id)await this.loadMatchEntry(id);}
  matchKeyboard(event:KeyboardEvent):void {if(event.key==='F3'){event.preventDefault();this.moveMatch(event.shiftKey?-1:1);}}
  closeMatches(event?:Event):void {event?.preventDefault();this.matchGeneration++;this.matchBusy=false;if(this.matchDialog?.open){this.matchDialog.close();this.matchReturnFocus?.focus();}this.matchReturnFocus=null;}

  private applySearchSelection():void {
    this.trafficSelection.ids=new Set(this.searchMatchIds);this.selectedTraffic.clear();this.trafficSelection.focused=this.sessions.find(row=>this.trafficSelection.ids.has(row.id))?.id??null;this.trafficSelection.anchor=this.trafficSelection.focused;this.updateTrafficSelection();
    const first=this.sessions.find(row=>this.trafficSelection.ids.has(row.id));if(first)void this.inspectSession(first,true);
  }
  async selectAllSearchMatches():Promise<void> {this.selectSearchMatches=true;try{await this.refreshSearchMatchIds();this.applySearchSelection();}catch(error:unknown){this.contentSearchStatus='Matches could not be selected: '+describeError(error);}}
  private async refreshSearchMatchIds():Promise<void> {
    if(!this.searchResultId)return;
    const resultId=this.searchResultId, revision=this.queryRevision;
    const ids=await invoke<string[]>('matching_traffic_ids',{query:{searchResultId:resultId,filters:this.filters.map(({column,operator,value})=>({column,operator,value}))}});
    if(!this.isConnected||this.searchResultId!==resultId||this.queryRevision!==revision)return;
    this.searchMatchIds=ids;this.contentMatchCount=ids.length;this.matchRevision=revision;
  }
  async cancelContentSearch():Promise<void> {if(this.searchOperation){this.searchCancelRequested=true;this.contentSearchStatus='Canceling search…';try{await invoke('cancel_traffic_search',{operationId:this.searchOperation});}catch(error:unknown){this.contentSearchStatus='Cancel failed: '+describeError(error);}}}
  async clearContentSearch(refresh=true):Promise<void> {
    this.closeMatches();
    if(this.searchOperation){const operation=this.searchOperation;this.searchOperation='';void invoke('cancel_traffic_search',{operationId:operation}).catch(()=>{});}
    this.searchingTraffic=false;this.searchResultId=null;this.searchMatchIds=[];this.contentSearchActive=false;this.contentMatchCount=0;this.contentSearchLabel='';this.contentSearchStatus='';this.searchText='';this.searchInput.value='';this.clearTrafficSelection();this.pageIndex=0;this.queryRevision++;if(refresh)await this.refreshSessions(undefined,true);
  }
  startColumnDrag(id:ColumnId,event:PointerEvent):void {
    if (event.button !== 0) return;
    const handle = event.currentTarget as HTMLElement;
    this.columnDrag = {id,startX:event.clientX,startY:event.clientY,pointer:event.pointerId,target:null,active:false,handle};
    handle.setPointerCapture(event.pointerId);
  }
  moveColumnDrag(event:PointerEvent):void {
    const drag = this.columnDrag;
    if (!drag || drag.pointer !== event.pointerId) return;
    if (!drag.active && Math.hypot(event.clientX-drag.startX,event.clientY-drag.startY) < 5) return;
    event.preventDefault(); drag.active = true; this.draggingColumn = drag.id; this.closeMenu();
    const bounds = this.sessionScroller.getBoundingClientRect();
    if (event.clientX > bounds.right-24) this.sessionScroller.scrollBy(20,0);
    else if (event.clientX < bounds.left+24) this.sessionScroller.scrollBy(-20,0);
    const header = (this.getRootNode() as ShadowRoot).elementFromPoint(event.clientX,event.clientY)?.closest<HTMLElement>('th[data-column-id]');
    const target = header && this.trafficTable.contains(header) ? header.dataset.columnId as ColumnId : null;
    drag.target = target !== drag.id ? target : null;
    this.dropColumnId = drag.target ?? '';
    this.dropPlacement = this.columns.findIndex(column => column.id === drag.id) < this.columns.findIndex(column => column.id === target) ? 'after' : 'before';
  }
  finishColumnDrag(event:PointerEvent):void {
    const drag = this.columnDrag;
    if (!drag || drag.pointer !== event.pointerId) return;
    if (drag.active) {
      event.preventDefault(); this.suppressMenuUntil = performance.now()+250;
      if (drag.target) this.reorderColumn(drag.id,drag.target);
    }
    this.clearColumnDrag();window.clearTimeout(this.trafficUndoTimer);
  }
  cancelColumnDrag(event:PointerEvent|KeyboardEvent):void {
    if (event instanceof KeyboardEvent && event.key !== 'Escape') return;
    if (this.columnDrag?.active) { event.preventDefault(); this.suppressMenuUntil = performance.now()+250; }
    this.clearColumnDrag();
  }
  private clearColumnDrag():void {
    const drag = this.columnDrag; this.columnDrag = null;
    this.draggingColumn = ''; this.dropColumnId = ''; this.dropPlacement = '';
    if (drag?.handle.hasPointerCapture(drag.pointer)) drag.handle.releasePointerCapture(drag.pointer);
  }
  startColumnResize(id:ColumnId,event:PointerEvent):void {
    if (event.button !== 0) return; event.preventDefault(); event.stopPropagation();
    const previous = this.preferences.columns.find((column) => column.id === id)!.width;
    const width = (this.getRootNode() as ShadowRoot).getElementById('header-'+id)!.getBoundingClientRect().width;
    this.resize = {id,startX:event.clientX,width,previous,pointer:event.pointerId};
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
  }
  resizeColumn(event:PointerEvent):void { if (this.resize?.pointer === event.pointerId) this.setColumnWidth(this.resize.id,this.resize.width+event.clientX-this.resize.startX,false); }
  finishColumnResize(event:PointerEvent):void { if (this.resize?.pointer !== event.pointerId) return; const id = this.resize.id; this.resize = null; this.setColumnWidth(id,this.preferences.columns.find((column) => column.id === id)!.width,true); }
  cancelColumnResize(event:PointerEvent):void { if (this.resize?.pointer !== event.pointerId) return; const {id,previous} = this.resize; this.resize = null; this.setColumnWidth(id,previous,true); }
  resizeColumnWithKeyboard(id:ColumnId,event:KeyboardEvent):void {
    if (event.key === 'Home') { event.preventDefault(); this.fitColumn(id); }
    else if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') { event.preventDefault(); this.setColumnWidth(id,this.preferences.columns.find((column) => column.id === id)!.width+(event.key === 'ArrowRight' ? 1 : -1)*(event.shiftKey ? 40 : 10),true); }
  }
  fitColumn(id:ColumnId):void {
    const context = new OffscreenCanvas(1,1).getContext('2d');
    if (!context) return; context.font = getComputedStyle(this.trafficTable).font || '13px Segoe UI';
    const width = Math.max(context.measureText(columnDefinitions.find((column) => column.id === id)!.label).width,...this.sessions.map((row) => context.measureText(cellText(row,id)).width))+38;
    this.setColumnWidth(id,width,true);
  }
  private setColumnWidth(id:ColumnId,width:number,committed:boolean):void { this.patchPreferences({columns:this.preferences.columns.map((column) => column.id === id ? {...column,width:Math.round(Math.max(32,Math.min(1200,width)))} : column)},committed); }

  async refreshSessions(event?:Event,force=false):Promise<void> {
    event?.preventDefault(); this.forceUpdate ||= force || event !== undefined; this.queryAgain = true;
    if (this.queryPending) return this.queryPending;
    this.queryPending = (async () => { while (this.queryAgain && this.isConnected) { this.queryAgain = false; const apply = this.forceUpdate; this.forceUpdate = false; await this.querySessions(apply); } })();
    try { await this.queryPending; } finally { this.queryPending = null; }
  }
  private async querySessions(force:boolean):Promise<void> {
    const revision = this.queryRevision;
    try {
      const page = await invoke<SessionPage>('query_sessions',{query:{cursor:null,latest:false,limit:this.pageLimit,terminal:null,method:null,host:null,search:null,searchResultId:this.searchResultId,sort:this.sort,offset:this.pageIndex*this.pageLimit,filters:this.filters.map(({column,operator,value}) => ({column,operator,value}))}});
      if (!this.isConnected || revision !== this.queryRevision) return;
      this.totalMatched = page.totalMatched ?? page.sessions.length;
      this.pageEnd = Math.min(this.totalMatched,this.pageIndex*this.pageLimit+page.sessions.length);
      if(this.contentSearchActive&&(force||this.matchRevision!==revision||this.contentMatchCount!==this.totalMatched))await this.refreshSearchMatchIds();
      if(!this.isConnected||revision!==this.queryRevision)return;
      if (force || this.followLatest || !this.sessions.length) { this.pendingRows = []; this.updatesPending = false; this.newTrafficCount = 0; this.displayedMatched = this.totalMatched; this.rebuildRows(page.sessions); }
      else {
        this.pendingRows = page.sessions;
        const ids = new Set(this.sessions.map((row) => row.id)); this.newTrafficCount = Math.max(this.totalMatched-this.displayedMatched,page.sessions.filter((row) => !ids.has(row.id)).length,0);
        this.updatesPending = this.newTrafficCount > 0 || page.sessions.some((row,index) => this.sessions[index]?.id !== row.id);
        const fresh = new Map(page.sessions.map((row) => [row.id,row])); this.rebuildRows(this.sessions.map((row) => fresh.get(row.id) ?? row));
      }
      this.queryLoaded = true; this.queryError = ''; this.renderSessionState();
      if (force) this.diagnostic = this.sessionText;
      this.renderFollowState(); this.$flushUpdates(); this.measurePinnedColumns();
      this.$emit('traffic-refreshed');
    } catch (error:unknown) { this.queryError = 'Traffic query failed: '+describeError(error); this.renderSessionState(); this.diagnostic = this.sessionText; }
  }
  async watchSessions():Promise<void> {
    if (this.watching) return; this.watching = true;
    const onEvent = new Channel<SessionHint>(); onEvent.onmessage = (hint) => {
      if (!this.isConnected) return;
      void this.refreshSessions();
      if (this.selectedSessionId && (hint.exchangeId === this.selectedSessionId || hint.lagged)) void this.refreshInspection();
    }; this.sessionUpdates = onEvent;
    try { await invoke<void>('watch_sessions',{onEvent}); this.watchError = ''; await this.refreshSessions(undefined,true); this.renderSessionState(); }
    catch (error:unknown) { this.watching = false; this.watchError = 'Live refresh failed: '+describeError(error); this.renderSessionState(); }
  }
  sessionScrolled():void { if (this.followLatest && this.sessionScroller.scrollTop > 8) { this.followLatest = false; this.renderFollowState(); } }
  private updateTrafficSelection():void {
    for(const row of this.sessions)if(this.trafficSelection.ids.has(row.id))this.selectedTraffic.set(row.id,row);
    for(const id of this.selectedTraffic.keys())if(!this.trafficSelection.ids.has(id))this.selectedTraffic.delete(id);
    this.selectedTrafficCount=this.trafficSelection.ids.size;
    const outside=this.selectedTrafficCount-this.sessions.filter(row=>this.trafficSelection.ids.has(row.id)).length;
    this.trafficSelectionText=`${this.selectedTrafficCount} selected${outside>0?` · ${outside} on other pages`:''}`;
    this.trafficCreateLabel=`Create ${this.selectedTrafficCount} auto-response${this.selectedTrafficCount===1?'':'s'}…`;
    this.rebuildRows(this.sessions);
  }
  clearTrafficSelection():void {this.trafficSelection.clear();this.selectedTraffic.clear();this.updateTrafficSelection();this.trafficMenu.hidePopover();}
  selectTraffic(session:SessionSummary,event:MouseEvent):void {
    this.trafficSelection.choose(session.id,this.sessions.map(row=>row.id),event);this.updateTrafficSelection();
    void this.inspectSession(session,true);
  }
  selectWithButton(session:SessionSummary,event:MouseEvent):void {event.stopPropagation();this.selectTraffic(session,event);}
  selectWithKeyboard(session:SessionSummary,event:KeyboardEvent):void {
    if(isTextEditing(event))return;
    if(this.contentSearchActive&&(event.ctrlKey||event.metaKey)&&event.key.toLowerCase()==='a'){event.preventDefault();void this.selectAllSearchMatches();return;}
    if(event.key==='Delete') {event.preventDefault();void this.removeSelectedTraffic();return;}
    if((event.ctrlKey||event.metaKey) && event.key.toLowerCase()==='z') {event.preventDefault();void this.undoTrafficRemoval();return;}
    if(!this.trafficSelection.key(session.id,this.sessions.map(row=>row.id),event))return;
    this.updateTrafficSelection();this.$flushUpdates();
    const id=this.trafficSelection.focused;const row=this.sessions.find(row=>row.id===id);
    if(row){(this.getRootNode() as ShadowRoot).getElementById('session-'+row.id)?.focus();if(this.trafficSelection.ids.has(row.id))void this.inspectSession(row,true);}
  }
  listKeyboard(event:KeyboardEvent):void {
    if(event.defaultPrevented || isTextEditing(event))return;
    if(this.contentSearchActive&&(event.ctrlKey||event.metaKey)&&event.key.toLowerCase()==='a'){event.preventDefault();void this.selectAllSearchMatches();return;}
    if((event.ctrlKey||event.metaKey) && event.key.toLowerCase()==='z'){event.preventDefault();void this.undoTrafficRemoval();}
    else if(event.key==='Delete'){event.preventDefault();void this.removeSelectedTraffic();}
  }
  positionTrafficMenu(event:MouseEvent):void {
    const rect=(event.currentTarget as HTMLElement).getBoundingClientRect();
    this.trafficMenuX=Math.max(10,Math.min(event.type==='contextmenu'?event.clientX:rect.left,window.innerWidth-320))+'px';
    this.trafficMenuY=Math.max(50,Math.min(event.type==='contextmenu'?event.clientY:rect.bottom+4,window.innerHeight-230))+'px';this.$flushUpdates();
  }
  openTrafficMenu(session:SessionSummary,event:MouseEvent):void {
    event.preventDefault();if(!this.trafficSelection.ids.has(session.id)){this.trafficSelection.replace(session.id);this.updateTrafficSelection();void this.inspectSession(session,true);}
    this.positionTrafficMenu(event);this.trafficMenu.showPopover();
  }
  selectedResponses():void {this.trafficMenu.hidePopover();this.$emit('autoresponse-batch-request',Array.from(this.trafficSelection.ids));}
  private pushTrafficUndo(action:{ids:string[];rows:SessionSummary[];expiresAt?:number|undefined}):void {this.trafficUndo=[...this.trafficUndo.slice(-19),action];this.scheduleTrafficUndoExpiry();}
  async clearTraffic():Promise<void> {
    if(this.removingTraffic)return;this.removingTraffic=true;
    try {
      const result=await invoke<{ids:string[];bytes:number;undoable:boolean;undoSeconds:number|null}>('clear_traffic');
      const removed=new Set(result.ids);this.trafficUndo=[];this.metadataGeneration++;this.traceMetadataRows=[];this.metadataContext='';this.metadataNetworkContext='';this.metadataNetworkTitle='';this.metadataNetworkSummary='';this.metadataNotes=[];this.metadataSummary='';this.metadataTraceId='';this.metadataTraceName='';this.metadataBusy=false;
      if(result.undoable&&result.ids.length)this.pushTrafficUndo({ids:result.ids,rows:this.sessions.filter(row=>removed.has(row.id)),expiresAt:result.undoSeconds===null?undefined:Date.now()+result.undoSeconds*1000});

      this.trafficUndoText=result.ids.length?'Cleared '+result.ids.length.toLocaleString()+' entries.'+(result.undoable?(result.undoSeconds?' Undo expires in 5 minutes.':' Undo is available.'):' Undo is unavailable because 1 GB or more was cleared.'): 'Traffic is already empty.';
      this.scheduleTrafficUndoExpiry();void this.clearContentSearch(false);this.followLatest=true;this.pageIndex=0;this.renderFollowState();this.clearTrafficSelection();this.inspectionGeneration++;this.selectedSessionId=null;this.selectedDetail=null;this.selectedMethodText='—';this.selectedUrlText='Select a request';this.selectedStatusText='No response';this.$emit('selection-changed',null);
      this.queryRevision++;this.pendingRows=[];await this.refreshSessions(undefined,true);
    }catch(error:unknown){await this.showError('Traffic could not be cleared',describeError(error));}
    finally{this.removingTraffic=false;}
  }
  private scheduleTrafficUndoExpiry():void {
    window.clearTimeout(this.trafficUndoTimer);
    const expiry=Math.min(...this.trafficUndo.flatMap(action=>action.expiresAt?[action.expiresAt]:[]));
    if(Number.isFinite(expiry))this.trafficUndoTimer=window.setTimeout(()=>{this.trafficUndo=this.trafficUndo.filter(action=>!action.expiresAt||action.expiresAt>Date.now());this.trafficUndoText='Clear Undo expired; retained data was released.';this.scheduleTrafficUndoExpiry();},Math.max(0,expiry-Date.now()));
  }
  async removeUnselectedTraffic():Promise<void> {
    if(this.removingTraffic)return;this.removingTraffic=true;
    try {
      const removed=await invoke<string[]>('remove_unselected_traffic_entries',{ids:[...this.trafficSelection.ids]});
      if(!removed.length){this.diagnostic='There are no unselected entries to remove.';return;}
      const removedIds=new Set(removed);
      this.pushTrafficUndo({ids:removed,rows:this.sessions.filter(row=>removedIds.has(row.id))});this.trafficUndoText=`Removed ${removed.length} ${removed.length===1?'entry':'entries'} from Traffic.`;
      if(this.selectedSessionId&&removedIds.has(this.selectedSessionId)){this.inspectionGeneration++;this.selectedSessionId=null;this.selectedDetail=null;this.$emit('selection-changed',null);}
      this.queryRevision++;this.pendingRows=[];await this.refreshSessions(undefined,true);this.updateTrafficSelection();this.trafficMenu.hidePopover();
    }catch(error:unknown){this.diagnostic='Traffic removal failed: '+describeError(error);}
    finally{this.removingTraffic=false;}
  }
  async removeSelectedTraffic():Promise<void> {
    if(this.removingTraffic || !this.trafficSelection.ids.size)return;this.removingTraffic=true;
    const ids=[...this.trafficSelection.ids],rows=[...this.selectedTraffic.values()];const index=this.sessions.findIndex(row=>row.id===this.trafficSelection.focused);
    try {
      const removed=await invoke<string[]>('remove_traffic_entries',{ids,restore:false});
      this.pushTrafficUndo({ids:removed,rows});
      this.trafficUndoText=`Removed ${removed.length} ${removed.length===1?'entry':'entries'} from Traffic.`;
      this.clearTrafficSelection();this.queryRevision++;this.pendingRows=[];await this.refreshSessions(undefined,true);
      const next=this.sessions[Math.max(0,Math.min(index,this.sessions.length-1))];
      if(next){this.trafficSelection.replace(next.id);this.updateTrafficSelection();await this.inspectSession(next,true);this.$flushUpdates();(this.getRootNode() as ShadowRoot).getElementById('session-'+next.id)?.focus();}
      else {this.inspectionGeneration++;this.selectedSessionId=null;this.selectedDetail=null;this.$emit('selection-changed',null);this.removingTraffic=false;this.$flushUpdates();this.trafficUndoButton.focus();}
      this.trafficMenu.hidePopover();
    }catch(error:unknown){this.diagnostic='Traffic removal failed: '+describeError(error);}
    finally{this.removingTraffic=false;}
  }
  async undoTrafficRemoval():Promise<void> {
    this.trafficUndo=this.trafficUndo.filter(action=>!action.expiresAt||action.expiresAt>Date.now());const action=this.trafficUndo.at(-1);if(!action || this.removingTraffic)return;this.removingTraffic=true;
    try {
      const restored=await invoke<string[]>('remove_traffic_entries',{ids:action.ids,restore:true});this.trafficUndo=this.trafficUndo.slice(0,-1);this.scheduleTrafficUndoExpiry();if(!this.traceMetadataRows.length)this.traceMetadataRows=await invoke<TraceMetadata[]>('trace_metadata_list').catch(()=>[]);
      this.trafficUndoText=this.trafficUndo.length?'Earlier removals can also be undone.':'';
      this.trafficSelection.ids=new Set(restored);for(const row of action.rows)if(restored.includes(row.id))this.selectedTraffic.set(row.id,row);
      this.queryRevision++;await this.refreshSessions(undefined,true);this.updateTrafficSelection();
      const id=restored[0];if(id){await this.revealSession(id);this.trafficSelection.ids=new Set(restored);for(const row of action.rows)if(restored.includes(row.id))this.selectedTraffic.set(row.id,row);this.updateTrafficSelection();}
      this.diagnostic=restored.length===action.ids.length?'Traffic entries restored.':`${restored.length} entries restored; others were evicted after removal.`;
    }catch(error:unknown){this.diagnostic='Traffic Undo failed: '+describeError(error);}
    finally{this.removingTraffic=false;}
  }
  private async inspectSession(session:SessionSummary,retainSelection=false):Promise<void> {
    if(!retainSelection){this.trafficSelection.replace(session.id);this.updateTrafficSelection();}
    const generation = ++this.inspectionGeneration;
    this.selectedSessionId = session.id; this.selectedDetail = null; this.followLatest = false; this.renderFollowState();
    this.selectedMethodText = session.method; this.selectedUrlText = session.url ?? session.host+session.path; this.selectedStatusText = session.status === 304 ? '304 Not Modified' : session.status === null ? 'Pending' : String(session.status); this.selectedTone = statusTone(session);
    this.reuseDisabled = true; this.reuseTitle = 'Loading response…'; this.matchedRuleId = ''; this.$emit('selection-changed',null);
    try {
      const detail = await loadSessionDetail(session.id,session.terminal === 'completed'); if (generation !== this.inspectionGeneration || !this.isConnected) return;
      this.applyDetail(detail);
    } catch (error:unknown) { if (generation === this.inspectionGeneration) this.diagnostic = 'Inspector unavailable: '+describeError(error); }
  }
  private applyDetail(detail:SessionDetail):void {
    this.selectedDetail = detail;
    this.previewPageAvailable=detail.storedBodies.some(body=>body.boundary==='client-response'&&body.availability==='complete'&&(body.mediaType==='text/html'||body.mediaType==='application/xhtml+xml')); const reusable = clientResponseSource(detail) !== null;
    const status = detail.responses.find((head) => head.boundary === 'client-response')?.status ?? null;
    this.selectedStatusText = status === 304 ? '304 Not Modified' : status === null ? 'Pending' : String(status);
    this.selectedTone = statusTone({status,terminal:detail.terminal} as SessionSummary);
    this.reuseDisabled = !reusable; this.reuseTitle = reusable ? 'Create a rule from this response' : autoResponseUnavailableReason(detail);
    this.matchedRuleId = detail.autoResponse?.ruleId ?? ''; this.matchedRuleLabel = detail.autoResponse ? 'Show '+detail.autoResponse.ruleName : 'Show matched rule';
    this.$emit('selection-changed',{sessionId:detail.id,detail,reusable});
  }
  private async refreshInspection():Promise<void> {
    this.inspectionAgain = true;
    if (this.liveInspection) return this.liveInspection;
    this.liveInspection = (async () => {
      while (this.inspectionAgain && this.isConnected && this.selectedSessionId) {
        this.inspectionAgain = false; const generation = this.inspectionGeneration;
        try {
          const detail = await invoke<SessionDetail>('session_detail',{id:this.selectedSessionId});
          if (generation === this.inspectionGeneration && this.isConnected) this.applyDetail(detail);
        } catch (error:unknown) { if (generation === this.inspectionGeneration) this.diagnostic = 'Inspector update unavailable: '+describeError(error); }
      }
    })();
    try { await this.liveInspection; } finally { this.liveInspection = null; }
  }
  beginAutoResponseFromSelected():void { if (this.selectedDetail && this.selectedSessionId) this.$emit('autoresponse-request',{sessionId:this.selectedSessionId,detail:this.selectedDetail}); }
  async revealSession(id:string):Promise<boolean> {
    window.clearTimeout(this.searchTimer);
    let revision = ++this.queryRevision;
    try {
      const page = await invoke<SessionPage>('query_sessions',{query:{limit:this.pageLimit,sort:this.sort,focusId:id}});
      if (revision !== this.queryRevision || !this.isConnected) return false;
      const row = page.sessions.find((session) => session.id === id);
      if (!row) throw new Error('Source no longer in Traffic.');
      // Discard live queries started with the old filters while the source page was loading.
      revision = ++this.queryRevision;
      const filtered = this.filters.length > 0 || this.searchText.length > 0;
      if(this.searchOperation){void invoke('cancel_traffic_search',{operationId:this.searchOperation}).catch(()=>{});this.searchOperation='';this.searchingTraffic=false;}
      this.searchResultId=null;this.searchMatchIds=[];this.contentSearchActive=false;this.contentSearchStatus='';this.contentSearchLabel='';this.contentMatchCount=0;
      this.filters = []; this.searchText = ''; this.searchInput.value = '';
      this.pageIndex = Math.floor((page.focusOffset ?? 0)/this.pageLimit);
      this.totalMatched = page.totalMatched; this.displayedMatched = page.totalMatched;
      this.pageEnd = this.pageIndex*this.pageLimit+page.sessions.length;
      this.followLatest = false; this.pendingRows = []; this.updatesPending = false; this.newTrafficCount = 0;
      this.rebuildRows(page.sessions); this.queryLoaded = true; this.queryError = '';
      this.renderSessionState(); this.renderFollowState(); this.$flushUpdates(); this.measurePinnedColumns();
      await this.inspectSession(row);
      if (revision !== this.queryRevision || this.selectedSessionId !== id) return true;
      const element = (this.getRootNode() as ShadowRoot).getElementById('session-'+id);
      element?.scrollIntoView({block:'nearest'}); element?.focus({preventScroll:true});
      if (filtered) this.diagnostic = 'Traffic filters cleared to reveal the source request.';
      return true;
    } catch (error:unknown) {
      if (revision !== this.queryRevision || !this.isConnected) return false;
      this.showNotice('Source unavailable',describeError(error),null,null);
      return false;
    }
  }
  showMatchedAutoResponse():void { if (this.matchedRuleId) this.$emit('matched-rule-request',this.matchedRuleId); }
  async copyUrl():Promise<void> { try { await navigator.clipboard.writeText(this.selectedUrlText); this.diagnostic = 'URL copied.'; } catch { this.diagnostic = 'Select the URL and use Copy.'; } }
  async replaySelected():Promise<void> {this.trafficMenu.hidePopover();const id=this.trafficSelection.ids.size===1?[...this.trafficSelection.ids][0]:this.selectedDetail?.id;if(!id)return;try{const detail=this.selectedDetail?.id===id?this.selectedDetail:await loadSessionDetail(id,false);this.$emit('replay-request',detail);}catch(error:unknown){this.diagnostic='Replay source unavailable: '+describeError(error);}}

  async copyRequest(format:RequestCommandFormat):Promise<void> {
    if(this.commandBusy || !this.selectedSessionId)return;
    this.trafficMenu.hidePopover();this.commandBusy=true;
    const id=this.trafficSelection.ids.size===1?[...this.trafficSelection.ids][0]!:this.selectedSessionId;this.commandSourceId=id;this.commandFormat=format;
    this.commandTitle=format==='powershell'?'Copy as PowerShell (5.1 and 7)':'Copy as cURL';this.commandText='';this.commandPreview.value='';this.commandNotices=[];this.commandFileRequired=false;this.commandStatus='Preparing command…';this.diagnostic=this.commandStatus;
    try {const result=await invoke<RequestCommand>('request_command',{id,format:this.commandFormat});if(!this.isConnected)return;this.setCommand(result);const copied=await this.copyCommand();if(result.bodyFileRequired||!copied)this.commandDialog.showModal();else this.diagnostic=this.commandTitle.replace('Copy as','').trim()+' command copied.'+(result.notices.length?' '+result.notices.join(' '):'');}
    catch(error:unknown){this.diagnostic='Could not generate command: '+describeError(error);}
    finally{this.commandBusy=false;}
  }
  private setCommand(result:RequestCommand):void {
    this.commandText=result.text;this.commandPreview.value=result.text;this.commandNotices=result.notices.map((text,index)=>({id:String(index),text}));this.commandFileRequired=result.bodyFileRequired;this.commandFileAvailable=result.bodyFileAvailable;
  }
  async copyCommand():Promise<boolean> {
    try {await navigator.clipboard.writeText(this.commandText);this.commandStatus=this.commandFileRequired?'Command copied. Supply the body file before running it.':'Command copied. Run it yourself when ready.';return true;}
    catch {this.commandStatus='Clipboard access failed. Select and copy the command below.';return false;}
  }
  async saveRequestBody():Promise<void> {
    if(this.commandBusy)return;this.commandBusy=true;this.commandStatus='Choose where to save the complete request body…';
    try {const result=await invoke<RequestCommand|null>('save_request_body',{id:this.commandSourceId,format:this.commandFormat});if(!this.isConnected)return;if(result){this.setCommand(result);const copied=await this.copyCommand();this.commandStatus=copied?'Request body saved. Updated command copied.':'Request body saved. Clipboard access failed; copy the command below.';}else this.commandStatus='Save canceled. The command still needs a body path.';}
    catch(error:unknown){this.commandStatus='Request body could not be saved: '+describeError(error);}
    finally{this.commandBusy=false;}
  }
  closeCommand():void {this.commandDialog.close();}
  async copyAllHeaders():Promise<void> {
    if(!this.selectedSessionId||this.commandBusy)return;this.trafficMenu.hidePopover();this.commandBusy=true;
    try {const id=this.trafficSelection.ids.size===1?[...this.trafficSelection.ids][0]:this.selectedSessionId;const text=await invoke<string>('copy_all_headers',{id});await navigator.clipboard.writeText(text);this.diagnostic='Request and response headers copied.';}
    catch(error:unknown){this.diagnostic='Headers could not be copied: '+describeError(error);}
    finally{this.commandBusy=false;}
  }
  async resumeLatest():Promise<void> { this.followLatest = true; this.pageIndex = 0; this.sort = {column:'started-at',direction:'descending'}; this.sortLabel = 'Newest first'; this.rebuildColumns(); this.queryRevision++; this.renderFollowState(); await this.refreshSessions(undefined,true); }
  showUpdates():void { if (this.pendingRows.length) this.rebuildRows(this.pendingRows); this.displayedMatched = this.totalMatched; this.pendingRows = []; this.updatesPending = false; this.newTrafficCount = 0; this.renderSessionState(); }
  async navigatePage(delta:number):Promise<void> { this.pageIndex = Math.max(0,this.pageIndex+delta); this.followLatest = false; this.queryRevision++; await this.refreshSessions(undefined,true); }
  private renderSessionState():void {
    const state = this.pending || this.lifecycle;
    const labels:Record<string,string> = {stopped:'Proxy stopped.',running:'Proxy running.',draining:'Finishing active requests.',starting:'Starting proxy.',stopping:'Stopping proxy.',failed:'Proxy failed.'};
    const empty = this.filters.length || this.contentSearchActive ? 'No matching exchanges.' : state === 'running' ? 'Waiting for proxied requests.' : state === 'stopped' ? 'Start the proxy to capture traffic.' : 'No exchanges captured.';
    const summary = this.queryError || this.watchError || (!this.queryLoaded ? 'Loading captured traffic…' : this.totalMatched ? 'Loaded '+this.sessions.length+' of '+this.totalMatched+' exchanges.' : empty);
    this.sessionText = this.viewerMode ? this.queryError || this.watchError || (!this.queryLoaded ? 'Loading saved traffic…' : this.totalMatched ? 'Loaded '+this.sessions.length+' of '+this.totalMatched+' saved entries.' : 'Import a SAZ or TMCap file to view saved traffic.') : (labels[state] ?? 'Proxy status unavailable.')+' '+summary;
    this.sessionKind = this.queryError || this.watchError || state === 'failed' ? 'error' : !this.queryLoaded || this.pending || state === 'stopping' ? 'progress' : state === 'running' ? 'success' : 'neutral';
  }
  private renderFollowState():void {
    const capturing = this.lifecycle === 'running' && !this.pending;
    const held = this.selectedSessionId ? 'Inspection pinned' : 'Row positions held';
    this.followText = this.viewerMode ? 'Showing saved traffic' : this.followLatest ? capturing ? 'Following live traffic' : 'Showing captured traffic' : held+' · '+(capturing ? 'capture continues' : 'showing captured traffic');
  }
  showCapturedPage():void {
    this.trafficMenu.hidePopover();const detail=this.selectedDetail;if(!detail||!this.previewPageAvailable||this.previewPageBusy)return;
    this.previewPageId=detail.id;this.previewPageUrl=detail.requests.find(head=>head.boundary==='client-request')?.target??'';this.previewPageStatus='';this.previewPageScope='original-trace';this.previewPageScripts.checked=false;this.previewPageDialog.showModal();
  }
  setPreviewScope(event:Event):void {this.previewPageScope=(event.currentTarget as HTMLSelectElement).value;}
  async showPreviewReport():Promise<void> {this.previewReportFocus=(this.getRootNode() as ShadowRoot).activeElement as HTMLElement|null;this.previewReportDialog.showModal();await this.refreshPreviewReport();}
  async refreshPreviewReport():Promise<void> {
    const label=this.previewReportLabel,generation=++this.previewReportGeneration;this.previewReportBusy=true;this.previewReportError='';
    try {const report=await invoke<CapturedPageReport>('captured_page_report',{label});if(generation!==this.previewReportGeneration||!this.isConnected)return;
      this.previewReportUrl=report.url;this.previewReportSummary=(report.scope==='all-loaded'?'All loaded traffic':report.source)+' · '+report.available+' frozen variants · '+report.skipped+' skipped · '+report.hits+' served · '+report.misses+' empty 404s'+(report.scriptsEnabled==null?'':report.scriptsEnabled?' · Scripts enabled':' · Scripts disabled');
      this.previewResourceRows=report.resources.map((row,index)=>({id:String(index),sourceId:row.entryId,sourceAvailable:row.sourceAvailable!==false,method:row.method,url:row.url,source:row.source,size:row.bytes==null?'Unavailable':row.bytes.toLocaleString()+' bytes',time:row.unixMillis==null||!Number.isFinite(new Date(row.unixMillis).getTime())?'Time unavailable':new Date(row.unixMillis).toISOString(),decision:row.decision}));
      this.previewRequestRows=report.requests.map(row=>({id:String(row.id),method:row.method,url:row.url,outcome:row.outcome==='served'?'Served':'Empty 404',reason:row.reason}));
    }catch(error:unknown){if(generation===this.previewReportGeneration)this.previewReportError='Preview diagnostics could not be read: '+describeError(error);}finally{if(generation===this.previewReportGeneration)this.previewReportBusy=false;}
  }
  async showPreviewSource(id:string):Promise<void> {try{await invoke('session_detail',{id});this.closePreviewReport();await this.revealSession(id);}catch{this.previewReportError='The source entry is no longer retained. Its resource decision remains in this report.';}}
  closePreviewReport(event?:Event):void {event?.preventDefault();this.previewReportGeneration++;this.previewReportBusy=false;this.previewReportDialog.close();this.previewReportFocus?.focus();this.previewReportFocus=null;}
  closeCapturedPageWarning():void {if(this.previewPageOperation)void invoke('cancel_captured_page',{operationId:this.previewPageOperation}).catch(()=>{});this.previewPageDialog.close();}
  async openCapturedPage():Promise<void> {
    if(this.previewPageBusy)return;this.previewPageBusy=true;this.previewPageStatus='Preparing captured responses…';
    const id=this.previewPageId,enableScripts=this.previewPageScripts.checked,operationId=crypto.randomUUID();this.previewPageOperation=operationId;
    try{const label=await invoke<string>('open_captured_page',{id,enableScripts,operationId,options:{scope:this.previewPageScope}});this.previewReportLabel=label;this.previewReportAvailable=true;if(this.isConnected){this.previewPageDialog.close();this.diagnostic='Captured page preview opened. Preview diagnostics show resource choices and missing requests.';}}
    catch(error:unknown){if(this.isConnected){this.previewPageStatus='Preview could not be opened: '+describeError(error);if(!this.previewPageDialog.open&&!describeError(error).includes('canceled'))this.showNotice('Captured page preview failed',describeError(error),null,null);}}
    finally {this.previewPageBusy=false;this.previewPageOperation='';}
  }
  @observable saveTraceFormat='native';
  changeSaveTraceFormat(event:Event):void {this.saveTraceFormat=(event.target as HTMLSelectElement).value;}
  showSaveTrace():void {if(this.savingTrace)return;this.saveTraceStatus='';this.saveTraceDialog.showModal();}
  closeSaveTrace():void {this.saveTraceDialog.close();}
  async saveTrafficTrace(event:Event):Promise<void> {
    event.preventDefault();if(this.savingTrace)return;const data=new FormData(this.saveTraceForm);this.savingTrace=true;this.saveTraceStatus='Choose a destination, then the trace will be saved…';
    try {const password=data.get('encrypt')==='on'?await this.promptTracePassword('Encrypt saved trace',true):null;if(data.get('encrypt')==='on'&&password===null){this.saveTraceStatus='Save canceled.';return;}const result=await invoke<{destination:string;entries:number;bytes:number;incompleteBodies:number}|null>('save_traffic_trace',{options:{format:String(data.get('format')??'native'),password,includeNetworkContext:data.get('networkContext')==='on',redactSensitiveHeaders:data.get('redactHeaders')==='on'}});if(!this.isConnected)return;if(result){this.saveTraceDialog.close();this.showNotice('Trace saved',result.destination+' · '+result.entries.toLocaleString()+' entries'+(result.incompleteBodies?' · '+result.incompleteBodies.toLocaleString()+' body boundaries were unavailable or incomplete.':''),null,null);}else this.saveTraceStatus='Save canceled.';}
    catch(error:unknown){if(this.isConnected){this.saveTraceStatus='Trace could not be saved: '+describeError(error);this.savingTrace=false;await this.showError('Trace save failed',describeError(error)+'\n\nChoose a writable destination and try saving again.');}}
    finally {this.savingTrace=false;}
  }
  async exportLiveCapture():Promise<void> {
    try { const result = await invoke<{destination:string;records:number;bytes:number}>('export_live_capture'); this.showNotice('TMCap export complete',result.destination,null,null); }
    catch (error:unknown) { await this.showError('TMCap export failed',describeError(error)); }
  }
  disconnectedCallback():void {
    this.clearColumnDrag();
    for(const unlisten of this.nativeUnlisteners)unlisten();this.nativeUnlisteners=[];if(this.importOperation)void invoke('cancel_trace_import',{operationId:this.importOperation}).catch(()=>{});if(this.searchOperation)void invoke('cancel_traffic_search',{operationId:this.searchOperation}).catch(()=>{});this.previewReportGeneration++;this.matchGeneration++;this.metadataGeneration++;
    window.clearTimeout(this.searchTimer); this.layoutObserver?.disconnect(); if (this.sessionUpdates) this.sessionUpdates.onmessage = () => undefined;
    if(this.previewPageOperation)void invoke('cancel_captured_page',{operationId:this.previewPageOperation}).catch(()=>{});
    this.inspectionGeneration++; super.disconnectedCallback();
  }
}
TrafficWorkspace.define('traffic-workspace');
