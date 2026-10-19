import {WebUIElement,attr,observable} from '@microsoft/webui-framework';
import {invoke} from '@tauri-apps/api/core';
import {listen,type UnlistenFn} from '@tauri-apps/api/event';
import {describeError} from '../utilities.js';
import type {HexPreview} from '../hex-viewer/hex-viewer.js';
import {DOCUMENT_NODE,documentNode,formatText,nodeIndex,tablePage,treeRows,valueText,type Presentation,type Span,type TreeRow,type VisualizerChoice,type VisualizerNode,type VisualizerSelection,type VisualizerView} from './models.js';

let nextGeneration=0n,nextRenderer=0;
const next=():string=>(++nextGeneration).toString();
export class ExtensionVisualizer extends WebUIElement {
  @attr slot='visualizer';
  @attr({mode:'boolean'}) active=false;
  @observable selection:VisualizerSelection|null=null;
  @observable choice:VisualizerChoice|null=null;
  @observable visualizerBusy=false;
  @observable visualizerPaused=false;
  @observable visualizerError='';
  @observable visualizerTitle='';
  @observable visualizerStatus='';
  @observable visualizerSummary='';
  @observable visualizerRows:TreeRow[]=[];
  @observable visualizerCanvasHeight='0px';
  @observable visualizerActiveRow='';
  @observable visualizerSelectedName='';
  @observable visualizerValue='';
  @observable visualizerFormat='';
  @observable visualizerMode:Presentation='tree';
  @observable visualizerSpans:Array<Span&{id:string;label:string}>=[];
  @observable visualizerDiagnostics:Array<{code:string;message:string;nodeId:string|null}>=[];
  @observable visualizerTable:ReturnType<typeof tablePage>={columns:[],rows:[],text:'',previous:false,next:false,limited:false};
  @observable visualizerSources:Array<{id:string;name:string;description:string}>=[];
  @observable visualizerSource='';
  @observable visualizerSourceText='';
  @observable visualizerHex:HexPreview|null=null;
  treeViewport!:HTMLDivElement;
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
  private readonly rendererId='visualizer-'+nextRenderer++;
  private readonly visibility=():void=>{if(!document.hidden)this.suspended=false;this.schedule();};
  activeChanged():void {this.schedule();}
  selectionChanged():void {this.schedule();}
  choiceChanged(previous:VisualizerChoice|null|undefined):void {if(previous?.key!==this.choice?.key)this.paused=false;this.schedule();}
  slotChanged():void {this.schedule();}
  protected hydratedCallback():void {
    this.observer=new IntersectionObserver(entries=>{this.inViewport=entries.some(entry=>entry.isIntersecting);this.schedule();});
    this.observer.observe(this);
    this.resize=new ResizeObserver(()=>this.windowRows());this.resize.observe(this.treeViewport);
    document.addEventListener('visibilitychange',this.visibility);
    this.schedule();
  }
  private schedule():void {
    if(this.queued)return;this.queued=true;
    queueMicrotask(()=>{this.queued=false;this.reconcile();});
  }
  private get visible():boolean {return this.isConnected&&this.active&&this.inViewport&&!document.hidden&&!this.suspended;}
  private reconcile():void {
    const selection=this.selection,choice=this.choice;
    const signature=this.visible&&selection&&choice?JSON.stringify([this.slot,selection.sessionId,selection.boundary,selection.surface,choice.extensionId,choice.visualizerId]):'';
    if(signature===this.signature)return;
    if(!signature){const wasOpen=!!this.signature;this.stop();if(wasOpen)this.pause();return;}
    if(this.paused)return;
    this.stop();this.signature=signature;
    if(signature)void this.start();
  }
  private watch():void {
    const revision=++this.listenerRevision;
    const add=(event:string,handler:()=>void):void=>{void listen(event,handler).then(unlisten=>{if(revision===this.listenerRevision&&this.visible)this.listeners.push(unlisten);else void Promise.resolve(unlisten()).catch(()=>undefined);}).catch(()=>{if(revision===this.listenerRevision)this.fail('Visualizer updates could not be observed. Retry to reopen it.');});};
    add('visualizers-suspended',()=>{this.suspended=true;this.stop();this.pause();});
    add('extensions-changed',()=>{const handle=this.view?.handle;if(handle)void invoke<boolean>('visualizer_active',{handle}).then(active=>{if(revision===this.listenerRevision&&!active)this.fail('This extension changed or was disabled. Choose an available viewer to continue.');}).catch(()=>{if(revision===this.listenerRevision)this.fail('The visualizer is no longer available.');});});
  }
  private unwatch():void {this.listenerRevision++;for(const unlisten of this.listeners)void Promise.resolve(unlisten()).catch(()=>undefined);this.listeners=[];}
  private stop():void {
    this.revision++;const pending=this.pending;this.pending=null;this.signature='';this.unwatch();
    if(pending)void invoke('deactivate_visualizer',pending).catch(()=>undefined);
    this.clear();
  }
  private clear():void {
    this.view=null;this.root=null;this.nodes.clear();this.allRows=[];this.visualizerRows=[];
    this.visualizerBusy=false;this.visualizerPaused=false;this.visualizerTitle='';this.visualizerStatus='';this.visualizerSummary='';this.visualizerError='';
    this.visualizerSelectedName='';this.visualizerValue='';this.visualizerFormat='';this.visualizerSpans=[];this.visualizerDiagnostics=[];this.visualizerSources=[];this.visualizerSource='';this.visualizerSourceText='';this.visualizerHex=null;this.visualizerCanvasHeight='0px';this.visualizerActiveRow='';
    this.visualizerTable={columns:[],rows:[],text:'',previous:false,next:false,limited:false};
    this.expanded=new Set([DOCUMENT_NODE]);this.selected=DOCUMENT_NODE;this.tableOffset=0;
  }
  private fail(message:string):void {const signature=this.signature;this.stop();this.signature=signature;this.visualizerError=message;}
  private pause():void {this.paused=true;this.visualizerPaused=true;this.visualizerError='Visualizer paused. Open it again to continue with available retained evidence.';}
  private async start():Promise<void> {
    const selection=this.selection,choice=this.choice;if(!selection||!choice)return;
    const revision=++this.revision,slot=this.slot,generation=next();this.visualizerBusy=true;this.watch();
    try {
      const pageId=await invoke<string>('visualizer_page');
      if(revision!==this.revision||!this.visible)return;
      this.pending={slot,generation,pageId};
      const view=await invoke<VisualizerView>('activate_visualizer',{request:{...this.pending,selection,extensionId:choice.extensionId,visualizerId:choice.visualizerId}});
      if(revision!==this.revision||!this.visible){void invoke('deactivate_visualizer',{slot,generation,pageId}).catch(()=>undefined);return;}
      this.accept(view);
    }catch(error:unknown){if(revision===this.revision)this.fail(describeError(error));}
    finally {if(revision===this.revision)this.visualizerBusy=false;}
  }
  retry():void {this.suspended=false;this.paused=false;this.stop();this.schedule();}
  private accept(view:VisualizerView):void {
    this.view=view;this.root=documentNode(view.document);this.nodes=nodeIndex(this.root);
    if(!this.nodes.has(this.selected))this.selected=DOCUMENT_NODE;
    this.visualizerTitle=view.document.title;this.visualizerStatus=view.document.status;this.visualizerSummary=view.document.summary??'';this.visualizerDiagnostics=view.document.diagnostics;
    this.visualizerSources=[...view.sources.map(source=>({id:source.id,name:source.name,description:`${source.length} bytes · ${source.availability}`})),...view.document.sources.map(source=>({id:source.id,name:source.name,description:`Derived · ${source.transform} · ${source.exactMapping?'exact parent mapping':'enclosing parent regions only'}`}))];
    this.updateRows();this.describeNode();
  }
  private updateRows():void {
    this.allRows=this.root?treeRows(this.root,this.expanded,this.selected,this.rendererId):[];
    this.visualizerCanvasHeight=this.allRows.length*26+'px';this.windowRows();
  }
  windowRows():void {
    const first=Math.max(0,Math.floor((this.treeViewport?.scrollTop??0)/26)-4),count=Math.ceil((this.treeViewport?.clientHeight??260)/26)+8;
    this.visualizerRows=this.allRows.slice(first,first+Math.max(12,count));
    this.visualizerActiveRow=this.visualizerRows.find(row=>row.id===this.selected)?.domId??'';
  }
  selectNode(id:string):void {
    if(!this.nodes.has(id))return;this.selected=id;this.tableOffset=0;this.updateRows();this.describeNode();
  }
  clickNode(id:string):void {this.selectNode(id);this.treeViewport.focus({preventScroll:true});}
  private describeNode():void {
    const node=this.nodes.get(this.selected);if(!node)return;
    this.visualizerSelectedName=node.name;this.visualizerValue=valueText(node.value);this.visualizerFormat=formatText(node.format);
    this.visualizerMode=node.presentation??this.view?.document.presentation??'tree';
    this.visualizerSpans=node.spans.map((span,i)=>({id:String(i),...span,label:`${this.visualizerSources.find(source=>source.id===span.sourceId)?.name??span.sourceId} · offset ${span.offset} · ${span.length} bytes`}));
    this.visualizerTable=tablePage(node,this.tableOffset);this.visualizerHex=null;
    if(this.visualizerMode==='hex')this.openSpan(node.spans[0]);
  }
  async toggleNode(id:string,event?:Event):Promise<void> {
    event?.stopPropagation();const node=this.nodes.get(id);if(!node)return;
    if(this.expanded.has(id)){this.expanded.delete(id);this.updateRows();return;}
    if(node.childrenToken&&this.view&&!this.visualizerBusy){
      const handle=this.view.handle,revision=this.revision;this.visualizerBusy=true;
      try {const view=await invoke<VisualizerView>('expand_visualizer',{handle,token:node.childrenToken});if(revision!==this.revision||!this.visible)return;this.expanded.add(id);this.accept(view);}
      catch(error:unknown){if(revision===this.revision)this.fail(describeError(error));}
      finally {if(revision===this.revision)this.visualizerBusy=false;}
    }else {this.expanded.add(id);this.updateRows();}
  }
  treeKey(event:KeyboardEvent):void {
    const index=this.allRows.findIndex(row=>row.id===this.selected),row=this.allRows[index];if(!row)return;
    let target=index;
    switch(event.key){
      case 'ArrowDown':target=Math.min(index+1,this.allRows.length-1);break;
      case 'ArrowUp':target=Math.max(0,index-1);break;
      case 'Home':target=0;break;case 'End':target=this.allRows.length-1;break;
      case 'ArrowRight':if(row.expandable&&!row.expanded){event.preventDefault();void this.toggleNode(row.id);return;}target=Math.min(index+1,this.allRows.length-1);break;
      case 'ArrowLeft':if(row.expanded){event.preventDefault();void this.toggleNode(row.id);return;}target=Math.max(0,this.allRows.findIndex(candidate=>candidate.id===row.parent));break;
      case 'Enter':case ' ':event.preventDefault();if(row.expandable)void this.toggleNode(row.id);return;
      default:return;
    }
    event.preventDefault();this.selectNode(this.allRows[target]!.id);
    const top=target*26,bottom=top+26;
    if(top<this.treeViewport.scrollTop)this.treeViewport.scrollTop=top;
    else if(bottom>this.treeViewport.scrollTop+this.treeViewport.clientHeight)this.treeViewport.scrollTop=bottom-this.treeViewport.clientHeight;
    this.windowRows();
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
    if(derived)this.visualizerSourceText+=` · Parents: ${derived.parents.map(parent=>`${this.visualizerSources.find(source=>source.id===parent.sourceId)?.name??parent.sourceId}, offset ${parent.offset}, ${parent.length} bytes`).join('; ')}`;
  }
  disconnectedCallback():void {
    this.stop();this.observer?.disconnect();this.resize?.disconnect();this.observer=null;this.resize=null;this.inViewport=false;
    document.removeEventListener('visibilitychange',this.visibility);super.disconnectedCallback();
  }
}
ExtensionVisualizer.define('extension-visualizer');
