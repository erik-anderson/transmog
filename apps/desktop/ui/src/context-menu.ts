export interface MenuPoint {x:number;y:number}

/** Own measurements/listeners only; templates retain ownership of positioning. */
export class MeasuredMenu {
  private anchor:MenuPoint={x:8,y:8};
  private readonly observer=new ResizeObserver(()=>this.position());
  constructor(private readonly element:()=>HTMLElement,private readonly update:(x:string,y:string)=>void) {}
  open(anchor:MenuPoint):void {
    this.anchor=anchor;this.observer.observe(this.element());window.addEventListener('resize',this.position);this.position();
  }
  private readonly position=():void=>{
    const menu=this.element();if(!menu.matches(':popover-open'))return;
    const box=menu.getBoundingClientRect(),zoom=menu.currentCSSZoom||1;
    this.update(Math.max(8,Math.min(this.anchor.x,innerWidth-box.width-8))/zoom+'px',Math.max(8,Math.min(this.anchor.y,innerHeight-box.height-8))/zoom+'px');
  };
  close():void {this.observer.disconnect();window.removeEventListener('resize',this.position);}
}

export function menuAnchor(event:MouseEvent|KeyboardEvent,source:HTMLElement):MenuPoint {
  const rect=source.getBoundingClientRect();
  return event instanceof MouseEvent&&event.button===2?{x:event.clientX,y:event.clientY}:{x:rect.left,y:rect.bottom};
}

export function navigateMenu(event:KeyboardEvent,items:HTMLElement[],active:Element|null):void {
  if(!['ArrowDown','ArrowUp','Home','End'].includes(event.key)||!items.length)return;
  event.preventDefault();event.stopPropagation();const index=items.indexOf(active as HTMLElement);
  const next=event.key==='Home'?0:event.key==='End'?items.length-1:(index+(event.key==='ArrowDown'?1:items.length-1))%items.length;
  items[next]?.focus();
}

export function copied(source:HTMLElement):void {source.dispatchEvent(new CustomEvent('inspection-copied',{bubbles:true,composed:true,detail:{source}}));}
