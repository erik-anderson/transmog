import { responseEvidence } from '../response-evidence.js';
import '../incident-intake/incident-intake.js';
import type {IncidentWindow} from '../incident-intake/incident-intake.js';
import {timingView, type TimingRow, type TimelineRow, type TransportView, type WaterfallRow} from '../timings.js';
import { attr, observable } from '@microsoft/webui-framework';
import { Channel, invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { WorkspaceElement } from '../workspace-element.js';
import type { SelectedResponse, ResponsePreviewIntent, AutomationStatus, ColumnId, Lifecycle, SessionSummary, TrafficView, SessionHint, SessionDetail, TrafficFilter, TrafficSort, WorkspacePreferences, RequestCommand, RequestCommandFormat } from '../models.js';
import { describeError, loadSessionDetail, clientResponseSource, autoResponseUnavailableReason } from '../utilities.js';
import { cellText, columnDefinitions, defaultWorkspace, displayColumns, statusText, statusTone } from '../table-model.js';
import {ListSelection,isTextEditing} from '../list-selection.js';
import {VirtualList} from '../virtual-list.js';
import initialState from '../initial-state.json';
import type {ComparisonOpen,ComparisonSubject,ScopeChoice} from '../comparison-models.js';
import type { CertificateInspector } from '../certificate-inspector/certificate-inspector.js';
import type { TraceMetadata, TraceImportResult, TraceImportProgress, TrafficSearchResult, TrafficSearchProgress, TrafficSearchEntry, TrafficSearchMatch, CapturedPageReport, PreviewRuleSnapshot } from '../models.js';

type Column = ReturnType<typeof displayColumns>[number] & {sortDirection:string;sortArrow:string};
type Row = SessionSummary & {index:number;rowIndex:number;loading:boolean;tone:string;selectionState:string;rowLabel:string;activeStreamLabel:string;cells:Array<{id:ColumnId;text:string;title:string;pinned:boolean;numeric:boolean;offsetCss:string;tone:string}>};

export class TrafficWorkspace extends WorkspaceElement {
  @observable incidentWindow:IncidentWindow|null=null;
  @observable incidentWindowLabel='';
  private incidentReturn:{scroll:number;selection:ListSelection;follow:boolean}|null=null;
  async onIncidentWindow(event:CustomEvent<{window:IncidentWindow;label:string}>):Promise<void>{event.stopPropagation();if(!this.incidentReturn)this.incidentReturn={scroll:this.sessionScroller.scrollTop,selection:this.trafficSelection.clone(),follow:this.followLatest};this.incidentWindow=event.detail.window;this.incidentWindowLabel=event.detail.label;this.followLatest=false;this.queryRevision++;await this.refreshSessions(undefined,true);if(this.metadataDialog.open)this.closeMetadata();if(this.timingDialog.open)this.closeTimings();this.trafficTable.focus({preventScroll:true});}
  async clearIncidentWindow(all=false):Promise<void>{const previous=this.incidentReturn;this.incidentReturn=null;this.incidentWindow=null;this.incidentWindowLabel='';if(all){this.filters=[];this.fetchDestinations=[];await this.clearContentSearch(false);}this.queryRevision++;this.followLatest=previous?.follow??false;await this.refreshSessions(undefined,true);if(previous&&!all){this.trafficSelection=previous.selection;for(const id of this.trafficSelection.ids)if(!this.virtualList.indexes.has(id))this.trafficSelection.ids.delete(id);this.updateTrafficSelection();this.sessionScroller.scrollTop=previous.scroll;this.scheduleViewport();}this.trafficTable.focus({preventScroll:true});}
  @observable exportReviewId='';
  @observable exportReviewBusy=false;
  @observable exportReviewScope='all';
  @observable exportReviewSummary='';
  @observable exportReviewNote='';
  @observable exportReviewFacts:Array<{label:string;value:string}>=[];
  private exportReviewRevision=0;
  async changeExportScope(event:Event):Promise<void>{this.exportReviewScope=(event.currentTarget as HTMLSelectElement).value;await this.reviewTraceExport();}
  async reviewTraceExport():Promise<void>{const revision=++this.exportReviewRevision;this.exportReviewBusy=true;this.exportReviewId='';this.exportReviewSummary='Reviewing retained evidence…';
   try{let selection:unknown=null;if(this.exportReviewScope==='selected')selection={kind:'selected',ids:[...this.trafficSelection.ids]};else if(this.exportReviewScope==='filtered')selection={kind:'filtered',query:{incidentWindow:this.incidentWindow,search:this.liveFilter,searchResultId:this.searchResultId,sort:this.sort,fetchDestinations:this.fetchDestinations,filters:this.filters.map(({column,operator,value})=>({column,operator,value}))}};else if(this.exportReviewScope.startsWith('source:'))selection={kind:'source',traceId:this.exportReviewScope.slice(7)};
    const inventory=await invoke<{id:string;entries:number;retainedEntries:number;sources:string[];sourceCount:number;hosts:string[];firstMillis:number|null;lastMillis:number|null;incompleteBodies:number;retainedBodyBytes:number;streamRecords:number;retainedStreamBytes:number;incidentMarkers:number;note:string}>('review_traffic_export',{selection});if(revision!==this.exportReviewRevision||!this.isConnected)return;
    this.exportReviewId=inventory.id;this.exportReviewSummary=`${inventory.entries} of ${inventory.retainedEntries} retained entries · ${inventory.sourceCount} original sources · ${inventory.incidentMarkers} incident markers`;
    const date=(value:number|null)=>value==null?'Unavailable':Number.isFinite(new Date(value).valueOf())?new Date(value).toISOString():'Outside supported calendar range';this.exportReviewFacts=[{label:'Included sources (up to 100)',value:inventory.sources.join(' · ')||'Live traffic'},{label:'Included hosts (up to 100)',value:inventory.hosts.join(' · ')||'No known hosts'},{label:'Recorded start range (UTC, source clocks)',value:date(inventory.firstMillis)+' to '+date(inventory.lastMillis)},{label:'Retained HTTP body bytes',value:inventory.retainedBodyBytes.toLocaleString()},{label:'Incomplete body observations',value:String(inventory.incompleteBodies)},{label:'Observed stream records / retained stream storage',value:inventory.streamRecords+' / '+inventory.retainedStreamBytes.toLocaleString()+' bytes'}];this.exportReviewNote=inventory.note;
   }catch(error:unknown){if(revision===this.exportReviewRevision)this.exportReviewSummary='Export review failed: '+describeError(error);}finally{if(revision===this.exportReviewRevision)this.exportReviewBusy=false;}}
  private emitComparison(detail:ComparisonOpen):void{this.$emit('comparison-request',detail);}
  @observable comparisonBaseline='';
  @observable comparisonBaselineLabel='';
  setComparisonBaseline():void{const id=this.trafficSelection.ids.size===1?[...this.trafficSelection.ids][0]:this.selectedSessionId;if(!id)return;this.comparisonBaseline=id;this.comparisonBaselineLabel=this.selectedDetail?.id===id?`#${this.selectedDetail.requestNumber} · ${this.selectedMethodText} ${this.selectedUrlText}`:'Selected request';this.trafficMenu.hidePopover();}
  clearComparisonBaseline():void{this.comparisonBaseline='';this.comparisonBaselineLabel='';}
  compareSelected():void{const ids=[...this.trafficSelection.ids];if(ids.length!==2){this.diagnostic='Select exactly two requests, including requests outside the visible table window.';return;}this.trafficMenu.hidePopover();this.emitComparison({left:{kind:'exchange',id:ids[0]!},right:{kind:'exchange',id:ids[1]!}});}
  compareWithBaseline():void{const id=this.trafficSelection.ids.size===1?[...this.trafficSelection.ids][0]:this.selectedSessionId;if(!id||!this.comparisonBaseline)return;this.trafficMenu.hidePopover();this.emitComparison({left:{kind:'exchange',id:this.comparisonBaseline},right:{kind:'exchange',id}});}
  async compareCaptureScopes():Promise<void>{this.filesMenu.hidePopover();const sources=await invoke<TraceMetadata[]>('trace_metadata_list').catch(()=>[]);const choices:ScopeChoice[]=sources.map(s=>({value:s.id,label:`${s.name} · ${s.sessions} imported entries`,scope:{kind:'source',traceId:s.id}}));choices.push({value:'live',label:'Live traffic · retained entries',scope:{kind:'source',traceId:null}});if(this.trafficSelection.ids.size)choices.push({value:'selected',label:`Selected requests · ${this.trafficSelection.ids.size} entries`,scope:{kind:'selected',ids:[...this.trafficSelection.ids]}});choices.push({value:'filtered',label:`Current filter · ${this.totalMatched} entries before virtual windowing`,scope:{kind:'filtered',query:{incidentWindow:this.incidentWindow,search:this.liveFilter,searchResultId:this.searchResultId,sort:this.sort,fetchDestinations:this.fetchDestinations,filters:this.filters.map(({column,operator,value})=>({column,operator,value}))}}});this.$emit('capture-comparison-request',{choices});}
  @observable previewComparisonBaselineLabel='';
  private previewComparisonBaseline:ComparisonSubject|null=null;
  setPreviewResponseBaseline(observation:string):void{this.previewComparisonBaseline={kind:'preview',run:this.previewReportLabel,observation:Number(observation)};this.previewComparisonBaselineLabel=`${this.previewReportLabel} · response ${observation}`;}
  clearPreviewResponseBaseline():void{this.previewComparisonBaseline=null;this.previewComparisonBaselineLabel='';}
  comparePreviewResponse(observation:string,sourceId:string,ruleId:string):void{
    if(!this.previewReportLabel)return;
    const asset=this.previewRuleRows.find(rule=>rule.id===ruleId)?.asset;
    const left:ComparisonSubject|null=this.previewComparisonBaseline??(sourceId?{kind:'exchange',id:sourceId}:asset?{kind:'asset',reference:asset}:null);
    if(!left){this.previewReportError='Choose a captured source or set a response baseline from another retained preview.';return;}
    this.emitComparison({left,right:{kind:'preview',run:this.previewReportLabel,observation:Number(observation)}});
  }

  certificateInspector!:CertificateInspector;
  detailsButton!:HTMLButtonElement;
  actionsButton!:HTMLButtonElement;
  requestActionsMenu!:HTMLElement;
  private closePopover(menu:HTMLElement,trigger:HTMLElement):void {
    const active=(this.getRootNode() as ShadowRoot).activeElement;
    const restore=!!active&&menu.contains(active);
    menu.hidePopover();
    if(restore)trigger.focus({preventScroll:true});
  }
  closeRequestActions(event:Event):void {if((event.target as Element).closest('button'))this.closePopover(this.requestActionsMenu,this.actionsButton);}
  @observable detailsTimingsSelected='true';
  @observable detailsConnectionSelected='false';
  @observable detailsCertificatesSelected='false';
  @observable detailsSourceSelected='false';
  @observable detailsTab='timings';
  @observable detailsTimingsTabIndex=0;
  @observable detailsConnectionTabIndex=-1;
  @observable detailsCertificatesTabIndex=-1;
  @observable detailsSourceTabIndex=-1;
  @observable hasEnabledResponses=false;
  autoresponseStateChanged():void {this.hasEnabledResponses=!!this.autoresponseState?.rules.some(rule=>rule.enabled!==false&&rule.request.responseAsset!==null);}
  @observable detailsUrl='';
  @observable detailsHasSource=false;
  @observable emptyTrafficText='Start the proxy to capture traffic.';
  private detailsCertificateLoaded=false;
  private detailsSourceId='';
  private detailsReturnFocus:HTMLElement|null=null;
  showDetailsTab(tab:string):void {
    this.detailsTimingsSelected=String(tab==='timings');
    this.detailsConnectionSelected=String(tab==='connection');
    this.detailsCertificatesSelected=String(tab==='certificates');
    this.detailsSourceSelected=String(tab==='source');
    this.detailsTab=tab;
    this.detailsTimingsTabIndex=tab==='timings'?0:-1;this.detailsConnectionTabIndex=tab==='connection'?0:-1;this.detailsCertificatesTabIndex=tab==='certificates'?0:-1;this.detailsSourceTabIndex=tab==='source'?0:-1;
    if((tab==='connection'||tab==='certificates')&&!this.detailsCertificateLoaded){
      this.detailsCertificateLoaded=true;void this.certificateInspector.inspect(this.timingId);
    }
  }
  detailsTabKeydown(event:KeyboardEvent):void {
    if(!['ArrowLeft','ArrowRight','Home','End'].includes(event.key))return;
    const tabs=['timings','connection','certificates',...(this.detailsHasSource?['source']:[])];
    const index=tabs.indexOf(this.detailsTab),next=event.key==='Home'?0:event.key==='End'?tabs.length-1:(index+(event.key==='ArrowRight'?1:-1)+tabs.length)%tabs.length;
    event.preventDefault();this.showDetailsTab(tabs[next]!);this.$flushUpdates();
    this.timingDialog.querySelector<HTMLButtonElement>('[role="tab"][data-selected]')?.focus();
  }
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
  @observable responseEvidenceRows:TimingRow[]=[];
  timingDialog!:HTMLDialogElement;
  private timingId="";
  private timingGeneration=0;
  @observable previewDocumentChoices:Array<{id:string;url:string;label:string}>=[];
  @observable previewDocumentNote='';
  @observable previewContextBusy=false;
  private previewContextRevision=0;
  private previewSceneDocumentId='';
  previewDocumentChanged(event:Event):void {
    const id=(event.currentTarget as HTMLSelectElement).value;
    const page=this.previewDocumentChoices.find(page=>page.id===id);if(!page)return;
    this.previewPageId=page.id;this.previewPageUrl=page.url;
    this.previewReusesScene=this.previewSceneAvailable&&id===this.previewSceneDocumentId;
    this.previewSourceLabel=this.previewReusesScene?this.previewReportLabel:null;
    this.previewPageScope=this.previewReusesScene?this.previewSceneScope:'original-trace';
  }
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
  @observable previewRequestRows:Array<{id:string;method:string;url:string;outcome:string;reason:string;ruleId:string;sourceId:string;sha256:string;comparable:boolean}>=[];
  @observable previewRuleRows:PreviewRuleSnapshot[]=[];
  @observable previewSceneAvailable=false;
  @observable previewPageMode='captured';
  @observable previewRulesBusy=false;
  @observable previewRulesError='';
  @observable previewRuleSummary='';
  @observable previewRules:Array<{id:string;name:string;included:boolean;state:string}>=[];
  @observable previewDraftDirty=false;
  @observable previewDraftExcluded=false;
  private previewRulesGeneration:number|undefined;
  private requestedPreviewVariant:import('../models.js').ExperimentSelection|undefined;
  private previewVariantSelection:import('../models.js').ExperimentSelection|undefined;
  private previewSceneVariant:import('../models.js').ExperimentSelection|undefined;
  private requestedPreviewVariantLatest=false;
  @observable previewVariantLocked=false;
  @observable previewVariantText='';
  private previewChoicesGeneration=0;
  private previewPageGeneration=0;
  @observable previewReusesScene=false;
  private previewSceneUrl='';
  private previewSceneScope='original-trace';
  private previewSourceLabel:string|null=null;
  private requestedPreviewRule:string|null=null;
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
  @observable saveTraceOpen=false;
  @observable saveTraceCompleted=0;
  @observable saveTraceTotal=0;
  saveTraceDialog!:HTMLDialogElement;
  saveTraceForm!:HTMLFormElement;
  @observable importingTrace=false;
  @observable showTraceImportHelp=initialState.showTraceImportHelp;
  @observable importStatus='';
  importButton!:HTMLButtonElement;
  filesMenu!:HTMLElement;
  filesMenuButton!:HTMLButtonElement;
  @observable importPercent=0;
  @observable openedTraceName='';
  @observable traceMetadataRows:TraceMetadata[]=[];
  @observable metadataTraceOptions:Array<{id:string;label:string}>=[];
  @observable metadataTraceId='';
  @observable metadataTraceName='';
  @observable metadataSummary='';
  @observable sourceRequestIdentity='';
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
  private metadataReturnFocus:HTMLElement|null=null;
  private openedTracePath='';
  private importOperation='';
  private openedFileQueue:Array<{path:string;ask:boolean}>=[];
  private nativeUnlisteners:UnlistenFn[]=[];
  @attr view = 'traffic';
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
  private trafficSelectionRevision=0;
  private trafficUndoTimer:number|undefined;
  @observable private trafficUndo:Array<{ids:string[];expiresAt?:number|undefined}>=[];
  @observable selectedDetail:SessionDetail | null = null;
  @observable streamPanel=false;
  showStream(value:boolean):void {this.streamPanel=value;}
  @observable selectedMethodText = '—';
  @observable selectedUrlText = 'Select a request';
  @observable selectedStatusText = 'No response';
  @observable selectedTone = 'pending';
  @observable inspectorSide = 'response';
  showInspectorSide(side:string):void {this.inspectorSide=side;}
  changeInspectorSide(event:Event):void {this.showInspectorSide((event.currentTarget as HTMLSelectElement).value);}
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
  @observable rowCount=2;
  @observable topSpace='0px';
  @observable bottomSpace='0px';
  @observable activeRowId='';
  private virtualList=new VirtualList();
  private trafficView:TrafficView|null=null;
  private pendingView:TrafficView|null=null;
  private rowCache=new Map<string,SessionSummary>();
  private rowRevision=0;
  private rowRequests=new Map<string,Promise<void>>();
  private rowObserver:ResizeObserver|null=null;
  private viewportStart=0;
  private viewportEnd=0;
  private renderFrame:number|undefined;
  private refreshTimer:number|undefined;
  private rowLayout='';
  private scrollGeneration=0;
  @observable sort:TrafficSort = {column:'started-at',direction:'descending'};
  @observable sortLabel = 'Newest first';
  @observable filters:TrafficFilter[] = [];
  @observable filterCount=0;
  @observable fetchDestinations:string[]=[];
  @observable fetchDestinationSummary='All destinations';
  @observable fetchDestinationOptions=structuredClone(initialState.fetchDestinationOptions);
  @observable trafficSearchKind='filter';
  @observable searchPlaceholder='Filter URLs and processes';
  private liveFilter='';
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
  private matchCurrentId='';
  private matchPreserveSelection=false;
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
  private inspectionGeneration = 0;
  private inspectionSummaryId = '';
  private queryRevision = 0;
  private resize: {id:ColumnId;startX:number;width:number;previous:number;pointer:number} | null = null;
  private columnDrag: {id:ColumnId;startX:number;startY:number;pointer:number;target:ColumnId|null;active:boolean;handle:HTMLElement} | null = null;
  private suppressMenuUntil = 0;
  private searchTimer:number | undefined;
  private layoutObserver:ResizeObserver | null = null;
  private liveInspection:Promise<void> | null = null;
  private inspectionAgain = false;
  private queryLoaded = false;
  private queryError = '';
  private watchError = '';

  protected hydratedCallback():void {
    this.layoutObserver = new ResizeObserver(() => { this.measurePinnedColumns();this.scheduleViewport(); });
    this.layoutObserver.observe(this.sessionScroller);
    this.rowObserver=new ResizeObserver(entries=>{
      if(this.view!=='traffic'||!this.sessionScroller.clientHeight)return;
      const anchor=this.virtualList.anchor(this.sessionScroller.scrollTop);let changed=false;
      for(const entry of entries){const element=entry.target as HTMLElement;if(element.dataset.loading==='true')continue;const size=entry.borderBoxSize[0]?.blockSize??entry.target.getBoundingClientRect().height;changed=this.virtualList.uniformSize(size)||changed;}
      if(changed){this.updateSpacers();this.$flushUpdates();const top=this.virtualList.restore(anchor);if(top!==null)this.sessionScroller.scrollTop=top;this.scheduleViewport();}
    });
    void this.watchSessions();
    void this.watchOpenedTraces();
    void invoke<{windows:boolean}>('desktop_bootstrap').then(bootstrap=>{if(this.isConnected)this.windowsCommands=bootstrap.windows;}).catch(()=>{});
  }
  lifecycleChanged():void { this.renderSessionState(); this.renderFollowState(); }
  viewerModeChanged():void {this.renderSessionState();this.renderFollowState();}
  traceMetadataRowsChanged():void {this.metadataTraceOptions=this.traceMetadataRows.map((trace,index)=>({id:trace.id,label:`${index+1}. ${trace.name} · ${trace.sessions} ${trace.sessions===1?'entry':'entries'}`}));}
  sessionsChanged():void {if(this.sessions.length)this.showTraceImportHelp=false;}

  private async watchOpenedTraces():Promise<void> {
    try {
      const opened=await listen('trace-open-request',()=>{void this.takeOpenedTraces();});
      if(!this.isConnected){opened();return;}this.nativeUnlisteners.push(opened);
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
  async importPaths(paths:string[]):Promise<void> {this.openedFileQueue.push(...paths.slice(0,16).map(path=>({path,ask:false})));await this.processOpenedTraces();}
  async chooseOpenedTrace(separate:boolean):Promise<void> {
    const path=this.openedTracePath;this.openedTracePath='';this.openTraceDialog.close();
    if(separate){try{await invoke('open_trace_viewer',{paths:[path]});}catch(error:unknown){this.importStatus='Capture viewer could not be opened: '+describeError(error);await this.showError('Could not open capture viewer',describeError(error));}}
    else await this.importTrace(path);
    await this.processOpenedTraces();
  }
  cancelOpenedTrace(event?:Event):void {event?.preventDefault();this.openedTracePath='';this.openTraceDialog.close();void this.processOpenedTraces();}
  async openSavedTrace(separate:boolean):Promise<void> {
    this.closePopover(this.filesMenu,this.filesMenuButton);
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
    if(failure&&this.isConnected){await this.showError('Import failed',(path.split(/[\\/]/).pop()??path)+'\n\n'+failure+'\n\nTraffic is unchanged. Choose another capture or try importing again.');if(this.isConnected&&this.view==='traffic')this.filesMenuButton.focus();}
  }
  dismissImportStatus():void {if(this.importingTrace)return;this.importStatus='';this.filesMenuButton.focus();}
  async cancelImport():Promise<void> {if(this.importOperation){this.importStatus='Canceling import…';try{await invoke('cancel_trace_import',{operationId:this.importOperation});}catch(error:unknown){this.importStatus='Cancel could not be requested: '+describeError(error);}}}
  async showTimings():Promise<void> {
    this.trafficMenu.hidePopover();const id=this.trafficSelection.ids.size===1?[...this.trafficSelection.ids][0]:this.selectedDetail?.id;if(!id)return;
    const active=(this.getRootNode() as ShadowRoot).activeElement;
    this.detailsReturnFocus=active instanceof HTMLElement&&active.closest('.selection-actions')?this.detailsButton:this.trafficTable;
    this.showDetailsTab('timings');this.detailsUrl=this.selectedUrlText;this.detailsHasSource=false;this.detailsSourceId='';this.detailsCertificateLoaded=false;
    this.timingId=id;this.timingWaterfall=[];this.timingMinor=[];this.timingWork=[];this.timingReportText='';this.timingReportVisible=false;this.timingStatus='';this.timingPhases=[];this.timingRows=[];this.timingTransports=[];this.timingSaved=[];this.responseEvidenceRows=[];this.timingSummary='Loading measurements…';this.$flushUpdates();this.timingDialog.showModal();await this.refreshTimings();
  }
  async refreshTimings():Promise<void> {
    const id=this.timingId,generation=++this.timingGeneration;this.timingBusy=true;this.timingError='';
    try {const detail=await invoke<SessionDetail>('session_detail',{id});if(!this.isConnected||generation!==this.timingGeneration)return;this.sourceRequestIdentity='Workspace request '+detail.requestNumber+(detail.originalId?' · Original request ID: '+detail.originalId:'')+(detail.originalRequestNumber?' · Original number: '+detail.originalRequestNumber+' ('+detail.originalRequestNumberProvenance+')':'');const source=detail.traceId??'';this.detailsUrl=detail.requests[0]?.target??this.detailsUrl;if(source!==this.detailsSourceId){this.detailsSourceId=source;this.detailsHasSource=!!source;if(source)void this.loadDetailsSource(source);}const view=timingView(detail);this.timingTitle='Timings and transport · '+(detail.requests[0]?.method??'Request');this.timingSummary=view.summary;this.timingPhases=view.phases;this.timingRows=view.timeline;this.timingTransports=view.transports;this.timingSaved=view.saved;this.responseEvidenceRows=responseEvidence(detail);this.timingWaterfall=view.waterfall;this.timingMinor=view.minor;this.timingWork=view.work;this.timingRange=view.range;this.timingReportText=view.report;this.timingReportPreview.value=view.report;}
    catch(error:unknown){if(generation===this.timingGeneration)this.timingError='Measurements unavailable: '+describeError(error);}
    finally {if(generation===this.timingGeneration)this.timingBusy=false;}
  }
  async copyTimings():Promise<void> {if(!this.timingReportText)return;try {await navigator.clipboard.writeText(this.timingReportText);this.timingStatus='Timing report copied.';}catch{this.timingReportVisible=true;this.timingStatus='Select the report text below and use Copy.';}}
  closeTimings():void {this.timingDialog.close();}
  timingsClosed():void {if(this.timingDialog.open)return;this.timingGeneration++;this.timingBusy=false;this.metadataGeneration++;this.certificateInspector.cancelInspection();this.detailsReturnFocus?.focus({preventScroll:true});this.detailsReturnFocus=null;}
  private async loadDetailsSource(id:string):Promise<void> {
    const generation=++this.metadataGeneration;this.metadataTraceName='Loading capture source…';this.metadataSummary='';this.metadataContext='';this.metadataNetworkTitle='';this.metadataNetworkContext='';this.metadataNetworkSummary='';this.metadataNotes=[];this.metadataError='';
    try{const rows=await invoke<TraceMetadata[]>('trace_metadata_list');if(!this.isConnected||generation!==this.metadataGeneration)return;this.traceMetadataRows=rows;this.loadTraceMetadata(id);}
    catch(error:unknown){if(generation===this.metadataGeneration)this.metadataError='Capture source unavailable: '+describeError(error);}
  }
  async showTraceMetadata(id?:string):Promise<void> {
    this.sourceRequestIdentity='';
    const active=(this.getRootNode() as ShadowRoot).activeElement;
    this.metadataReturnFocus=active instanceof HTMLElement?(this.filesMenu.contains(active)?this.filesMenuButton:active):null;
    this.closePopover(this.filesMenu,this.filesMenuButton);
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
  closeMetadata():void {this.metadataDialog.close();}
  metadataClosed():void {if(this.metadataDialog.open)return;this.metadataGeneration++;this.metadataBusy=false;const focus=this.metadataReturnFocus?.isConnected&&this.metadataReturnFocus.getClientRects().length?this.metadataReturnFocus:this.filesMenuButton;focus.focus({preventScroll:true});this.metadataReturnFocus=null;}
  pendingChanged():void { this.renderSessionState(); this.renderFollowState(); }
  viewChanged():void {if(this.view==='traffic')this.scheduleViewport();}
  preferencesChanged():void {
    this.listShare = this.preferences.listSplit+'%';
    this.requestShare = this.preferences.requestSplit+'%';
    this.dividerOrientation = this.preferences.layout === 'stacked' ? 'horizontal' : 'vertical';
    this.rebuildColumns();
    this.scheduleViewport();
  }
  selectedSessionIdChanged():void { this.rebuildRows(this.sessions); }
  private requestNumberDigits=1;
  private rebuildColumns():void {
    this.columns = displayColumns(this.preferences.columns.map(column=>column.id==='request-number'&&column.width===72?{...column,width:Math.max(column.width,this.requestNumberDigits*8+24)}:column)).map((column) => ({...column,sortDirection:this.sort.column === column.id ? this.sort.direction : 'none',sortArrow:this.sort.column === column.id ? this.sort.direction === 'ascending' ? '↑' : '↓' : ''}));
    this.settingsColumns = this.preferences.columns.map((column) => ({...column,label:column.id==='request-number'?'Request order (#)':columnDefinitions.find((definition) => definition.id === column.id)!.label}));
    this.tableWidth = this.columns.reduce((sum,column) => sum+column.width,0)+'px';
    this.rebuildRows(this.sessions);
  }
  private rebuildRows(rows:SessionSummary[]):void {
    this.sessions = rows.map((row) => {
      const activeStreamLabel = row.terminal === 'active'
        ? row.streamProtocol === 'web-socket' ? 'WebSocket · Active' : row.streamProtocol === 'sse' ? 'SSE · Active' : ''
        : '';
      return {...row,activeStreamLabel,index:this.virtualList.indexes.get(row.id)??0,rowIndex:(this.virtualList.indexes.get(row.id)??0)+2,loading:!this.rowCache.has(row.id),tone:statusTone(row),selectionState:this.trafficSelection.ids.has(row.id) ? 'true' : 'false',rowLabel:this.rowCache.has(row.id)?(activeStreamLabel ? activeStreamLabel+': ' : '')+(row.topLevelNavigation ? 'Top-level navigation: ' : '')+row.method+' '+row.host+row.path:'Loading traffic entry',
      cells:this.columns.map((column) => ({id:column.id,text:column.id === 'status' && row.status === 304 ? '304' : cellText(row,column.id),title:cellText(row,column.id)+(column.id==='protocol'&&activeStreamLabel?' · '+activeStreamLabel:''),pinned:column.pinned,numeric:column.numeric,offsetCss:column.offsetCss,tone:column.id === 'status' ? statusTone(row) : ''}))};
    });
    this.selectedTrafficCount=this.trafficSelection.ids.size;
    this.trafficSelectionText=`${this.selectedTrafficCount.toLocaleString()} selected`;
    this.trafficCreateLabel=`Create ${this.selectedTrafficCount} auto-response${this.selectedTrafficCount===1?'':'s'}…`;
    this.activeRowId=this.sessions.some(row=>row.id===this.trafficSelection.focused)?'session-'+this.trafficSelection.focused:'';
  }
  private releaseView(view:TrafficView|null):void {if(view)void invoke('release_traffic_view',{viewId:view.id}).catch(()=>{});}
  private cacheRows(rows:SessionSummary[]):void {
    const digits=Math.max(this.requestNumberDigits,...rows.map(row=>row.requestNumber?.length??1));
    if(digits>this.requestNumberDigits){this.requestNumberDigits=digits;this.rebuildColumns();}
    for(const row of rows){this.rowCache.delete(row.id);this.rowCache.set(row.id,row);}
    // Only metadata near the viewport is retained by the renderer. Selection keeps IDs.
    while(this.rowCache.size>1024){const first=this.rowCache.keys().next().value;if(first===undefined)break;this.rowCache.delete(first);}
  }
  private updateSpacers():void {
    this.topSpace=this.virtualList.offset(this.viewportStart)+'px';
    this.bottomSpace=Math.max(0,this.virtualList.total-this.virtualList.offset(this.viewportEnd))+'px';
  }
  private scheduleViewport():void {
    if(!this.sessionScroller||this.renderFrame!==undefined)return;
    this.renderFrame=requestAnimationFrame(()=>{this.renderFrame=undefined;if(this.isConnected)this.renderViewport();});
  }
  private renderViewport():void {
    if(!this.sessionScroller?.clientHeight||this.view!=='traffic')return;
    const font=parseFloat(getComputedStyle(this.sessionScroller).fontSize)||13;
    const layout=[this.preferences.compactRows,font,this.sessionScroller.clientWidth,...this.columns.map(column=>column.width)].join(':');
    if(layout!==this.rowLayout){
      const anchor=this.virtualList.anchor(this.sessionScroller.scrollTop);this.rowLayout=layout;
      const rootFont=parseFloat(getComputedStyle(document.documentElement).fontSize)||16;
      this.virtualList.reset(this.virtualList.ids,rootFont*(this.preferences.compactRows?2.05:2.6),true);
      const restored=this.virtualList.restore(anchor);if(restored!==null)this.sessionScroller.scrollTop=restored;
    }
    const header=this.trafficTable.tHead?.getBoundingClientRect().height??35;
    const range=this.virtualList.range(this.sessionScroller.scrollTop,Math.max(1,this.sessionScroller.clientHeight-header));
    const root=this.getRootNode() as ShadowRoot,active=root.activeElement;
    if(active instanceof HTMLElement&&active.closest('tr[data-session-id]')){
      const index=this.virtualList.indexes.get(active.closest<HTMLElement>('tr[data-session-id]')!.dataset.sessionId??'');
      if(index===undefined||index<range.start||index>=range.end)this.trafficTable.focus({preventScroll:true});
    }
    this.viewportStart=range.start;this.viewportEnd=range.end;
    const ids=this.virtualList.ids.slice(range.start,range.end);
    const rows=ids.map(id=>this.rowCache.get(id)??{id,caller:{kind:'local-unknown',processName:null,processId:null},method:'…',host:'',path:'',url:'',startedAt:0,contentType:null,topLevelNavigation:false,fetchDestination:null,protocol:'',status:null,durationMs:null,requestBytes:0,responseBytes:0,terminal:'active',loss:false,capturing:false,autoResponse:null} as SessionSummary);
    this.rebuildRows(rows);this.updateSpacers();this.$flushUpdates();
    this.rowObserver?.disconnect();
    for(const id of ids){const element=root.getElementById('session-'+id);if(element)this.rowObserver?.observe(element);}
    for(let offset=Math.floor(range.start/100)*100;offset<range.end;offset+=100){
      if(this.virtualList.ids.slice(offset,offset+100).some(id=>!this.rowCache.has(id)))void this.loadWindow(offset).catch(error=>{this.queryError='Traffic rows could not be loaded: '+describeError(error);this.renderSessionState();});
    }
  }
  private async loadWindow(offset:number,force=false):Promise<void> {
    const view=this.trafficView;if(!view)return;
    const revision=this.rowRevision,key=view.id+':'+revision+':'+offset;const pending=this.rowRequests.get(key);if(pending)return pending;
    if(!force&&!view.ids.slice(offset,offset+100).some(id=>!this.rowCache.has(id)))return;
    const request=invoke<SessionSummary[]>('traffic_view_rows',{viewId:view.id,offset,limit:100}).then(rows=>{
      if(!this.isConnected||this.trafficView!==view||this.rowRevision!==revision)return;
      this.cacheRows(rows);this.scheduleViewport();
      if(rows.length<Math.min(100,view.ids.length-offset))void this.refreshSessions(undefined,true);
    }).catch((error:unknown)=>{if(this.isConnected&&this.trafficView===view&&this.rowRevision===revision)throw error;}).finally(()=>{this.rowRequests.delete(key);});
    this.rowRequests.set(key,request);
    return request;
  }
  private applyView(view:TrafficView,resetScroll=false):void {
    const anchor=resetScroll?null:this.virtualList.anchor(this.sessionScroller.scrollTop);
    this.releaseView(this.trafficView);if(this.pendingView!==view)this.releaseView(this.pendingView);
    this.trafficView=view;this.pendingView=null;this.rowRevision++;this.rowCache.clear();this.cacheRows(view.page.sessions);
    this.virtualList.reset(view.ids);this.totalMatched=view.ids.length;this.rowCount=Math.max(2,view.ids.length+1);
    this.updatesPending=false;this.newTrafficCount=0;
    for(const id of this.trafficSelection.ids)if(!this.virtualList.indexes.has(id))this.trafficSelection.ids.delete(id);
    if(this.trafficSelection.focused&&!this.virtualList.indexes.has(this.trafficSelection.focused))this.trafficSelection.focused=null;
    if(this.trafficSelection.anchor&&!this.virtualList.indexes.has(this.trafficSelection.anchor))this.trafficSelection.anchor=null;
    this.viewportStart=0;this.viewportEnd=0;this.updateSpacers();this.$flushUpdates();
    this.sessionScroller.scrollTop=resetScroll?0:this.virtualList.restore(anchor)??0;
    this.renderViewport();
  }
  private async revealCurrent(id:string,focus=false,current:()=>boolean=()=>true):Promise<SessionSummary|null> {
    const generation=++this.scrollGeneration;
    while(this.isConnected&&generation===this.scrollGeneration&&current()){
      const index=this.virtualList.indexes.get(id),view=this.trafficView,revision=this.rowRevision;if(index===undefined||!view)return null;
      const height=Math.max(1,this.sessionScroller.clientHeight-(this.trafficTable.tHead?.getBoundingClientRect().height??35));
      const top=this.virtualList.offset(index),bottom=top+this.virtualList.size(index);
      if(top<this.sessionScroller.scrollTop)this.sessionScroller.scrollTop=top;
      else if(bottom>this.sessionScroller.scrollTop+height)this.sessionScroller.scrollTop=Math.max(0,bottom-height);
      this.renderViewport();
      if(!this.rowCache.has(id))await this.loadWindow(Math.floor(index/100)*100);
      if(generation!==this.scrollGeneration||!this.isConnected||!current())return null;
      // A live refresh can replace the ordering or invalidate an in-flight row read.
      // Retry this ID in the new view without superseding a newer navigation request.
      if(view!==this.trafficView||revision!==this.rowRevision)continue;
      const row=this.rowCache.get(id);
      if(!row){await this.refreshSessions(undefined,true);if(view===this.trafficView&&revision===this.rowRevision)return null;continue;}
      this.renderViewport();
      const element=(this.getRootNode() as ShadowRoot).getElementById('session-'+id);
      element?.scrollIntoView({block:'nearest',inline:'nearest',behavior:'instant'});
      if(focus&&!this.matchDialog.open)element?.focus({preventScroll:true});
      return row;
    }
    return null;
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
    this.filterInput.setCustomValidity('');
    this.filterDraft = this.filters.find((filter) => filter.column === id)?.value ?? '';
    this.$flushUpdates();
    this.filterInput.value = this.filterDraft;
    this.filterOperator.value = this.menuColumn.numeric ? 'equals' : 'contains';
  }
  private closeMenu():void { if (this.columnMenu.matches(':popover-open')) this.columnMenu.hidePopover(); }
  async applySort(direction:TrafficSort['direction'],column:ColumnId = this.menuColumn.id):Promise<void> {
    this.sort = {column,direction}; this.sortLabel = columnDefinitions.find((item) => item.id === column)!.label+' · '+(direction === 'ascending' ? 'ascending' : 'descending');
    this.followLatest = false; this.queryRevision++; this.rebuildColumns(); this.closeMenu(); await this.refreshSessions(undefined,true);
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
  clearFilterValidity():void {this.filterInput.setCustomValidity('');}
  async applyFilter(event:Event):Promise<void> {
    event.preventDefault(); const value = this.filterInput.value.trim(); if (!value) return;
    const operator = this.filterOperator.value as TrafficFilter['operator'];
    if(this.menuColumn.id==='request-number' && (!/^[0-9]+$/.test(value) || BigInt(value)<1n || BigInt(value)>18446744073709551615n)){this.filterInput.setCustomValidity('Enter a positive decimal request number from 1 to 18446744073709551615.');this.filterInput.reportValidity();return;}
    this.filterInput.setCustomValidity('');
    const others = this.filters.filter((filter) => filter.column !== this.menuColumn.id || filter.operator !== operator);
    if (others.length >= 14) { this.diagnostic = 'Remove a filter before adding another (maximum 14).'; return; }
    this.clearTrafficSelection();
    this.filters = [...others,{column:this.menuColumn.id,operator,value,label:this.menuColumn.label+' '+operator+' '+value}];
    this.queryRevision++; this.followLatest = false; this.closeMenu(); await this.refreshSessions(undefined,true);
  }
  async removeFilter(column:ColumnId,operator:string):Promise<void> { this.clearTrafficSelection();this.filters = this.filters.filter((filter) => filter.column !== column || filter.operator !== operator); this.queryRevision++; await this.refreshSessions(undefined,true); }
  filtersChanged():void {this.filterCount=this.filters.length+(this.fetchDestinations.length?1:0);}
  fetchDestinationsChanged():void {
    this.fetchDestinationOptions=this.fetchDestinationOptions.map(option=>({...option,selected:this.fetchDestinations.includes(option.value)}));
    const selected=this.fetchDestinationOptions.filter(option=>option.selected);
    this.fetchDestinationSummary=selected.length>2?selected.length+' selected':selected.map(option=>option.label).join(', ')||'All destinations';
    this.filtersChanged();
  }
  async setFetchDestination(value:string,event:Event):Promise<void> {
    const checked=(event.currentTarget as HTMLInputElement).checked;
    this.fetchDestinations=checked?[...new Set([...this.fetchDestinations,value])]:this.fetchDestinations.filter(destination=>destination!==value);
    await this.refreshDestinationFilter();
  }
  private async refreshDestinationFilter():Promise<void> {this.clearTrafficSelection();this.queryRevision++;this.followLatest=false;await this.refreshSessions(undefined,true);}
  async clearFetchDestinations():Promise<void> {this.fetchDestinations=[];await this.refreshDestinationFilter();}
  async clearFilters():Promise<void> {this.clearTrafficSelection();this.filters=[];this.fetchDestinations=[];this.queryRevision++;await this.refreshSessions(undefined,true);}
  searchChanged(event:Event):void {
    this.searchText=(event.currentTarget as HTMLInputElement).value;
    if(this.trafficSearchKind!=='filter')return;
    this.liveFilter=this.searchText;this.queryRevision++;
    window.clearTimeout(this.searchTimer);
    this.searchTimer=window.setTimeout(()=>void this.refreshSessions(undefined,true),120);
  }
  async changeTrafficSearchKind(event:Event):Promise<void> {
    const text=this.searchInput.value,kind=(event.currentTarget as HTMLSelectElement).value;
    await this.clearContentSearch(false);
    this.trafficSearchKind=kind;
    this.searchPlaceholder=this.trafficSearchKind==='filter'?'Filter URLs and processes':'Search headers and bodies';
    this.searchText=text;this.searchInput.value=text;
    this.liveFilter=this.trafficSearchKind==='filter'?text:'';
    this.searchInput.focus();this.queryRevision++;await this.refreshSessions(undefined,true);
  }
  setSearchOption(option:'searchCaseSensitive'|'searchIgnoreAccents'|'searchMetadata'|'searchRequestHeaders'|'searchResponseHeaders'|'searchRequestBodies'|'searchBodies'|'selectSearchMatches',event:Event):void {this[option]=(event.currentTarget as HTMLInputElement).checked;}
  setSearchMode(event:Event):void {this.searchMode=(event.currentTarget as HTMLSelectElement).value;}
  async runContentSearch(event?:Event):Promise<void> {
    event?.preventDefault();if(this.trafficSearchKind==='filter'){window.clearTimeout(this.searchTimer);await this.refreshSessions(undefined,true);return;}if(this.searchingTraffic)return;
    const pattern=this.searchInput.value;this.searchText=pattern;
    if(!pattern){await this.clearContentSearch();return;}
    this.searchingTraffic=true;this.searchCancelRequested=false;this.contentSearchStatus='Searching captured traffic…';
    const operationId=crypto.randomUUID();this.searchOperation=operationId;
    const onProgress=new Channel<TrafficSearchProgress>();onProgress.onmessage=progress=>{if(this.isConnected&&this.searchOperation===progress.operationId)this.contentSearchStatus=`Searching captured traffic · ${progress.completed} of ${progress.total} entries checked…`;};
    try{
      const result=await invoke<TrafficSearchResult>('search_traffic',{request:{operationId,pattern,mode:this.searchMode,caseSensitive:this.searchCaseSensitive,ignoreDiacritics:this.searchMode==='text'&&this.searchIgnoreAccents,metadata:this.searchMetadata,requestHeaders:this.searchRequestHeaders,responseHeaders:this.searchResponseHeaders,requestBodies:this.searchRequestBodies,responseBodies:this.searchBodies},onProgress});
      if(!this.isConnected||this.searchOperation!==operationId)return;
      let view:TrafficView,revision:number;
      do {
        revision=this.queryRevision;
        view=await invoke<TrafficView>('query_traffic_view',{query:{incidentWindow:this.incidentWindow,search:this.liveFilter,searchResultId:result.id,fetchDestinations:this.fetchDestinations,filters:this.filters.map(({column,operator,value})=>({column,operator,value})),sort:this.sort}});
        if(!this.isConnected||this.searchOperation!==operationId){this.releaseView(view);return;}
        if(revision!==this.queryRevision)this.releaseView(view);
      }while(revision!==this.queryRevision);
      if(this.searchCancelRequested){this.releaseView(view);this.contentSearchStatus='Search canceled. Previous results and selections are unchanged.';return;}
      const ids=view.ids;
      this.closeMatches();this.searchResultId=result.id;this.searchMatchIds=ids;this.contentMatchCount=ids.length;this.contentSearchActive=true;this.contentSearchLabel=pattern;
      this.contentSearchStatus=ids.length+' matching '+(ids.length===1?'entry':'entries')+' in '+result.examined+' searched entries.'+(result.unavailableBodies?' '+result.unavailableBodies+' text bodies were unavailable or beyond the search limit.':'');
      this.followLatest=false;this.queryRevision++;this.matchRevision=this.queryRevision;
      if(this.selectSearchMatches)this.trafficSelection.ids=new Set(ids);else this.clearTrafficSelection();
      this.applyView(view,true);this.queryLoaded=true;this.queryError='';this.renderSessionState();this.renderFollowState();this.$flushUpdates();this.measurePinnedColumns();this.$emit('traffic-refreshed');
      if(this.selectSearchMatches)await this.applySearchSelection();
    }catch(error:unknown){if(this.isConnected&&this.searchOperation===operationId)this.contentSearchStatus=describeError(error).includes('canceled')?'Search canceled. Previous results and selections are unchanged.':'Search failed: '+describeError(error)+(this.contentSearchActive?' Previous results are still shown.':'');}
    finally{onProgress.onmessage=()=>{};if(this.searchOperation===operationId){this.searchOperation='';this.searchingTraffic=false;}}
  }
  async showMatches():Promise<void> {
    if(!this.contentSearchActive||!this.searchMatchIds.length)return;
    this.matchReturnFocus=(this.getRootNode() as ShadowRoot).activeElement as HTMLElement|null;
    this.matchCurrentId='';this.matchPreserveSelection=this.trafficSelection.ids.size>1;
    const id=this.selectedSessionId&&this.searchMatchIds.includes(this.selectedSessionId)?this.selectedSessionId:this.searchMatchIds[0];if(!id)return;
    this.matchDialog.showModal();await this.loadMatchEntry(id);
  }
  async loadMatchEntry(id:string):Promise<void> {
    const generation=++this.matchGeneration,search=this.searchResultId;this.matchBusy=true;this.matchError='';this.matchEntryId=id;
    const current=()=>generation===this.matchGeneration&&search===this.searchResultId&&this.isConnected;
    try {
      const [locations,detail]=await Promise.all([invoke<TrafficSearchEntry>('traffic_search_entry',{searchId:search,id}),invoke<SessionDetail>('session_detail',{id})]);
      if(!current())return;
      const row=await this.revealCurrent(id,false,current);
      if(!current())return;
      if(!row)throw new Error('The request is no longer in the matching traffic.');
      if(!this.matchPreserveSelection)this.trafficSelection.replace(id);
      this.trafficSelection.focused=id;this.selectedSessionId=id;this.inspectionGeneration++;this.followLatest=false;
      this.setInspectionSummary(row);
      this.applyDetail(detail);this.updateTrafficSelection();this.renderFollowState();this.matchCurrentId=id;
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
  closeMatches(event?:Event):void {
    event?.preventDefault();const generation=++this.matchGeneration;this.matchBusy=false;
    if(this.matchDialog?.open){
      const id=this.matchCurrentId,revision=this.queryRevision,returnFocus=this.matchReturnFocus??this.trafficTable;
      this.matchDialog.close();
      const current=()=>this.matchGeneration===generation&&this.queryRevision===revision&&!this.matchDialog.open&&this.selectedSessionId===id&&this.trafficSelection.focused===id;
      if(id)void this.revealCurrent(id,true,current).then(row=>{if(!row&&current())returnFocus.focus({preventScroll:true});}).catch(()=>{if(current())returnFocus.focus({preventScroll:true});});
      else returnFocus.focus({preventScroll:true});
    }
    this.matchReturnFocus=null;
  }

  private async applySearchSelection():Promise<void> {
    this.trafficSelection.ids=new Set(this.searchMatchIds);
    const first=this.virtualList.indexAt(this.sessionScroller.scrollTop),visible=this.virtualList.ids[first];
    const height=Math.max(1,this.sessionScroller.clientHeight-(this.trafficTable.tHead?.getBoundingClientRect().height??35));
    const last=this.virtualList.indexAt(this.sessionScroller.scrollTop+height),selectedIndex=this.virtualList.indexes.get(this.selectedSessionId??'');
    const keepInspector=selectedIndex!==undefined&&selectedIndex>=first&&selectedIndex<=last&&this.selectedSessionId&&this.trafficSelection.ids.has(this.selectedSessionId);
    const id=keepInspector?this.selectedSessionId:visible&&this.trafficSelection.ids.has(visible)?visible:this.searchMatchIds[0]??null;
    this.trafficSelection.focused=id;this.trafficSelection.anchor=id;this.updateTrafficSelection();
    if(!id)return;
    const search=this.searchResultId;
    await this.inspectTraffic(id,false,()=>this.searchResultId===search&&this.trafficSelection.focused===id&&this.trafficSelection.ids.has(id));
  }
  async selectAllSearchMatches():Promise<void> {this.selectSearchMatches=true;const selection=this.trafficSelectionRevision;try{if(await this.refreshSearchMatchIds()&&selection===this.trafficSelectionRevision)await this.applySearchSelection();}catch(error:unknown){this.contentSearchStatus='Matches could not be selected: '+describeError(error);}}
  private async refreshSearchMatchIds():Promise<boolean> {
    if(!this.searchResultId)return false;
    const resultId=this.searchResultId, revision=this.queryRevision;
    const ids=await invoke<string[]>('matching_traffic_ids',{query:{incidentWindow:this.incidentWindow,search:this.liveFilter,searchResultId:resultId,sort:this.sort,fetchDestinations:this.fetchDestinations,filters:this.filters.map(({column,operator,value})=>({column,operator,value}))}});
    if(!this.isConnected||this.searchResultId!==resultId||this.queryRevision!==revision)return false;
    this.searchMatchIds=ids;this.contentMatchCount=ids.length;this.matchRevision=revision;
    return true;
  }
  async cancelContentSearch():Promise<void> {if(this.searchOperation){this.searchCancelRequested=true;this.contentSearchStatus='Canceling search…';try{await invoke('cancel_traffic_search',{operationId:this.searchOperation});}catch(error:unknown){this.contentSearchStatus='Cancel failed: '+describeError(error);}}}
  async clearContentSearch(refresh=true):Promise<void> {
    this.closeMatches();
    if(this.searchOperation){const operation=this.searchOperation;this.searchOperation='';void invoke('cancel_traffic_search',{operationId:operation}).catch(()=>{});}
    this.searchingTraffic=false;this.searchResultId=null;this.searchMatchIds=[];this.contentSearchActive=false;this.contentMatchCount=0;this.contentSearchLabel='';this.contentSearchStatus='';this.searchText='';this.liveFilter='';window.clearTimeout(this.searchTimer);this.searchInput.value='';this.clearTrafficSelection();this.queryRevision++;if(refresh)await this.refreshSessions(undefined,true);
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
    else if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') { event.preventDefault(); this.setColumnWidth(id,this.columns.find((column) => column.id === id)!.width+(event.key === 'ArrowRight' ? 1 : -1)*(event.shiftKey ? 40 : 10),true); }
  }
  fitColumn(id:ColumnId):void {
    const context = new OffscreenCanvas(1,1).getContext('2d');
    if (!context) return; context.font = getComputedStyle(this.trafficTable).font || '13px Segoe UI';
    const width = Math.max(context.measureText((id==='request-number'?'#':columnDefinitions.find((column) => column.id === id)!.label)).width,...this.sessions.map((row) => context.measureText(cellText(row,id)).width+(id==='path'&&row.topLevelNavigation?42:0)+(id==='protocol'&&row.activeStreamLabel?context.measureText(row.activeStreamLabel).width+18:0)))+38;
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
      const view = await invoke<TrafficView>('query_traffic_view',{query:{incidentWindow:this.incidentWindow,search:this.liveFilter,searchResultId:this.searchResultId,sort:this.sort,fetchDestinations:this.fetchDestinations,filters:this.filters.map(({column,operator,value}) => ({column,operator,value}))}});
      if (!this.isConnected || revision !== this.queryRevision) {this.releaseView(view);return;}
      if(view.page.retainedCount>0||view.ids.length)this.showTraceImportHelp=false;
      if(this.contentSearchActive){this.searchMatchIds=view.ids;this.contentMatchCount=view.ids.length;this.matchRevision=revision;}
      const existing=this.trafficView,ids=new Set(view.ids);
      if(force||this.followLatest||!existing||existing.ids.some(id=>!ids.has(id)))this.applyView(view,this.followLatest);
      else {
        this.releaseView(this.pendingView);this.pendingView=view;
        this.newTrafficCount=view.ids.filter(id=>!this.virtualList.indexes.has(id)).length;
        this.updatesPending=this.newTrafficCount>0||view.ids.some((id,index)=>existing.ids[index]!==id);
        this.rowRevision++;this.rowCache.clear();this.cacheRows(view.page.sessions);await this.loadWindow(Math.floor(this.viewportStart/100)*100,true);
        if(!this.isConnected||revision!==this.queryRevision)return;
        this.renderViewport();
      }
      this.queryLoaded = true; this.queryError = ''; this.renderSessionState();
      if (force) this.diagnostic = this.sessionText;
      this.renderFollowState(); this.$flushUpdates(); this.measurePinnedColumns();
      this.$emit('traffic-refreshed');
    } catch (error:unknown) {if(!this.isConnected||revision!==this.queryRevision)return;this.queryError = 'Traffic query failed: '+describeError(error); this.renderSessionState(); this.diagnostic = this.sessionText; }
  }
  async watchSessions():Promise<void> {
    if (this.watching) return; this.watching = true;
    const onEvent = new Channel<SessionHint>(); onEvent.onmessage = (_hint) => {
      if (!this.isConnected) return;
      if(this.refreshTimer===undefined)this.refreshTimer=window.setTimeout(()=>{this.refreshTimer=undefined;void this.refreshSessions();if(this.selectedSessionId)void this.refreshInspection();},200);

    }; this.sessionUpdates = onEvent;
    try { await invoke<void>('watch_sessions',{onEvent}); this.watchError = ''; await this.refreshSessions(undefined,true); this.renderSessionState(); }
    catch (error:unknown) { this.watching = false; this.watchError = 'Live refresh failed: '+describeError(error); this.renderSessionState(); }
  }
  sessionScrolled():void { if (this.followLatest && this.sessionScroller.scrollTop > 8) { this.followLatest = false; this.renderFollowState(); }this.scheduleViewport(); }
  private updateTrafficSelection():void {
    this.trafficSelectionRevision++;
    this.rebuildRows(this.sessions);
  }
  clearTrafficSelection():void {this.trafficSelection.clear();this.updateTrafficSelection();this.trafficMenu.hidePopover();}
  selectTraffic(session:SessionSummary,event:MouseEvent):void {
    this.trafficSelection.choose(session.id,this.virtualList.ids,event);this.updateTrafficSelection();
    void this.inspectTraffic(session.id,true,()=>this.trafficSelection.focused===session.id);
  }
  selectWithButton(session:SessionSummary,event:MouseEvent):void {event.stopPropagation();this.selectTraffic(session,event);}
  selectWithKeyboard(session:SessionSummary,event:KeyboardEvent):void {
    this.handleTrafficKey(session.id,event);
  }
  private handleTrafficKey(id:string,event:KeyboardEvent):void {
    if(isTextEditing(event))return;
    if(event.key==='Delete') {event.preventDefault();void this.removeSelectedTraffic();return;}
    if((event.ctrlKey||event.metaKey) && event.key.toLowerCase()==='z') {event.preventDefault();void this.undoTrafficRemoval();return;}
    if(event.key==='PageUp'||event.key==='PageDown'){
      const index=this.virtualList.indexes.get(id)??0,direction=event.key==='PageDown'?1:-1;
      const height=Math.max(1,this.sessionScroller.clientHeight-(this.trafficTable.tHead?.getBoundingClientRect().height??35));
      const destination=this.virtualList.indexAt(Math.max(0,this.virtualList.offset(index)+direction*height));
      const nextIndex=direction>0?Math.max(index+1,destination):Math.min(index-1,destination);
      const next=this.virtualList.ids[Math.max(0,Math.min(this.virtualList.ids.length-1,nextIndex))];
      event.preventDefault();if(next){if((event.ctrlKey||event.metaKey)&&!event.shiftKey)this.trafficSelection.focused=next;else this.trafficSelection.choose(next,this.virtualList.ids,event);}
    }else if(!this.trafficSelection.key(id,this.virtualList.ids,event))return;
    this.updateTrafficSelection();this.$flushUpdates();
    if((event.ctrlKey||event.metaKey)&&event.key.toLowerCase()==='a')return;
    const focused=this.trafficSelection.focused;
    if(focused){
      if(this.trafficSelection.ids.has(focused))void this.inspectTraffic(focused,true,()=>this.trafficSelection.focused===focused&&this.trafficSelection.ids.has(focused));
      else void this.revealCurrent(focused,true).catch(error=>{this.diagnostic=describeError(error);});
    }
  }
  listKeyboard(event:KeyboardEvent):void {
    if(event.defaultPrevented || isTextEditing(event))return;
    const target=event.composedPath()[0];
    if(!(target instanceof HTMLElement))return;
    const row=target.closest('tr[data-session-id]');
    if(target!==this.trafficTable&&(!row||!this.trafficTable.contains(row))){
      if(this.trafficTable.contains(target))return;
      if(!['Delete','Escape'].includes(event.key)&&!((event.ctrlKey||event.metaKey)&&['a','z'].includes(event.key.toLowerCase())))return;
    }
    this.handleTrafficKey(this.trafficSelection.focused??this.virtualList.ids[0]??'',event);
  }
  positionTrafficMenu(event:MouseEvent):void {
    const rect=(event.currentTarget as HTMLElement).getBoundingClientRect();
    this.trafficMenuX=Math.max(10,Math.min(event.type==='contextmenu'?event.clientX:rect.left,window.innerWidth-320))+'px';
    this.trafficMenuY=Math.max(50,Math.min(event.type==='contextmenu'?event.clientY:rect.bottom+4,window.innerHeight-230))+'px';this.$flushUpdates();
  }
  openTrafficMenu(session:SessionSummary,event:MouseEvent):void {
    event.preventDefault();if(!this.trafficSelection.ids.has(session.id)){this.trafficSelection.replace(session.id);this.updateTrafficSelection();void this.inspectTraffic(session.id,false,()=>this.trafficSelection.focused===session.id&&this.trafficSelection.ids.has(session.id));}
    this.positionTrafficMenu(event);this.trafficMenu.showPopover();
  }
  selectedResponses():void {this.trafficMenu.hidePopover();this.$emit('autoresponse-batch-request',Array.from(this.trafficSelection.ids));}
  private pushTrafficUndo(action:{ids:string[];expiresAt?:number|undefined}):void {this.trafficUndo=[...this.trafficUndo.slice(-19),action];this.scheduleTrafficUndoExpiry();}
  async clearTraffic():Promise<void> {
    if(this.removingTraffic)return;this.removingTraffic=true;
    try {
      const result=await invoke<{ids:string[];bytes:number;undoable:boolean;undoSeconds:number|null}>('clear_traffic');
      this.trafficUndo=[];this.incidentWindow=null;this.incidentWindowLabel="";this.incidentReturn=null;this.metadataGeneration++;this.traceMetadataRows=[];this.metadataContext='';this.metadataNetworkContext='';this.metadataNetworkTitle='';this.metadataNetworkSummary='';this.metadataNotes=[];this.metadataSummary='';this.metadataTraceId='';this.metadataTraceName='';this.metadataBusy=false;
      if(result.undoable&&result.ids.length)this.pushTrafficUndo({ids:result.ids,expiresAt:result.undoSeconds===null?undefined:Date.now()+result.undoSeconds*1000});

      this.trafficUndoText=result.ids.length?'Cleared '+result.ids.length.toLocaleString()+' entries.'+(result.undoable?(result.undoSeconds?' Undo expires in 5 minutes.':' Undo is available.'):' Undo is unavailable because 1 GB or more was cleared.'): 'Traffic is already empty.';
      this.scheduleTrafficUndoExpiry();void this.clearContentSearch(false);this.followLatest=true;this.renderFollowState();this.clearTrafficSelection();this.inspectionGeneration++;this.selectedSessionId=null;this.selectedDetail=null;this.selectedMethodText='—';this.selectedUrlText='Select a request';this.selectedStatusText='No response';this.$emit('selection-changed',null);
      this.queryRevision++;await this.refreshSessions(undefined,true);
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
      this.pushTrafficUndo({ids:removed});this.trafficUndoText=`Removed ${removed.length} ${removed.length===1?'entry':'entries'} from Traffic.`;
      if(this.selectedSessionId&&removedIds.has(this.selectedSessionId)){this.inspectionGeneration++;this.selectedSessionId=null;this.selectedDetail=null;this.$emit('selection-changed',null);}
      this.queryRevision++;await this.refreshSessions(undefined,true);this.updateTrafficSelection();this.trafficMenu.hidePopover();
    }catch(error:unknown){this.diagnostic='Traffic removal failed: '+describeError(error);}
    finally{this.removingTraffic=false;}
  }
  async removeSelectedTraffic():Promise<void> {
    if(this.removingTraffic || !this.trafficSelection.ids.size)return;this.removingTraffic=true;
    const ids=[...this.trafficSelection.ids],order=this.virtualList.ids;const index=this.virtualList.indexes.get(this.trafficSelection.focused??'')??0;
    try {
      const removed=await invoke<string[]>('remove_traffic_entries',{ids,restore:false});
      this.pushTrafficUndo({ids:removed});
      this.trafficUndoText=`Removed ${removed.length} ${removed.length===1?'entry':'entries'} from Traffic.`;
      this.clearTrafficSelection();this.queryRevision++;await this.refreshSessions(undefined,true);
      const removedIds=new Set(removed);
      const next=order.slice(index).find(id=>!removedIds.has(id)&&this.virtualList.indexes.has(id))??order.slice(0,index).reverse().find(id=>!removedIds.has(id)&&this.virtualList.indexes.has(id));
      if(next){this.trafficSelection.replace(next);this.updateTrafficSelection();await this.inspectTraffic(next,true);}
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
      this.trafficSelection.ids=new Set(restored);
      this.queryRevision++;await this.refreshSessions(undefined,true);this.updateTrafficSelection();
      const id=restored.find(id=>this.virtualList.indexes.has(id));if(id){this.trafficSelection.focused=id;this.trafficSelection.anchor=id;await this.inspectTraffic(id,true);this.updateTrafficSelection();}
      this.diagnostic=restored.length===action.ids.length?'Traffic entries restored.':`${restored.length} entries restored; others were evicted after removal.`;
    }catch(error:unknown){this.diagnostic='Traffic Undo failed: '+describeError(error);}
    finally{this.removingTraffic=false;}
  }
  private async inspectSession(session:SessionSummary,retainSelection=false):Promise<void> {
    if(!retainSelection){this.trafficSelection.replace(session.id);this.updateTrafficSelection();}
    const generation=this.beginInspection(session.id,session);
    await this.loadInspection(session,generation);
  }
  private beginInspection(id:string,session?:SessionSummary):number {
    const generation=++this.inspectionGeneration;
    this.selectedSessionId=id;this.selectedDetail=null;this.followLatest=false;this.renderFollowState();
    this.setInspectionSummary(session);
    this.reuseDisabled = true; this.reuseTitle = 'Loading response…'; this.matchedRuleId = ''; this.$emit('selection-changed',null);
    return generation;
  }
  private setInspectionSummary(session?:SessionSummary):void {
    this.inspectionSummaryId=session?.id??'';
    this.selectedMethodText=session?.method??'…';this.selectedUrlText=session?.url??'Loading request…';
    this.selectedStatusText=session?statusText(session):'Loading…';
    this.selectedTone=session?statusTone(session):'';
  }
  private async inspectTraffic(id:string,focus=false,current:()=>boolean=()=>true):Promise<void> {
    const generation=this.beginInspection(id,this.rowCache.get(id));
    const valid=()=>generation===this.inspectionGeneration&&this.selectedSessionId===id&&this.isConnected&&current();
    try {
      const row=await this.revealCurrent(id,focus,valid);
      if(!row||!valid())return;
      this.setInspectionSummary(row);await this.loadInspection(row,generation);
    }catch(error:unknown){if(valid())this.diagnostic='Inspector unavailable: '+describeError(error);}
  }
  private async loadInspection(session:SessionSummary,generation:number):Promise<void> {
    try {
      const detail = await loadSessionDetail(session.id,session.terminal === 'completed'); if (generation !== this.inspectionGeneration || !this.isConnected) return;
      this.applyDetail(detail);
    } catch (error:unknown) { if (generation === this.inspectionGeneration) this.diagnostic = 'Inspector unavailable: '+describeError(error); }
  }
  private applyDetail(detail:SessionDetail):void {
    if(detail.id!==this.selectedDetail?.id || (!this.selectedDetail?.stream&&detail.stream))this.streamPanel=!!detail.stream;
    this.selectedDetail = detail;
    this.previewPageAvailable=detail.storedBodies.some(body=>body.boundary==='client-response'&&body.availability==='complete'&&(body.mediaType==='text/html'||body.mediaType==='application/xhtml+xml')); const reusable = clientResponseSource(detail) !== null;
    const status = detail.responses.find((head) => head.boundary === 'client-response')?.status ?? null;
    const responseStatus = {status,terminal:detail.terminalState};
    this.selectedStatusText = statusText(responseStatus);
    this.selectedTone = statusTone(responseStatus);
    this.reuseDisabled = !reusable; this.reuseTitle = reusable ? 'Create a rule from this response' : autoResponseUnavailableReason(detail);
    this.matchedRuleId = detail.traceId ? '' : detail.autoResponse?.ruleId ?? ''; this.matchedRuleLabel = detail.autoResponse ? 'Show '+detail.autoResponse.ruleName : 'Show matched rule';
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
  beginExperimentFromSelected():void {
    if(this.viewerMode||this.reuseDisabled||!this.selectedDetail||!this.selectedSessionId)return;
    this.$emit('response-experiment-request',{sessionId:this.selectedSessionId,detail:structuredClone(this.selectedDetail),reusable:true} satisfies SelectedResponse);
  }
  beginAutoResponseFromSelected():void { if (this.selectedDetail && this.selectedSessionId) this.$emit('autoresponse-request',{sessionId:this.selectedSessionId,detail:this.selectedDetail}); }
  async revealSession(id:string):Promise<boolean> {
    window.clearTimeout(this.searchTimer);

    let revision = ++this.queryRevision;
    try {
      const view = await invoke<TrafficView>('query_traffic_view',{query:{sort:this.sort,focusId:id}});
      if (revision !== this.queryRevision || !this.isConnected) {this.releaseView(view);return false;}
      this.incidentWindow=null;this.incidentWindowLabel="";this.incidentReturn=null;
    // Discard live queries started with the old filters while the source page was loading.
      revision = ++this.queryRevision;
      const filtered = this.filterCount > 0 || this.searchText.length > 0;
      if(this.searchOperation){void invoke('cancel_traffic_search',{operationId:this.searchOperation}).catch(()=>{});this.searchOperation='';this.searchingTraffic=false;}
      this.searchResultId=null;this.searchMatchIds=[];this.contentSearchActive=false;this.contentSearchStatus='';this.contentSearchLabel='';this.contentMatchCount=0;
      this.filters = []; this.fetchDestinations=[]; this.searchText = ''; this.liveFilter='';window.clearTimeout(this.searchTimer);this.searchInput.value = '';
      this.followLatest = false;this.closeMatches();this.applyView(view);this.queryLoaded = true; this.queryError = '';
      this.renderSessionState(); this.renderFollowState(); this.$flushUpdates(); this.measurePinnedColumns();
      const row=await this.revealCurrent(id,true);if(!row)throw new Error('Source no longer in Traffic.');
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
  async copyUrl():Promise<void> {if(!this.selectedSessionId||this.inspectionSummaryId!==this.selectedSessionId){this.diagnostic='The request URL is still loading.';return;}try { await navigator.clipboard.writeText(this.selectedUrlText); this.diagnostic = 'URL copied.'; } catch { this.diagnostic = 'Select the URL and use Copy.'; } }
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
  async copyRequestReference():Promise<void> {
    this.trafficMenu.hidePopover();
    const id=this.trafficSelection.ids.size===1?[...this.trafficSelection.ids][0]:this.selectedSessionId;if(!id)return;
    try{const detail=await invoke<SessionDetail>('session_detail',{id});
      const traces=detail.traceId?await invoke<TraceMetadata[]>('trace_metadata_list'):[];
      const trace=traces.find(trace=>trace.id===detail.traceId);
      const text='Workspace request '+detail.requestNumber+' (workspace-local)\n'+(trace?'Capture: '+trace.name+'\nPath: '+trace.path+'\n':'Live traffic\n')+'Request ID: '+(detail.originalId??detail.id)+(detail.originalRequestNumber?'\nOriginal number: '+detail.originalRequestNumber+' ('+detail.originalRequestNumberProvenance+')':'')+'\n'+(detail.requests[0]?.target??'');
      await navigator.clipboard.writeText(text);this.diagnostic='Request reference copied.';
    }catch(error:unknown){this.diagnostic='Request reference could not be copied: '+describeError(error);}
  }
  async copyAllHeaders():Promise<void> {
    if(!this.selectedSessionId||this.commandBusy)return;this.trafficMenu.hidePopover();this.commandBusy=true;
    try {const id=this.trafficSelection.ids.size===1?[...this.trafficSelection.ids][0]:this.selectedSessionId;const text=await invoke<string>('copy_all_headers',{id});await navigator.clipboard.writeText(text);this.diagnostic='Request and response headers copied.';}
    catch(error:unknown){this.diagnostic='Headers could not be copied: '+describeError(error);}
    finally{this.commandBusy=false;}
  }
  async resumeLatest():Promise<void> { this.followLatest = true; this.sort = {column:'started-at',direction:'descending'}; this.sortLabel = 'Newest first'; this.rebuildColumns(); this.queryRevision++; this.renderFollowState(); await this.refreshSessions(undefined,true); }
  showUpdates():void {if(this.pendingView)this.applyView(this.pendingView);this.renderSessionState();}
  private renderSessionState():void {
    const state = this.pending || this.lifecycle;
    const labels:Record<string,string> = {stopped:'Proxy stopped.',running:'Proxy running.',draining:'Finishing active requests.',starting:'Starting proxy.',stopping:'Stopping proxy.',failed:'Proxy failed.'};
    const empty = this.filterCount || this.liveFilter || this.contentSearchActive ? 'No matching traffic. Adjust or clear your filters.' : this.viewerMode ? 'Import a saved capture to view traffic.' : state === 'running' ? 'Waiting for traffic. Make a request through the proxy.' : state === 'stopped' ? 'Start the proxy to capture traffic.' : 'No exchanges captured.';
    this.emptyTrafficText=empty;
    const summary = this.queryError || this.watchError || (!this.queryLoaded ? 'Loading captured traffic…' : this.totalMatched ? this.totalMatched.toLocaleString()+' traffic entries.' : empty);
    this.sessionText = this.viewerMode ? this.queryError || this.watchError || (!this.queryLoaded ? 'Loading saved traffic…' : this.totalMatched ? this.totalMatched.toLocaleString()+' saved entries.' : empty) : (labels[state] ?? 'Proxy status unavailable.')+' '+summary;
    this.sessionKind = this.queryError || this.watchError || state === 'failed' ? 'error' : !this.queryLoaded || this.pending || state === 'stopping' ? 'progress' : state === 'running' ? 'success' : 'neutral';
  }
  private renderFollowState():void {
    const capturing = this.lifecycle === 'running' && !this.pending;
    const held = 'Auto-scroll paused';
    this.followText = this.viewerMode ? 'Showing saved traffic' : this.followLatest ? capturing ? 'Following live traffic' : 'Showing captured traffic' : held+' · '+(capturing ? 'capture continues' : 'showing captured traffic');
  }
  showCapturedPage():void {
    this.trafficMenu.hidePopover();const detail=this.selectedDetail;if(!detail||!this.previewPageAvailable||this.previewPageBusy)return;
    this.closePopover(this.requestActionsMenu,this.actionsButton);
    this.previewContextRevision++;this.previewContextBusy=false;this.previewDocumentChoices=[];this.previewDocumentNote='';
    this.previewReusesScene=false;this.previewSourceLabel=null;this.previewPageMode='captured';this.previewDraftExcluded=false;this.$emit('preview-options-request',undefined);void this.loadPreviewRules();this.previewPageId=detail.id;this.previewPageUrl=detail.requests.find(head=>head.boundary==='client-request')?.target??'';this.previewPageGeneration++;this.previewPageStatus='';this.previewPageScope='original-trace';this.previewPageScripts.checked=false;this.previewPageDialog.showModal();
  }
  async loadPreviewRules():Promise<void> {
    const generation=++this.previewChoicesGeneration, requestedRule=this.requestedPreviewRule,latest=this.requestedPreviewVariantLatest;let variant=this.requestedPreviewVariant;this.requestedPreviewRule=null;this.requestedPreviewVariant=undefined;this.requestedPreviewVariantLatest=false;this.previewVariantSelection=variant;this.previewVariantLocked=Boolean(variant);
    this.previewRules=[];this.previewRulesError='';this.previewRuleSummary='';this.previewRulesGeneration=undefined;
    if(this.viewerMode)return;
    this.previewRulesBusy=true;
    try {const status=await invoke<AutomationStatus>('automation_status');if(generation!==this.previewChoicesGeneration||!this.previewPageDialog.open)return;
      this.previewRulesGeneration=status.generation;
      if(variant&&latest){const selection=variant,revision=status.experiments?.find(e=>e.id===selection.experimentId)?.variants.find(v=>v.id===selection.variantId)?.revisions.at(-1)?.revision;if(revision)variant={...selection,revision};this.previewVariantSelection=variant;}
      const selection=variant;
      const variantRules=selection?status.experiments?.find(e=>e.id===selection.experimentId)?.variants.find(v=>v.id===selection.variantId)?.revisions.find(r=>r.revision===selection.revision)?.rules:undefined;
      if(variant&&!variantRules)throw new Error('The selected saved variant revision is unavailable.');
      this.previewRules=(variantRules??status.rules).filter(rule=>rule.request.responseAsset).sort((a,b)=>a.priority-b.priority||(a.id<b.id?-1:a.id>b.id?1:0)).map(rule=>({id:rule.id,name:rule.displayName??rule.id,included:variant?rule.enabled!==false:requestedRule?rule.id===requestedRule:rule.enabled!==false,state:variant?'Saved variant revision '+variant.revision:rule.enabled?'Enabled for live traffic':'Disabled for live traffic'}));
      this.requestedPreviewRule=null;this.updatePreviewRuleSummary();
    } catch(error:unknown) {if(generation===this.previewChoicesGeneration)this.previewRulesError='Saved rules could not be loaded: '+describeError(error);}
    finally {if(generation===this.previewChoicesGeneration)this.previewRulesBusy=false;}
  }
  setPreviewMode(event:Event):void {this.previewPageStatus='';this.previewPageMode=(event.currentTarget as HTMLSelectElement).value;}
  setPreviewRule(id:string,event:Event):void {if(this.previewVariantSelection)return;this.previewPageStatus='';this.previewRules=this.previewRules.map(row=>row.id===id?{...row,included:(event.currentTarget as HTMLInputElement).checked}:row);this.updatePreviewRuleSummary();}
  excludePreviewDraft(event:Event):void {this.previewPageStatus='';this.previewDraftExcluded=(event.currentTarget as HTMLInputElement).checked;}
  private updatePreviewRuleSummary():void {const selected=this.previewRules.filter(row=>row.included);this.previewRuleSummary=selected.length+' selected'+(selected.length?': '+selected.slice(0,3).map(row=>row.name.length>90?row.name.slice(0,89)+'…':row.name).join(', ')+(selected.length>3?' and '+(selected.length-3)+' more':''):'');}
  async showRulePreview(intent:ResponsePreviewIntent):Promise<void> {
    if(this.viewerMode||this.previewPageBusy)return;
    const revision=++this.previewContextRevision,selection=this.selectedSessionId;
    if(intent.sourceSessionId){
      this.previewContextBusy=true;
      try {
        const choices=await invoke<{pages:Array<{id:string;url:string;startedAt:number;recordedReferrer:boolean}>;more:boolean}>('captured_page_choices',{sourceId:intent.sourceSessionId});
        if(revision!==this.previewContextRevision||!this.isConnected||this.view!=='traffic'||selection!==this.selectedSessionId)return;
        if(!choices.pages.length)throw new Error('No complete HTML response remains in this source. Import its page or select another captured page in Traffic.');
        const previous=this.previewSceneAvailable&&choices.pages.find(page=>page.id===this.previewSceneDocumentId);
        this.requestedPreviewRule=intent.mode==='modified'?intent.ruleId:null;this.requestedPreviewVariant=intent.mode==='modified'?intent.variant:undefined;
        if(previous)this.showRelatedPreview(intent.mode);
        else {
          this.previewReusesScene=false;this.previewSourceLabel=null;this.previewPageScope='original-trace';
          const page=choices.pages.find(page=>page.id===this.selectedSessionId)??choices.pages[0]!;
          this.previewPageId=page.id;this.previewPageUrl=page.url;this.previewPageGeneration++;this.previewPageStatus='';
          this.previewDraftExcluded=false;this.$emit('preview-options-request',undefined);this.previewPageScripts.checked=false;
          this.previewPageDialog.showModal();void this.loadPreviewRules();
        }
        this.previewPageMode=intent.mode;
        this.previewDocumentChoices=choices.pages.map(page=>({id:page.id,url:page.url,label:(page.recordedReferrer?'Recorded referrer · ':'')+page.url}));
        this.previewDocumentNote='Choose the captured HTML page for this resource. Recorded referrer/time proximity are hints, not proof of a navigation.'+(choices.more?' Showing up to 100 pages; select another HTML response directly in Traffic for additional choices.':'');
      }catch(error:unknown){if(revision===this.previewContextRevision&&this.isConnected&&this.view==='traffic'&&selection===this.selectedSessionId)this.showNotice('Choose a captured page',describeError(error),null,null);}
      finally{if(revision===this.previewContextRevision)this.previewContextBusy=false;}
      return;
    }
    this.requestedPreviewRule=intent.mode==='modified'?intent.ruleId:null;this.requestedPreviewVariant=intent.mode==='modified'?intent.variant:undefined;
    if(this.previewReportAvailable&&this.previewSceneAvailable)this.showRelatedPreview(intent.mode);
    else if(this.previewPageAvailable){this.showCapturedPage();this.previewPageMode=intent.mode;}
    else {this.requestedPreviewRule=null;this.requestedPreviewVariant=undefined;this.showNotice('Select a captured page','Select a complete HTML response in Traffic, then choose Preview page… and include this saved rule.',null,null);return;}
  }
  showRelatedPreview(mode:string):void {
    if(this.previewPageBusy||!this.previewSceneAvailable)return;
    if(mode==='modified'&&!this.requestedPreviewRule&&!this.requestedPreviewVariant&&this.previewSceneVariant){this.requestedPreviewVariant={...this.previewSceneVariant};this.requestedPreviewVariantLatest=true;}
    this.previewContextRevision++;this.previewContextBusy=false;this.previewDocumentChoices=[];this.previewDocumentNote='';this.previewPageId=this.previewSceneDocumentId;
    this.closePopover(this.requestActionsMenu,this.actionsButton);this.closePreviewReport();this.previewReusesScene=true;this.previewPageUrl=this.previewSceneUrl;this.previewPageScope=this.previewSceneScope;this.previewSourceLabel=this.previewReportLabel;this.previewPageMode=mode;this.previewDraftExcluded=false;
    this.$emit('preview-options-request',undefined);this.previewPageGeneration++;this.previewPageStatus='';this.previewPageScripts.checked=false;this.previewPageDialog.showModal();void this.loadPreviewRules();
  }
  showPreviewRule(id:string):void {this.closePreviewReport();this.$emit('matched-rule-request',id);}
  setPreviewScope(event:Event):void {this.previewPageScope=(event.currentTarget as HTMLSelectElement).value;}
  async showPreviewReport():Promise<void> {const active=(this.getRootNode() as ShadowRoot).activeElement as HTMLElement|null;this.previewReportFocus=active&&this.requestActionsMenu.contains(active)?this.actionsButton:active;this.previewReportDialog.showModal();await this.refreshPreviewReport();}
  async refreshPreviewReport():Promise<void> {
    const label=this.previewReportLabel,generation=++this.previewReportGeneration;this.previewReportBusy=true;this.previewReportError='';
    try {const report=await invoke<CapturedPageReport>('captured_page_report',{label});if(generation!==this.previewReportGeneration||!this.isConnected)return;
      this.previewVariantText=report.variant?`${report.variant.experimentId} / ${report.variant.variantId} / revision ${report.variant.revision}`:'';
      this.previewSceneVariant=report.variant??undefined;
      this.previewSceneUrl=report.url;this.previewSceneScope=report.scope;this.previewRuleRows=report.rules??[];this.previewSceneAvailable=report.bytesAvailable!==false;this.previewReportUrl=report.url;this.previewReportSummary=(this.previewRuleRows.length?'Modified':'Baseline')+' · '+(report.runtime??'WebView2')+' · '+(report.bytesAvailable===false?'Related preview bytes expired · ':'')+(report.scope==='all-loaded'?'All loaded traffic':report.source)+' · '+report.available+' frozen variants · '+report.skipped+' skipped · '+report.hits+' served · '+report.misses+' network failures · '+(report.failures??0)+' failed · '+Math.max(0,report.available+report.skipped-report.resources.length)+' omitted resource decisions · '+Math.max(0,report.hits+report.misses+(report.failures??0)-report.requests.length)+' older requests'+(report.scriptsEnabled==null?'':report.scriptsEnabled?' · Scripts enabled':' · Scripts disabled');
      this.previewResourceRows=report.resources.map((row,index)=>({id:String(index),sourceId:row.entryId,sourceAvailable:row.sourceAvailable!==false,method:row.method,url:row.url,source:row.source,size:row.bytes==null?'Unavailable':row.bytes.toLocaleString()+' bytes',time:row.unixMillis==null||!Number.isFinite(new Date(row.unixMillis).getTime())?'Time unavailable':new Date(row.unixMillis).toISOString(),decision:row.decision+(row.sha256?' · SHA-256 '+row.sha256:'')}));
      this.previewRequestRows=report.requests.map(row=>({id:String(row.id),method:row.method,url:row.url,outcome:row.outcome==='failed/unavailable'?'Failed / unavailable':row.outcome==='auto-response'?'Auto-response':row.outcome==='captured'||row.outcome==='served'?'Captured':'Missing · Network failure',reason:row.reason,ruleId:row.ruleId??'',sourceId:row.entryId??'',sha256:row.sha256??'',comparable:row.outcome==='captured'||row.outcome==='auto-response'||row.outcome==='served'}));
    }catch(error:unknown){if(generation===this.previewReportGeneration)this.previewReportError='Preview diagnostics could not be read: '+describeError(error);}finally{if(generation===this.previewReportGeneration)this.previewReportBusy=false;}
  }
  async showPreviewSource(id:string):Promise<void> {try{await invoke('session_detail',{id});this.closePreviewReport();await this.revealSession(id);}catch{this.previewReportError='The source entry is no longer retained. Its resource decision remains in this report.';}}
  closePreviewReport(event?:Event):void {event?.preventDefault();this.previewReportGeneration++;this.previewReportBusy=false;this.previewReportDialog.close();this.previewReportFocus?.focus();this.previewReportFocus=null;}
  closeCapturedPageWarning():void {if(this.previewPageOperation)void invoke('cancel_captured_page',{operationId:this.previewPageOperation}).catch(()=>{});this.previewContextRevision++;this.previewContextBusy=false;this.previewPageGeneration++;this.previewChoicesGeneration++;this.previewRulesBusy=false;this.previewPageDialog.close();this.actionsButton.focus({preventScroll:true});}
  async openCapturedPage():Promise<void> {
    if(this.previewPageBusy)return;const modified=this.previewPageMode==='modified';if(modified&&(this.previewRulesBusy||this.previewRulesError||!this.previewRules.some(row=>row.included)&&!this.previewVariantSelection||this.previewDraftDirty&&!this.previewDraftExcluded)){this.previewPageStatus='Select saved rules and resolve or explicitly exclude unsaved changes before opening.';return;}this.previewPageBusy=true;this.previewPageStatus='Preparing captured responses…';
    const generation=this.previewPageGeneration,id=this.previewPageId,enableScripts=this.previewPageScripts.checked,operationId=crypto.randomUUID();this.previewPageOperation=operationId;
    try{const label=await invoke<string>('open_captured_page',{id,enableScripts,operationId,sourceLabel:this.previewSourceLabel,options:modified?{scope:this.previewPageScope,...(this.previewVariantSelection?{variant:this.previewVariantSelection}:{ruleIds:this.previewRules.filter(row=>row.included).map(row=>row.id),generation:this.previewRulesGeneration})}:{scope:this.previewPageScope}});this.previewReportLabel=label;this.previewReportAvailable=true;this.previewSceneAvailable=true;this.previewSceneDocumentId=id;this.previewSceneVariant=modified?this.previewVariantSelection:undefined;this.previewSceneUrl=this.previewPageUrl;this.previewSceneScope=this.previewPageScope;if(this.isConnected&&generation===this.previewPageGeneration&&this.previewPageDialog.open){this.previewPageDialog.close();this.actionsButton.focus({preventScroll:true});this.diagnostic='Captured page preview opened. Preview diagnostics show resource choices and missing requests.';}}
    catch(error:unknown){if(this.isConnected&&generation===this.previewPageGeneration){this.previewPageStatus='Preview could not be opened: '+describeError(error);if(!this.previewPageDialog.open&&!describeError(error).includes('canceled'))this.showNotice('Captured page preview failed',describeError(error),null,null);}}
    finally {this.previewPageBusy=false;this.previewPageOperation='';}
  }
  @observable saveTraceFormat='native';
  changeSaveTraceFormat(event:Event):void {this.saveTraceFormat=(event.target as HTMLSelectElement).value;}
  showSaveTrace():void {this.closePopover(this.filesMenu,this.filesMenuButton);if(!this.savingTrace){this.saveTraceStatus='';this.exportReviewScope='all';void this.reviewTraceExport();}this.saveTraceOpen=true;this.saveTraceDialog.showModal();}
  closeSaveTrace():void {this.saveTraceDialog.close();}
  saveTraceClosed():void {this.saveTraceOpen=false;if(!this.savingTrace){this.exportReviewRevision++;this.exportReviewBusy=false;this.exportReviewId="";this.exportReviewFacts=[];this.exportReviewNote="";this.exportReviewSummary="";}this.filesMenuButton.focus({preventScroll:true});}
  async saveTrafficTrace(event:Event):Promise<void> {
    event.preventDefault();if(this.savingTrace||this.exportReviewBusy||!this.exportReviewId)return;
    const data=new FormData(this.saveTraceForm);
    this.savingTrace=true;this.saveTraceCompleted=0;this.saveTraceTotal=0;
    this.saveTraceStatus='Choose a destination, then the trace will be saved…';
    let onProgress:Channel<{phase:string;completed:number;total:number}>|undefined;
    try {
      const password=data.get('encrypt')==='on'?await this.promptTracePassword('Encrypt saved trace',true):null;
      if(data.get('encrypt')==='on'&&password===null){this.saveTraceStatus='Save canceled.';return;}
      onProgress=new Channel<{phase:string;completed:number;total:number}>();
      onProgress.onmessage=progress=>{
      if(!this.isConnected||!this.savingTrace)return;
      this.saveTraceCompleted=progress.completed;this.saveTraceTotal=progress.total;
      const phase=progress.phase==='writing'?'Writing trace':progress.phase==='converting'?'Creating SAZ archive':progress.phase==='finalizing'?'Finalizing trace':'Preparing trace';
      this.saveTraceStatus=phase+(progress.total?` · ${progress.completed.toLocaleString()} of ${progress.total.toLocaleString()} entries…`:'…');
      };
      const result=await invoke<{destination:string;entries:number;bytes:number;incompleteBodies:number;placeholderEntries:number;connectionEntries:number;omittedIncidentMarkers?:number}|null>('save_traffic_trace',{options:{reviewId:this.exportReviewId,includeOriginalContext:data.get('originalContext')==='on',format:String(data.get('format')??'native'),password,includeNetworkContext:data.get('networkContext')==='on',redactSensitiveHeaders:data.get('redactHeaders')==='on'},onProgress});
      if(!this.isConnected)return;
      if(result){
        this.saveTraceDialog.close();
        this.showNotice('Trace saved',result.destination+' · '+result.entries.toLocaleString()+' entries'+(result.omittedIncidentMarkers?' · '+result.omittedIncidentMarkers+' incident markers omitted by format/context choices.':'')+(result.connectionEntries?' · '+result.connectionEntries.toLocaleString()+' generated connection '+(result.connectionEntries===1?'summary':'summaries'):'')+(result.placeholderEntries?' · '+result.placeholderEntries.toLocaleString()+' entries use labeled placeholders for uncaptured headers.':'')+(result.incompleteBodies?' · '+result.incompleteBodies.toLocaleString()+' body boundaries were unavailable or incomplete.':''),null,null);
      }else this.saveTraceStatus='Save canceled.';
    }
    catch(error:unknown){if(onProgress)onProgress.onmessage=()=>{};this.savingTrace=false;if(this.isConnected){this.saveTraceStatus='Trace could not be saved: '+describeError(error);await this.showError('Trace save failed',describeError(error));}}
    finally {if(onProgress)onProgress.onmessage=()=>{};this.savingTrace=false;}
  }
  async exportLiveCapture():Promise<void> {
    try { const result = await invoke<{destination:string;records:number;bytes:number}>('export_live_capture'); this.showNotice('TMCap export complete',result.destination,null,null); }
    catch (error:unknown) { await this.showError('TMCap export failed',describeError(error)); }
  }
  disconnectedCallback():void {
    this.clearColumnDrag();
    for(const unlisten of this.nativeUnlisteners)unlisten();this.nativeUnlisteners=[];if(this.importOperation)void invoke('cancel_trace_import',{operationId:this.importOperation}).catch(()=>{});if(this.searchOperation)void invoke('cancel_traffic_search',{operationId:this.searchOperation}).catch(()=>{});this.previewReportGeneration++;this.matchGeneration++;this.metadataGeneration++;
    window.clearTimeout(this.searchTimer); this.layoutObserver?.disconnect(); if (this.sessionUpdates) this.sessionUpdates.onmessage = () => undefined;
    this.rowObserver?.disconnect();window.clearTimeout(this.refreshTimer);if(this.renderFrame!==undefined)cancelAnimationFrame(this.renderFrame);this.releaseView(this.trafficView);this.releaseView(this.pendingView);this.scrollGeneration++;
    if(this.previewPageOperation)void invoke('cancel_captured_page',{operationId:this.previewPageOperation}).catch(()=>{});
    this.inspectionGeneration++; super.disconnectedCallback();
  }
}
TrafficWorkspace.define('traffic-workspace');
