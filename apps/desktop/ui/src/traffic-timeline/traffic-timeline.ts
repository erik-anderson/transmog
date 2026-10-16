import {WebUIElement,attr,observable} from '@microsoft/webui-framework';
import type {TrafficTimelineRow,TraceMetadata} from '../models.js';
import {milliseconds} from '../timings.js';
import {clampWindow,intervalEnd,overlaps,requestAddress,timelineTime,timelineRange} from './timeline-model.js';
import initialState from '../initial-state.json';
import {isTextEditing} from '../list-selection.js';

type DisplayRow=TrafficTimelineRow&{label:string;source:string;host:string;duration:string;description:string;x:number;width:number;responseX:number;responseWidth:number;plottable:boolean;hasResponse:boolean};

export class TrafficTimeline extends WebUIElement {
  @observable entries:TrafficTimelineRow[]=[];
  @observable chosen:string[]=[];
  @observable sources:TraceMetadata[]=[];
  @observable compact=true;
  @attr current='';
  @observable timelineDisplay={...initialState.timelineDisplay,rows:[] as DisplayRow[]};
  viewport!:HTMLElement;
  overview!:SVGSVGElement;
  private observer:ResizeObserver|null=null;
  private scoped:TrafficTimelineRow[]=[];
  private matching:TrafficTimelineRow[]=[];
  private from=0;
  private to=1;
  private minimum=0;
  private maximum=1;
  private adjusted=false;
  private order='start';
  private previousRowHeight=32;
  private get rowHeight():number {return this.compact?32:48;}
  private rebuildPending=false;
  private drag:{pointer:number;target:Element;part:string;x:number;from:number;to:number;adjusted:boolean;scale:number;minimum:number;maximum:number}|null=null;

  protected hydratedCallback():void {this.observer=new ResizeObserver(()=>this.renderRows());this.observer.observe(this.viewport);this.rebuild();}
  entriesChanged():void {if(this.drag)this.rebuildPending=true;else this.rebuild();}
  chosenChanged():void {this.rebuild();}
  sourcesChanged():void {this.renderRows();}
  compactChanged():void {
    const scroll=(this.viewport?.scrollTop??0)/this.previousRowHeight*this.rowHeight;
    this.previousRowHeight=this.rowHeight;this.renderRows(scroll);this.$flushUpdates();
    if(this.viewport)this.viewport.scrollTop=scroll;
  }
  private rebuild():void {
    // Deliberate scope changes cancel a gesture; live snapshots wait for its end.
    if(this.drag)this.cancelDrag();
    const selected=new Set(this.chosen);
    this.scoped=this.entries.filter(row=>!selected.size||selected.has(row.id));
    let first=Infinity,last=-Infinity;
    for(const row of this.scoped)if(row.startMillis!=null){first=Math.min(first,row.startMillis);last=Math.max(last,intervalEnd(row)??row.startMillis);}
    if(!Number.isFinite(first)){first=0;last=1;}
    this.minimum=this.adjusted?Math.min(first,this.from):first;
    this.maximum=Math.max(this.minimum+1,this.adjusted?Math.max(last,this.to):last);
    if(!this.adjusted){this.from=this.minimum;this.to=this.maximum;}
    this.renderRange();
  }
  resetRange():void {this.adjusted=false;this.rebuild();}
  changeOrder(event:Event):void {this.order=(event.currentTarget as HTMLSelectElement).value;this.renderRange();}
  private renderRange():void {
    this.matching=this.scoped.filter(row=>overlaps(row,this.from,this.to)).sort((a,b)=>this.order==='duration'?
      (b.durationMicros??-1)-(a.durationMicros??-1)||(a.startMillis??Infinity)-(b.startMillis??Infinity):
      (a.startMillis??Infinity)-(b.startMillis??Infinity));
    const extent=this.maximum-this.minimum,counts=new Int32Array(201);
    for(const row of this.scoped){const end=intervalEnd(row);if(row.startMillis==null||end==null)continue;
      const start=Math.max(0,Math.min(199,Math.floor((row.startMillis-this.minimum)/extent*200)));
      const finish=Math.max(start,Math.min(199,Math.floor((end-this.minimum)/extent*200)));
      counts[start]!++;counts[finish+1]!--;
    }
    const bins:number[]=[];let count=0,peak=1;
    for(let i=0;i<200;i++){count+=counts[i]!;bins.push(count);peak=Math.max(peak,count);}
    const unavailable=this.scoped.filter(row=>row.startMillis==null||row.durationMicros==null).length;
    const clocks=new Set(this.scoped.map(row=>row.traceId??'live'));
    this.timelineDisplay={...this.timelineDisplay,
      densityPath:bins.map((value,index)=>value?`M${index*5} 40V${40-value/peak*36}H${index*5+5}V40Z`:'').join(''),
      windowX:(this.from-this.minimum)/extent*1000,windowWidth:(this.to-this.from)/extent*1000,
      startHandle:(this.from-this.minimum)/extent*1000-4,endHandle:(this.to-this.minimum)/extent*1000-4,
      fromValue:(this.from-this.minimum)/extent*1000,toValue:(this.to-this.minimum)/extent*1000,
      ...timelineRange((this.from-this.minimum)*1000,(this.to-this.minimum)*1000),
      origin:this.minimum>0?timestamp(this.minimum):'Start time unavailable',
      summary:(this.matching.length===this.scoped.length?`${this.scoped.length.toLocaleString()}${this.chosen.length?' selected':''} request${this.scoped.length===1?'':'s'}`:`${this.matching.length.toLocaleString()} of ${this.scoped.length.toLocaleString()}${this.chosen.length?' selected':''} requests in range`)+(unavailable?` · ${unavailable.toLocaleString()} without timing`:''),
      clocks:clocks.size>1?'Recorded source clocks may differ; alignment does not establish cross-source chronology.':'Times are relative to the first recorded start in this scope.',
      selected:this.chosen.length>0,empty:this.matching.length===0,mixedClocks:clocks.size>1};
    this.renderRows();
  }
  renderRows(scroll=this.viewport?.scrollTop??0):void {
    const height=this.viewport?.clientHeight??300,rowHeight=this.rowHeight;
    const start=Math.max(0,Math.min(this.matching.length,Math.floor(scroll/rowHeight)-5)),end=Math.min(this.matching.length,start+Math.ceil(height/rowHeight)+11);
    const extent=this.to-this.from,sourceNames=new Map(this.sources.map(source=>[source.id,source.name]));
    const active=this.getRootNode() instanceof ShadowRoot?(this.getRootNode() as ShadowRoot).activeElement:document.activeElement;
    if(active instanceof HTMLElement&&active.closest('.timeline-row')){
      const index=this.matching.findIndex(row=>row.id===active.dataset.id);
      if(index<start||index>=end)this.viewport?.focus({preventScroll:true});
    }
    const rows=this.matching.slice(start,end).map(row=>{
      const finish=intervalEnd(row),plottable=finish!=null&&row.startMillis!=null;
      const x=plottable?Math.max(0,Math.min(1000,(row.startMillis!-this.from)/extent*1000)):0;
      const right=plottable?Math.max(0,Math.min(1000,(finish-this.from)/extent*1000)):0;
      const response=row.startMillis!=null&&row.responseMicros!=null?row.startMillis+row.responseMicros/1000:null;
      const responseX=response==null?right:Math.max(x,Math.min(right,(response-this.from)/extent*1000));
      const source=row.traceId?sourceNames.get(row.traceId)??'Imported capture':'Live traffic';
      const address=requestAddress(row.url),label=`#${row.requestNumber} ${row.method} ${address.path}`,duration=timelineTime(row.durationMicros);
      return {...row,label,host:address.host,source,duration,x,width:Math.max(0,right-x),responseX,responseWidth:Math.max(0,right-responseX),plottable,hasResponse:response!=null,
        description:`#${row.requestNumber} ${row.method} ${row.url} · ${milliseconds(row.durationMicros)} · ${row.terminal} · ${source} · ${row.basis}${row.startMillis==null?' · Start time unavailable':' · '+timestamp(row.startMillis)}`};
    });
    this.timelineDisplay={...this.timelineDisplay,rows,top:start*rowHeight+'px',bottom:Math.max(0,this.matching.length-end)*rowHeight+'px'};
  }
  inspect(id:string):void {this.$emit('timeline-inspect',id);}
  clearSelection():void {this.$emit('timeline-clear-selection');}
  refresh():void {this.$emit('timeline-refresh');}
  selectAll():void {this.$emit('timeline-select',this.matching.map(row=>row.id));}
  timelineKey(event:KeyboardEvent):void {
    if(event.defaultPrevented||isTextEditing(event))return;
    if((event.ctrlKey||event.metaKey)&&event.key.toLowerCase()==='a'){
      event.preventDefault();event.stopPropagation();this.selectAll();
    }else if(event.key==='Escape'){
      event.preventDefault();event.stopPropagation();
      if(this.drag)this.cancelDrag();else this.clearSelection();
    }
  }
  reveal(id:string):void {
    const index=this.matching.findIndex(row=>row.id===id);
    if(index<0){this.viewport.focus({preventScroll:true});return;}
    const top=index*this.rowHeight;
    if(top<this.viewport.scrollTop)this.viewport.scrollTop=top;
    else if(top+this.rowHeight>this.viewport.scrollTop+this.viewport.clientHeight)this.viewport.scrollTop=top+this.rowHeight-this.viewport.clientHeight;
    this.renderRows();this.$flushUpdates();
    (this.getRootNode() as ShadowRoot).getElementById('timeline-'+id)?.focus({preventScroll:true});
  }
  rowKey(id:string,event:KeyboardEvent):void {
    if(!['ArrowDown','ArrowUp','Home','End'].includes(event.key))return;
    event.preventDefault();event.stopPropagation();
    const index=this.matching.findIndex(row=>row.id===id),next=event.key==='Home'?0:event.key==='End'?this.matching.length-1:Math.max(0,Math.min(this.matching.length-1,index+(event.key==='ArrowDown'?1:-1)));
    const row=this.matching[next];if(!row)return;
    this.reveal(row.id);this.inspect(row.id);
  }
  startDrag(event:PointerEvent):void {
    if(event.button!==0||!this.scoped.some(row=>intervalEnd(row)!=null))return;
    event.preventDefault();event.stopPropagation();
    const target=event.target as Element,part=target.getAttribute('data-part')??'new';
    (target instanceof SVGElement&&target.hasAttribute('tabindex')?target:this.overview).focus({preventScroll:true});
    const bounds=this.overview.getBoundingClientRect(),scale=(this.maximum-this.minimum)/Math.max(1,bounds.width);
    this.drag={pointer:event.pointerId,target:this.overview,part,x:event.clientX,from:this.from,to:this.to,adjusted:this.adjusted,scale,minimum:this.minimum,maximum:this.maximum};
    this.overview.setPointerCapture(event.pointerId);
    if(part==='new'){const minimumWidth=Math.min(1,Math.max(.001,(this.maximum-this.minimum)/1000));this.from=Math.min(this.maximum-minimumWidth,this.minimum+Math.max(0,Math.min(bounds.width,event.clientX-bounds.left))*scale);this.to=this.from+minimumWidth;this.adjusted=true;this.renderRange();}
  }
  moveDrag(event:PointerEvent):void {
    const drag=this.drag;if(!drag||drag.pointer!==event.pointerId)return;
    const delta=(event.clientX-drag.x)*drag.scale;
    if(drag.part==='new'){
      const bounds=this.overview.getBoundingClientRect(),anchor=drag.minimum+Math.max(0,Math.min(bounds.width,drag.x-bounds.left))*drag.scale;
      const point=Math.max(drag.minimum,Math.min(drag.maximum,anchor+delta));
      const minimumWidth=Math.min(1,Math.max(.001,(drag.maximum-drag.minimum)/1000));
      this.from=Math.min(drag.maximum-minimumWidth,Math.min(anchor,point));this.to=Math.min(drag.maximum,Math.max(this.from+minimumWidth,Math.max(anchor,point)));
    }else [this.from,this.to]=clampWindow(drag.from,drag.to,drag.minimum,drag.maximum,drag.part,delta);
    this.adjusted=true;this.renderRange();
  }
  finishDrag(event:PointerEvent):void {if(this.drag?.pointer===event.pointerId)this.releaseDrag();}
  cancelDrag():void {const drag=this.drag;if(!drag)return;this.from=drag.from;this.to=drag.to;this.adjusted=drag.adjusted;this.releaseDrag();this.renderRange();}
  private releaseDrag():void {const drag=this.drag;this.drag=null;if(drag?.target.hasPointerCapture(drag.pointer))drag.target.releasePointerCapture(drag.pointer);if(this.rebuildPending){this.rebuildPending=false;this.rebuild();}}
  rangeKey(part:string,event:KeyboardEvent):void {
    if(event.key==='Escape'&&this.drag){event.preventDefault();event.stopPropagation();this.cancelDrag();return;}
    if(!['ArrowLeft','ArrowRight','Home','End','+','-','0'].includes(event.key))return;
    event.preventDefault();event.stopPropagation();
    if(event.key==='0'){this.resetRange();return;}
    const extent=this.maximum-this.minimum;
    if(event.key==='+'||event.key==='-'){
      const width=Math.min(extent,Math.max(Math.min(1,extent),(this.to-this.from)*(event.key==='+'?.8:1.25))),center=(this.from+this.to)/2;
      this.from=Math.max(this.minimum,Math.min(this.maximum-width,center-width/2));this.to=this.from+width;
    }else {
      const delta=event.key==='Home'?-extent:event.key==='End'?extent:extent*(event.shiftKey?.1:.01)*(event.key==='ArrowLeft'?-1:1);
      [this.from,this.to]=clampWindow(this.from,this.to,this.minimum,this.maximum,part,delta);
    }
    this.adjusted=true;this.renderRange();
  }
  disconnectedCallback():void {this.releaseDrag();this.observer?.disconnect();super.disconnectedCallback();}
}
function timestamp(value:number):string {const date=new Date(value);return Number.isFinite(date.getTime())?date.toISOString():'Outside supported calendar range';}
TrafficTimeline.define('traffic-timeline');
