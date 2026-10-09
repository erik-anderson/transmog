import { WebUIElement, attr, observable } from '@microsoft/webui-framework';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWebview, type DragDropEvent } from '@tauri-apps/api/webview';
import type { TraceImportIntent } from '../models.js';
import { describeError } from '../utilities.js';

const acceptsTrace = (path:string):boolean => /\.(tmcap|saz|har|netlog|json)$/i.test(path);

/** Shared trace picker and native drop surface. Imports are owned by Traffic. */
export class TraceDropTarget extends WebUIElement {
  @attr variant='box';
  @attr({mode:'boolean'}) active=false;
  @attr({mode:'boolean'}) busy=false;
  @observable traceDragActive=false;
  @observable pickingTrace=false;
  dropRegion!:HTMLElement;
  private paths:string[]=[];
  private unlisten:(()=>void)|null=null;
  private listenerGeneration=0;

  protected hydratedCallback():void {void this.watchDrops();}
  connectedCallback():void {const reconnect=Boolean(this.dropRegion);super.connectedCallback();if(reconnect)void this.watchDrops();}
  activeChanged():void {if(!this.active)this.clearDrag();}
  busyChanged():void {if(this.busy)this.clearDrag();}
  private clearDrag():void {this.paths=[];this.traceDragActive=false;}
  private async watchDrops():Promise<void> {
    const generation=++this.listenerGeneration;
    try {const unlisten=await getCurrentWebview().onDragDropEvent(event=>this.handleNativeDrag(event.payload));
      if(!this.isConnected||generation!==this.listenerGeneration){unlisten();return;}this.unlisten=unlisten;
    }catch { /* Browser fixtures do not expose native drag events. */ }
  }
  handleNativeDrag(event:DragDropEvent):void {
    if(event.type==='leave'||!this.active||this.busy||this.pickingTrace){this.clearDrag();return;}
    if(event.type==='enter'||event.type==='drop')this.paths=event.paths;
    const bounds=this.dropRegion.getBoundingClientRect(), x=event.position.x/window.devicePixelRatio, y=event.position.y/window.devicePixelRatio;
    const accepted=this.paths.length>0&&this.paths.every(acceptsTrace)&&x>=bounds.left&&x<=bounds.right&&y>=bounds.top&&y<=bounds.bottom&&bounds.width>0&&bounds.height>0;
    this.traceDragActive=accepted;
    if(event.type==='drop'){
      const paths=accepted?this.paths.slice(0,16):[];this.clearDrag();
      if(paths.length)this.$emit('trace-import-request',{paths} satisfies TraceImportIntent);
    }
  }
  async chooseTrace():Promise<void> {
    if(!this.active||this.busy||this.pickingTrace)return;
    this.pickingTrace=true;
    try {const path=await invoke<string|null>('pick_trace_path');if(path&&this.isConnected&&this.active&&!this.busy)this.$emit('trace-import-request',{paths:[path]} satisfies TraceImportIntent);}
    catch(error:unknown){if(this.isConnected)this.$emit('operation-error',{title:'Could not choose trace',message:describeError(error),resolve:()=>{}});}
    finally {this.pickingTrace=false;}
  }
  disconnectedCallback():void {this.listenerGeneration++;this.unlisten?.();this.unlisten=null;this.clearDrag();super.disconnectedCallback();}
}
TraceDropTarget.define('trace-drop-target');
