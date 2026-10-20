import {WebUIElement,attr,observable} from '@microsoft/webui-framework';
import {invoke} from '@tauri-apps/api/core';
import {listen,type UnlistenFn} from '@tauri-apps/api/event';
import {describeError} from '../utilities.js';
import type {HexPreview} from '../hex-viewer/hex-viewer.js';
import {ReloadObserver} from './reload.js';
import {DOCUMENT_NODE,documentNode,formatText,nodeAncestors,nodeIndex,sourceDescription,tablePage,treeRows,valueText,type LiveStatus,type LiveUpdate,type Presentation,type Span,type TreeRow,type VisualizerChoice,type VisualizerNode,type VisualizerSelection,type VisualizerView} from './models.js';

let nextGeneration=0n,nextRenderer=0;
const next=():string=>(++nextGeneration).toString();
const paneSplits = new Map<string, {vertical: number; horizontal: number}>();
export class ExtensionVisualizer extends WebUIElement {
  @attr slot='visualizer';
  @attr({mode:'boolean'}) active=false;
  @attr({mode:'boolean'}) automatic=false;
  @observable selection:VisualizerSelection|null=null;
  @observable choice:VisualizerChoice|null=null;
  @observable visualizerBusy=false;
  @observable visualizerReloading=false;
  @observable visualizerReloadNotice='';
  @observable visualizerPaused=false;
  @observable visualizerLiveSupported=false;
  @observable visualizerLiveAvailable=false;
  @observable visualizerLive=false;
  @observable visualizerLiveWarning='';
  @observable visualizerFrozen=false;
  @observable visualizerError='';
  @observable visualizerTitle='';
  @observable visualizerStatus='';
  @observable visualizerSummary='';
  @observable visualizerRows:TreeRow[]=[];
  @observable visualizerCanvasHeight='0px';
  @observable visualizerActiveRow='';
  @observable visualizerDetailsOpen=false;
  @observable visualizerSplit = '48';
  @observable visualizerTreeShare = '48%';
  @observable visualizerOrientation = 'vertical';
  @observable visualizerInitialSplit = '48';
  @observable visualizerSelectedName='';
  @observable visualizerValue='';
  @observable visualizerFormat='';
  @observable visualizerMode:Presentation='tree';
  @observable visualizerSpans:Array<Span&{id:string;label:string}>=[];
  @observable visualizerDiagnostics:Array<{code:string;message:string;nodeId:string|null}>=[];
  @observable visualizerDiagnosticStatus = '';
  @observable visualizerTable:ReturnType<typeof tablePage>={columns:[],rows:[],text:'',previous:false,next:false,limited:false};
  @observable visualizerSources:Array<{id:string;name:string;description:string}>=[];
  @observable visualizerSource='';
  @observable visualizerSourceText='';
  @observable visualizerHex:HexPreview|null=null;
  treeViewport!:HTMLDivElement;
  detailsViewport!:HTMLElement;
  aboutButton!:HTMLButtonElement;
  aboutPanel!:HTMLElement;
  liveControl?: HTMLInputElement;
  private view:VisualizerView|null=null;
  private root:VisualizerNode|null=null;
  private nodes=new Map<string,VisualizerNode>();
  private expanded=new Set<string>([DOCUMENT_NODE]);
  private selected=DOCUMENT_NODE;
  private allRows:TreeRow[]=[];
  private tableOffset=0;
  private revision=0;
  private pending:{slot:string;generation:string;pageId:string}|null=null;
  private signature='';
  private inViewport=false;
  private suspended=false;
  private paused=false;
  private queued=false;
  private observer:IntersectionObserver|null=null;
  private resize:ResizeObserver|null=null;
  private listeners:UnlistenFn[]=[];
  private listenerRevision=0;
  private liveTimer:ReturnType<typeof setTimeout>|null=null;
  private livePoll:Promise<void>|null=null;
  private retryLive=false;
  private liveWanted=false;
  private checkActivityAfterReload=false;
  private readonly reloadObserver=new ReloadObserver({
    state:()=>this.visible&&this.view?.instance.development?{revision:this.revision,handle:this.view.handle,busy:this.visualizerBusy}:null,
    replace:()=>this.reload(),notice:message=>{this.visualizerReloadNotice=message;},
  });
  private readonly rendererId='visualizer-'+nextRenderer++;
  readonly visualizerAboutId=this.rendererId+'-about';
  private readonly visibility=():void=>{if(!document.hidden)this.suspended=false;this.schedule();};
  activeChanged():void {this.schedule();}
  automaticChanged():void {this.paused=false;this.schedule();}
  selectionChanged():void {this.schedule();}
  choiceChanged(previous:VisualizerChoice|null|undefined):void {if(previous?.key!==this.choice?.key)this.paused=false;this.schedule();}
  slotChanged():void {this.updateLayout();this.schedule();}
  protected hydratedCallback():void {
    this.observer=new IntersectionObserver(entries=>{this.inViewport=entries.some(entry=>entry.isIntersecting);this.schedule();});
    this.observer.observe(this);
    this.resize=new ResizeObserver(()=>{this.updateLayout();this.windowRows();});
    this.resize.observe(this);this.resize.observe(this.treeViewport);
    this.updateLayout();
    document.addEventListener('visibilitychange',this.visibility);
    this.schedule();
  }
  private updateLayout(): void {
    if (!this.isConnected) return;
    const orientation = this.clientWidth <= 500 ? 'horizontal' : 'vertical';
    const splits = paneSplits.get(this.slot);
    const value = splits?.[orientation] ?? (orientation === 'vertical' ? 48 : 40);
    this.visualizerOrientation = orientation;
    this.visualizerInitialSplit = orientation === 'vertical' ? '48' : '40';
    this.visualizerSplit = String(value);
    this.visualizerTreeShare = value + '%';
  }
  resizeStructure(event: CustomEvent<{value: number; committed: boolean}>): void {
    event.stopPropagation();
    const splits = paneSplits.get(this.slot) ?? {vertical: 48, horizontal: 40};
    const value = Math.max(15, Math.min(85, event.detail.value));
    if (this.visualizerOrientation === 'vertical') splits.vertical = value;
    else splits.horizontal = value;
    // Host panes have stable identities. Bound preferences for transient/test panes too.
    if (!paneSplits.has(this.slot) && paneSplits.size >= 32) paneSplits.delete(paneSplits.keys().next().value!);
    paneSplits.set(this.slot, splits);
    this.visualizerSplit = String(value);
    this.visualizerTreeShare = value + '%';
    this.windowRows();
  }
  private schedule():void {
    if(this.queued)return;this.queued=true;
    queueMicrotask(()=>{this.queued=false;this.reconcile();});
  }
  private get visible():boolean {return this.isConnected&&this.active&&this.inViewport&&!document.hidden&&!this.suspended;}
  private reconcile():void {
    const selection=this.selection,choice=this.choice;
    const signature=this.visible&&selection&&choice?JSON.stringify([this.slot,selection.sessionId,selection.boundary,selection.surface,choice.extensionId,choice.visualizerId,this.automatic]):'';
    if(signature===this.signature)return;
    if(!signature){const wasOpen=!!this.signature;this.stop();if(wasOpen)this.pause();return;}
    if(this.paused&&!this.automatic)return;
    this.stop();this.signature=signature;
    if(signature)void this.start();
  }
  private watch():void {
    const revision=++this.listenerRevision;
    const add=(event:string,handler:(event:{payload:unknown})=>void):void=>{void listen(event,handler).then(unlisten=>{if(revision===this.listenerRevision&&this.visible)this.listeners.push(unlisten);else void Promise.resolve(unlisten()).catch(()=>undefined);}).catch(()=>{if(revision===this.listenerRevision)this.fail('Visualizer updates could not be observed. Retry to reopen it.');});};
    add('visualizers-suspended',()=>{this.suspended=true;this.stop();this.pause();});
    add('extensions-changed',()=>this.extensionsChanged(revision));
    add('extension-reload-requested',event=>{if(event.payload===this.choice?.extensionId)this.reloadObserver.checkSoon();});
  }
  private extensionsChanged(revision:number):void {
    if(this.visualizerReloading){this.checkActivityAfterReload=true;return;}
    const handle=this.view?.handle;if(!handle)return;
    void invoke<boolean>('visualizer_active',{handle}).then(active=>{
      if(this.visualizerReloading){this.checkActivityAfterReload=true;return;}
      if(revision!==this.listenerRevision||handle.instanceId!==this.view?.handle.instanceId)return;
      if(!active)this.fail('This extension changed or was disabled. Choose an available viewer to continue.');
      else this.reloadObserver.checkSoon();
    }).catch(()=>{if(this.visualizerReloading){this.checkActivityAfterReload=true;return;}if(revision===this.listenerRevision&&handle.instanceId===this.view?.handle.instanceId)this.fail('The visualizer is no longer available.');});
  }
  private unwatch():void {this.listenerRevision++;for(const unlisten of this.listeners)void Promise.resolve(unlisten()).catch(()=>undefined);this.listeners=[];}
  private stop(preserveResult=false):void {
    this.revision++;const pending=this.pending;this.pending=null;this.signature='';this.unwatch();
    this.stopPolling();this.retryLive=false;this.liveWanted=false;
    this.reloadObserver.stop();this.visualizerReloading=false;this.checkActivityAfterReload=false;
    if(pending)void invoke('deactivate_visualizer',pending).catch(()=>undefined);
    if(preserveResult){this.view=null;this.visualizerHex=null;this.visualizerFrozen=true;this.visualizerLive=false;this.visualizerBusy=false;}
    else this.clear();
  }
  private clear():void {
    this.view=null;this.root=null;this.nodes.clear();this.allRows=[];this.visualizerRows=[];
    this.visualizerReloadNotice='';
    this.visualizerDiagnosticStatus='';
    this.visualizerBusy=false;this.visualizerPaused=false;this.visualizerTitle='';this.visualizerStatus='';this.visualizerSummary='';this.visualizerError='';
    this.visualizerLiveSupported=false;this.visualizerLiveAvailable=false;this.visualizerLive=false;this.visualizerLiveWarning='';this.visualizerFrozen=false;
    this.visualizerDetailsOpen=false;
    this.visualizerSelectedName='';this.visualizerValue='';this.visualizerFormat='';this.visualizerSpans=[];this.visualizerDiagnostics=[];this.visualizerSources=[];this.visualizerSource='';this.visualizerSourceText='';this.visualizerHex=null;this.visualizerCanvasHeight='0px';this.visualizerActiveRow='';
    this.visualizerTable={columns:[],rows:[],text:'',previous:false,next:false,limited:false};
    this.expanded=new Set([DOCUMENT_NODE]);this.selected=DOCUMENT_NODE;this.tableOffset=0;
  }
  private fail(message:string):void {const signature=this.signature;this.stop();this.signature=signature;this.visualizerError=message;}
  private pause():void {this.paused=true;this.visualizerPaused=true;this.visualizerError='Visualizer paused. Open it again to continue with available retained evidence.';}
  private async start(live=false):Promise<void> {
    const selection=this.selection,choice=this.choice;if(!selection||!choice)return;
    const revision=++this.revision,slot=this.slot,generation=next(),automatic=this.automatic;this.visualizerBusy=true;this.liveWanted=live;this.visualizerLive=live;this.watch();
    try {
      const pageId=await invoke<string>('visualizer_page');
      if(revision!==this.revision||!this.visible)return;
      this.pending={slot,generation,pageId};
      const view=await invoke<VisualizerView>('activate_visualizer',{request:{...this.pending,selection,extensionId:choice.extensionId,visualizerId:choice.visualizerId,live,automatic}});
      if(revision!==this.revision||!this.visible){void invoke('deactivate_visualizer',{slot,generation,pageId}).catch(()=>undefined);return;}
      if(automatic&&view.document.status!=='complete')throw new Error('Auto visualization requires a complete parse');
      this.accept(view);
    }catch(error:unknown){
      if(revision===this.revision){
        if(automatic&&!live){this.stop();this.$emit('automatic-rejected',{selection,key:choice.key});}
        else if(live&&this.root)this.failLive(describeError(error));
        else this.fail(describeError(error));
      }
    }
    finally {if(revision===this.revision){this.visualizerBusy=false;this.scheduleLive(revision);this.reloadObserver.checkSoon();}}
  }
  private async reload():Promise<void> {
    const previous=this.view;if(!previous?.instance.development||!this.visible||this.visualizerBusy)return;
    const revision=++this.revision,generation=next(),handle=previous.handle;
    const pending={slot:handle.slot,pageId:handle.pageId,generation};this.pending=pending;
    this.visualizerBusy=true;this.visualizerReloading=true;this.visualizerReloadNotice='';this.stopPolling();
    try {
      await this.livePoll;
      if(revision!==this.revision||!this.visible)return;
      const view=await invoke<VisualizerView>('reload_visualizer',{request:{handle,generation}});
      if(revision!==this.revision||!this.visible){void invoke('deactivate_visualizer',pending).catch(()=>undefined);return;}
      const stoppedLive=this.liveWanted&&!view.live.enabled;
      if(stoppedLive)this.liveWanted=false;
      this.accept(view);
      if(stoppedLive)this.visualizerReloadNotice='Reloaded. Live updates are off for this view.';
    }catch(error:unknown){if(revision===this.revision)this.visualizerReloadNotice='Reload failed: '+describeError(error)+' Showing the previous result. Retry in Settings → Extensions.';}
    finally {if(revision===this.revision){this.visualizerBusy=false;this.visualizerReloading=false;this.scheduleLive(revision);if(this.checkActivityAfterReload){this.checkActivityAfterReload=false;this.extensionsChanged(this.listenerRevision);}}}
  }
  retry():void {const live=this.retryLive;this.suspended=false;this.paused=false;if(live&&this.visible){this.reopenLive();return;}this.stop();this.schedule();}
  private stopPolling():void {if(this.liveTimer!==null)clearTimeout(this.liveTimer);this.liveTimer=null;}
  private liveStatus(status:LiveStatus):void {
    if(this.view)this.view.live=status;
    const supported=!!this.view?.instance.declaration.liveUpdates&&status.available&&this.view.document.updates!=='final';
    if(!supported&&this.liveControl?.matches(':focus'))this.treeViewport.focus({preventScroll:true});
    this.visualizerLiveSupported=supported;
    if(!this.visualizerLiveSupported){this.liveWanted=false;this.stopPolling();}
    this.visualizerLiveAvailable=status.available;this.visualizerLive=status.enabled&&this.liveWanted;
    this.visualizerLiveWarning=status.missedHints!=='0'?'Some updates may be missing. Reopen the visualizer to rebuild context.':'';
  }
  private failLive(message:string):void {const signature=this.signature;this.stop(true);this.signature=signature;this.retryLive=true;this.visualizerError=message;}
  reopenLive():void {
    if(!this.visible||this.visualizerBusy)return;
    const signature=this.signature;this.stop(true);this.signature=signature;this.visualizerError='';this.visualizerLiveWarning='';this.retryLive=true;
    void this.start(true);
  }
  async setLive(event:Event):Promise<void> {
    const target=event.currentTarget as HTMLInputElement;
    if(!this.view||!this.visualizerLiveAvailable||this.visualizerBusy){target.checked=this.visualizerLive;return;}
    if(target.checked){this.reopenLive();return;}
    this.stopPolling();this.liveWanted=false;this.visualizerLive=false;this.retryLive=false;
    const revision=this.revision,handle=this.view.handle;
    try {
      await this.livePoll;
      if(revision!==this.revision||!this.visible)return;
      const status = await invoke<LiveStatus>('stop_visualizer_live', {handle});
      if (revision === this.revision && this.visible) this.liveStatus(status);
    }catch(error:unknown){if(revision===this.revision)this.failLive(describeError(error));}
  }
  private scheduleLive(revision:number):void {
    if(revision!==this.revision||!this.visible||!this.visualizerLive)return;
    this.stopPolling();
    this.liveTimer=setTimeout(()=>{
      this.liveTimer=null;
      if(revision!==this.revision||!this.visible||!this.visualizerLive)return;
      if(this.visualizerBusy){this.scheduleLive(revision);return;}
      const poll=this.pollLive(revision);this.livePoll=poll;
      void poll.finally(()=>{if(this.livePoll===poll)this.livePoll=null;this.scheduleLive(revision);});
    },250);
  }
  private async pollLive(revision:number):Promise<void> {
    const handle=this.view?.handle;if(!handle)return;
    try {
      const result=await invoke<LiveUpdate>('poll_visualizer_live',{handle});
      if(revision!==this.revision||!this.visible)return;
      if(result.view)this.accept(result.view);else this.liveStatus(result.status);
    }catch(error:unknown){if(revision===this.revision)this.failLive(describeError(error));}
  }
  private accept(view:VisualizerView):void {
    const preserveMode=!!this.root;
    const previousIndex = this.allRows.findIndex(row => row.selected);
    const previousTop = previousIndex * 26;
    const selectedWasVisible = previousIndex >= 0 && previousTop + 26 > this.treeViewport.scrollTop &&
      previousTop < this.treeViewport.scrollTop + this.treeViewport.clientHeight;
    const previous=this.view,hex=this.visualizerHex;
    const retainHex=!!hex&&previous?.handle.instanceId===view.handle.instanceId&&(
      view.sources.some(source=>source.id===this.visualizerSource&&previous.sources.some(old=>old.id===source.id&&old.length===source.length&&old.availability===source.availability&&old.name===source.name))||
      view.document.sources.some(source=>source.id===this.visualizerSource&&previous.document.sources.some(old=>old.id===source.id&&old.bytesBase64===source.bytesBase64&&old.name===source.name&&old.transform===source.transform&&old.exactMapping===source.exactMapping&&JSON.stringify(old.parents)===JSON.stringify(source.parents))));
    this.view=view;this.root=documentNode(view.document);this.nodes=nodeIndex(this.root);
    this.expanded=new Set([...this.expanded].filter(id=>{const node=this.nodes.get(id);return node&&(!node.childrenToken||node.children.length>0);}));
    const retained=preserveMode&&this.nodes.has(this.selected);
    if(!retained){
      if(this.detailsViewport.matches(':focus-within'))this.closeDetails();
      else this.visualizerDetailsOpen=false;
      this.treeViewport.scrollTop=0;
      this.selected=DOCUMENT_NODE;
      this.tableOffset=0;
    }
    this.visualizerFrozen=false;this.liveStatus(view.live);
    this.visualizerTitle=view.document.title;this.visualizerStatus=view.document.status;this.visualizerSummary=view.document.summary??'';this.visualizerDiagnostics=view.document.diagnostics;
    this.visualizerDiagnosticStatus='';
    this.visualizerSources=[...view.sources.map(source=>({id:source.id,name:source.name,description:sourceDescription(source)})),...view.document.sources.map(source=>({id:source.id,name:source.name,description:`Derived · ${source.transform} · ${source.exactMapping?'exact parent mapping':'enclosing parent regions only'}`}))];
    this.updateRows(retained);
    this.describeNode(retained,retained&&retainHex);
    if (retained && selectedWasVisible && this.allRows.findIndex(row => row.selected) !== previousIndex) {
      this.scrollSelectedRow();
    }
  }
  private updateRows(revealSelection = false): void {
    this.allRows = this.root ? treeRows(this.root, this.expanded, this.selected, this.rendererId) : [];
    if (revealSelection && this.root && !this.allRows.some(row => row.selected)) {
      for (const ancestor of nodeAncestors(this.root, this.selected)) this.expanded.add(ancestor);
      this.allRows = treeRows(this.root, this.expanded, this.selected, this.rendererId);
    }
    this.visualizerCanvasHeight = this.allRows.length * 26 + 'px';
    this.windowRows();
  }
  windowRows():void {
    const first=Math.max(0,Math.floor((this.treeViewport?.scrollTop??0)/26)-4),count=Math.ceil((this.treeViewport?.clientHeight??260)/26)+8;
    this.visualizerRows=this.allRows.slice(first,first+Math.max(12,count));
    this.visualizerActiveRow=this.visualizerRows.find(row=>row.id===this.selected)?.domId??'';
  }
  selectNode(id: string): void {
    if (!this.nodes.has(id)) return;
    this.visualizerDetailsOpen = true;
    this.selected = id;
    this.tableOffset = 0;
    this.updateRows(true);
    this.describeNode(this.visualizerMode === 'hex');
    this.scrollSelectedRow();
  }
  private scrollSelectedRow(): void {
    const index = this.allRows.findIndex(row => row.selected);
    if (index < 0) return;
    this.$flushUpdates();
    const top = index * 26, bottom = top + 26;
    if (top < this.treeViewport.scrollTop) this.treeViewport.scrollTop = top;
    else if (bottom > this.treeViewport.scrollTop + this.treeViewport.clientHeight) {
      this.treeViewport.scrollTop = bottom - this.treeViewport.clientHeight;
    }
    this.windowRows();
  }
  closeDetails():void {
    this.visualizerDetailsOpen=false;
    this.treeViewport.focus({preventScroll:true});
  }
  closeAbout():void {
    this.aboutPanel.hidePopover();
    this.aboutButton.focus({preventScroll:true});
  }
  async copyDiagnostics(): Promise<void> {
    const revision = this.revision, view = this.view;
    const text = this.visualizerDiagnostics.map(item => `${item.code}: ${item.message}`).join('\n\n');
    try {
      await navigator.clipboard.writeText(text);
      if (revision === this.revision && view === this.view && this.visible) this.visualizerDiagnosticStatus = 'Diagnostics copied.';
    } catch {
      if (revision === this.revision && view === this.view && this.visible) this.visualizerDiagnosticStatus = 'Select the diagnostic text and use Copy.';
    }
  }
  aboutKey(event:KeyboardEvent):void {
    if(event.key!=='Escape')return;
    event.preventDefault();
    event.stopPropagation();
    this.closeAbout();
  }
  detailsKey(event:KeyboardEvent):void {
    if(event.defaultPrevented||event.key!=='Escape'||!this.visualizerDetailsOpen)return;
    event.preventDefault();
    event.stopPropagation();
    this.closeDetails();
  }
  clickNode(id:string):void {this.selectNode(id);this.treeViewport.focus({preventScroll:true});}
  private describeNode(preserveMode=false,retainHex=false):void {
    const node=this.nodes.get(this.selected);if(!node)return;
    const sameSpans=JSON.stringify(node.spans)===JSON.stringify(this.visualizerSpans.map(({id:_id,label:_label,...span})=>span));
    this.visualizerSelectedName=node.name;this.visualizerValue=valueText(node.value);this.visualizerFormat=formatText(node.format);
    if(!preserveMode)this.visualizerMode=node.presentation??this.view?.document.presentation??'tree';
    this.visualizerSpans=node.spans.map((span,i)=>({id:String(i),...span,label:`${this.visualizerSources.find(source=>source.id===span.sourceId)?.name??span.sourceId} · offset ${span.offset} · ${span.length} bytes`}));
    this.visualizerTable=tablePage(node,this.tableOffset);
    if(retainHex&&sameSpans&&this.visualizerMode==='hex')return;
    this.visualizerHex=null;
    if(this.visualizerMode==='hex')this.openSpan(node.spans[0]);
    if(this.visualizerMode==='hex'&&!node.spans.length)this.visualizerSourceText+=' · No source range for this field.';
  }
  async toggleNode(id: string, event?: Event): Promise<void> {
    event?.stopPropagation();
    const node = this.nodes.get(id);
    if (!node) return;
    if (this.visualizerFrozen && node.childrenToken) return;
    if (this.expanded.has(id)) {
      this.expanded.delete(id);
      this.updateRows();
      if (!this.allRows.some(row => row.id === this.selected)) {
        this.selected = id;
        this.tableOffset = 0;
        this.updateRows();
        this.describeNode(this.visualizerMode === 'hex');
      }
      return;
    }
    if (node.childrenToken) {
      if (!this.view || this.visualizerBusy) return;
      const handle = this.view.handle, revision = this.revision;
      this.visualizerBusy = true;
      try {
        await this.livePoll;
        if (revision !== this.revision || !this.visible) return;
        const token = this.nodes.get(id)?.childrenToken;
        if (!token) {
          this.expanded.add(id);
          this.updateRows();
          return;
        }
        const view = await invoke<VisualizerView>('expand_visualizer', {handle, token});
        if (revision !== this.revision || !this.visible) return;
        this.expanded.add(id);
        this.accept(view);
      } catch (error: unknown) {
        if (revision === this.revision) this.fail(describeError(error));
      } finally {
        if (revision === this.revision) this.visualizerBusy = false;
      }
    } else {
      this.expanded.add(id);
      this.updateRows();
    }
  }
  treeKey(event:KeyboardEvent):void {
    const index=this.allRows.findIndex(row=>row.id===this.selected),row=this.allRows[index];if(!row)return;
    let target=index;
    switch(event.key){
      case 'Escape':this.detailsKey(event);return;
      case 'ArrowDown':target=Math.min(index+1,this.allRows.length-1);break;
      case 'ArrowUp':target=Math.max(0,index-1);break;
      case 'Home':target=0;break;case 'End':target=this.allRows.length-1;break;
      case 'ArrowRight':
        event.preventDefault();
        if(!row.expandable)return;
        if(!row.expanded){void this.toggleNode(row.id);return;}
        target=Math.min(index+1,this.allRows.length-1);
        break;
      case 'ArrowLeft':if(row.expanded){event.preventDefault();void this.toggleNode(row.id);return;}target=Math.max(0,this.allRows.findIndex(candidate=>candidate.id===row.parent));break;
      case 'Enter':case ' ':
        event.preventDefault();
        if(!this.visualizerDetailsOpen)this.visualizerDetailsOpen=true;
        else if(row.expandable)void this.toggleNode(row.id);
        return;
      default:return;
    }
    event.preventDefault();this.selectNode(this.allRows[target]!.id);
  }
  changeMode(event:Event):void {this.visualizerMode=(event.currentTarget as HTMLSelectElement).value as Presentation;if(this.visualizerMode==='hex')this.openSpan(this.nodes.get(this.selected)?.spans[0]);else this.visualizerHex=null;}
  tablePage(delta:number):void {const node=this.nodes.get(this.selected);if(node){this.tableOffset=Math.max(0,this.tableOffset+delta);this.visualizerTable=tablePage(node,this.tableOffset);}}
  showSpan(span:Span):void {this.visualizerMode='hex';this.openSpan(span);}
  changeSource(event:Event):void {this.visualizerSource=(event.currentTarget as HTMLSelectElement).value;this.openSpan(undefined);}
  private openSpan(span:Span|undefined):void {
    if(!this.view)return;
    const id=span?.sourceId??this.visualizerSource??this.visualizerSources[0]?.id;
    const chosen=this.visualizerSources.find(source=>source.id===id)??this.visualizerSources[0];if(!chosen){this.visualizerHex=null;return;}
    this.visualizerSource=chosen.id;this.visualizerSourceText=chosen.description;
    const derived=this.view.document.sources.find(source=>source.id===chosen.id),original=this.view.sources.find(source=>source.id===chosen.id);
    const key=this.view.handle.instanceId+':'+chosen.id;
    this.visualizerHex={key,bytesBase64:derived?.bytesBase64??'',offset:'0',truncated:false,...(original?{source:{kind:'visualizer' as const,handle:this.view.handle,sourceId:original.id,totalBytes:original.length,readLimit:65536}}:{}),...(span?{selection:{offset:span.offset,length:span.length}}:{})};
    const parents=derived?.parents??(original?.httpDecoded?[original.httpDecoded.parent]:[]);
    if(parents.length)this.visualizerSourceText+=` · Parents: ${parents.map(parent=>`${this.visualizerSources.find(source=>source.id===parent.sourceId)?.name??parent.sourceId}, offset ${parent.offset}, ${parent.length} bytes`).join('; ')}`;
  }
  disconnectedCallback():void {
    this.stop();this.observer?.disconnect();this.resize?.disconnect();this.observer=null;this.resize=null;this.inViewport=false;
    document.removeEventListener('visibilitychange',this.visibility);super.disconnectedCallback();
  }
}
ExtensionVisualizer.define('extension-visualizer');
