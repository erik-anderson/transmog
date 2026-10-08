import { attr, observable } from '@microsoft/webui-framework';
import { Channel, invoke } from '@tauri-apps/api/core';
import { WorkspaceElement } from '../workspace-element.js';
import type { AutomationStatus, ColumnId, Lifecycle, SessionSummary, SessionPage, SessionHint, SessionDetail, TrafficFilter, TrafficSort, WorkspacePreferences } from '../models.js';
import { describeError, loadSessionDetail, clientResponseSource, autoResponseUnavailableReason } from '../utilities.js';
import { cellText, columnDefinitions, defaultWorkspace, displayColumns, statusTone } from '../table-model.js';
import {ListSelection,isTextEditing} from '../list-selection.js';

type Column = ReturnType<typeof displayColumns>[number] & {sortDirection:string;sortArrow:string};
type Row = SessionSummary & {tone:string;selectionState:string;rowLabel:string;cells:Array<{id:ColumnId;text:string;title:string;pinned:boolean;numeric:boolean;offsetCss:string;tone:string}>};

export class TrafficWorkspace extends WorkspaceElement {
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
  @observable removingTraffic=false;
  @observable trafficUndoText='';
  @observable trafficMenuX='12px';
  @observable trafficMenuY='80px';
  trafficMenu!:HTMLElement;
  trafficUndoButton!:HTMLButtonElement;
  private trafficSelection=new ListSelection();
  private selectedTraffic=new Map<string,SessionSummary>();
  private trafficUndo:Array<{ids:string[];rows:SessionSummary[]}>=[];
  @observable selectedDetail:SessionDetail | null = null;
  @observable selectedMethodText = '—';
  @observable selectedUrlText = 'Select a request';
  @observable selectedStatusText = 'No response';
  @observable selectedTone = 'pending';
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
  }
  lifecycleChanged():void { this.renderSessionState(); this.renderFollowState(); }
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
  async clearFilters():Promise<void> { this.clearTrafficSelection();this.filters = []; this.searchText = ''; this.searchInput.value = ''; this.pageIndex = 0; this.queryRevision++; await this.refreshSessions(undefined,true); }
  searchChanged(event:Event):void {
    this.clearTrafficSelection();
    this.searchText = (event.currentTarget as HTMLInputElement).value;
    window.clearTimeout(this.searchTimer);
    this.searchTimer = window.setTimeout(() => { this.pageIndex = 0; this.queryRevision++; void this.refreshSessions(undefined,true); },250);
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
    this.clearColumnDrag();
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
      const page = await invoke<SessionPage>('query_sessions',{query:{cursor:null,latest:false,limit:this.pageLimit,terminal:null,method:null,host:null,search:this.searchText || null,sort:this.sort,offset:this.pageIndex*this.pageLimit,filters:this.filters.map(({column,operator,value}) => ({column,operator,value}))}});
      if (!this.isConnected || revision !== this.queryRevision) return;
      this.totalMatched = page.totalMatched ?? page.sessions.length;
      this.pageEnd = Math.min(this.totalMatched,this.pageIndex*this.pageLimit+page.sessions.length);
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
    if(event.key==='Delete') {event.preventDefault();void this.removeSelectedTraffic();return;}
    if((event.ctrlKey||event.metaKey) && event.key.toLowerCase()==='z') {event.preventDefault();void this.undoTrafficRemoval();return;}
    if(!this.trafficSelection.key(session.id,this.sessions.map(row=>row.id),event))return;
    this.updateTrafficSelection();this.$flushUpdates();
    const id=this.trafficSelection.focused;const row=this.sessions.find(row=>row.id===id);
    if(row){(this.getRootNode() as ShadowRoot).getElementById('session-'+row.id)?.focus();if(this.trafficSelection.ids.has(row.id))void this.inspectSession(row,true);}
  }
  listKeyboard(event:KeyboardEvent):void {
    if(event.defaultPrevented || isTextEditing(event))return;
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
  selectedResponses():void {this.trafficMenu.hidePopover();this.$emit('autoresponse-batch-request',Array.from(this.selectedTraffic.keys()));}
  async removeSelectedTraffic():Promise<void> {
    if(this.removingTraffic || !this.trafficSelection.ids.size)return;this.removingTraffic=true;
    const ids=[...this.trafficSelection.ids],rows=[...this.selectedTraffic.values()];const index=this.sessions.findIndex(row=>row.id===this.trafficSelection.focused);
    try {
      const removed=await invoke<string[]>('remove_traffic_entries',{ids,restore:false});
      this.trafficUndo.push({ids:removed,rows});if(this.trafficUndo.length>20)this.trafficUndo.shift();
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
    const action=this.trafficUndo.at(-1);if(!action || this.removingTraffic)return;this.removingTraffic=true;
    try {
      const restored=await invoke<string[]>('remove_traffic_entries',{ids:action.ids,restore:true});this.trafficUndo.pop();
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
    this.selectedDetail = detail; const reusable = clientResponseSource(detail) !== null;
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
  replaySelected():void { if (this.selectedDetail) this.$emit('replay-request',this.selectedDetail); }
  async resumeLatest():Promise<void> { this.followLatest = true; this.pageIndex = 0; this.sort = {column:'started-at',direction:'descending'}; this.sortLabel = 'Newest first'; this.rebuildColumns(); this.queryRevision++; this.renderFollowState(); await this.refreshSessions(undefined,true); }
  showUpdates():void { if (this.pendingRows.length) this.rebuildRows(this.pendingRows); this.displayedMatched = this.totalMatched; this.pendingRows = []; this.updatesPending = false; this.newTrafficCount = 0; this.renderSessionState(); }
  async navigatePage(delta:number):Promise<void> { this.pageIndex = Math.max(0,this.pageIndex+delta); this.followLatest = false; this.queryRevision++; await this.refreshSessions(undefined,true); }
  private renderSessionState():void {
    const state = this.pending || this.lifecycle;
    const labels:Record<string,string> = {stopped:'Proxy stopped.',running:'Proxy running.',starting:'Starting proxy.',stopping:'Stopping proxy.',failed:'Proxy failed.'};
    const empty = this.filters.length || this.searchText ? 'No matching exchanges.' : state === 'running' ? 'Waiting for proxied requests.' : state === 'stopped' ? 'Start the proxy to capture traffic.' : 'No exchanges captured.';
    const summary = this.queryError || this.watchError || (!this.queryLoaded ? 'Loading captured traffic…' : this.totalMatched ? 'Loaded '+this.sessions.length+' of '+this.totalMatched+' exchanges.' : empty);
    this.sessionText = (labels[state] ?? 'Proxy status unavailable.')+' '+summary;
    this.sessionKind = this.queryError || this.watchError || state === 'failed' ? 'error' : !this.queryLoaded || this.pending || state === 'stopping' ? 'progress' : state === 'running' ? 'success' : 'neutral';
  }
  private renderFollowState():void {
    const capturing = this.lifecycle === 'running' && !this.pending;
    const held = this.selectedSessionId ? 'Inspection pinned' : 'Row positions held';
    this.followText = this.followLatest ? capturing ? 'Following live traffic' : 'Showing captured traffic' : held+' · '+(capturing ? 'capture continues' : 'showing captured traffic');
  }
  async exportLiveCapture():Promise<void> {
    try { const result = await invoke<{destination:string;records:number;bytes:number}>('export_live_capture'); this.showNotice('TMCap export complete',result.destination,null,null); }
    catch (error:unknown) { this.showNotice('TMCap export failed',describeError(error),null,null); }
  }
  disconnectedCallback():void {
    this.clearColumnDrag();
    window.clearTimeout(this.searchTimer); this.layoutObserver?.disconnect(); if (this.sessionUpdates) this.sessionUpdates.onmessage = () => undefined;
    this.inspectionGeneration++; super.disconnectedCallback();
  }
}
TrafficWorkspace.define('traffic-workspace');
