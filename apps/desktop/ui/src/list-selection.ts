/** Stable-ID selection shared by manageable lists. Focus and selection are distinct. */
export class ListSelection {
  ids=new Set<string>();
  focused:string|null=null;
  anchor:string|null=null;
  clone():ListSelection {const copy=new ListSelection();copy.ids=new Set(this.ids);copy.focused=this.focused;copy.anchor=this.anchor;return copy;}
  clear():void {this.ids.clear();this.anchor=null;this.focused=null;}
  replace(id:string):void {this.ids=new Set([id]);this.focused=id;this.anchor=id;}
  choose(id:string,order:string[],modifiers:{ctrlKey?:boolean;shiftKey?:boolean;metaKey?:boolean}={}):void {
    const additive=modifiers.ctrlKey||modifiers.metaKey;
    this.focused=id;
    if(modifiers.shiftKey && this.anchor && order.includes(this.anchor)) {
      const a=order.indexOf(this.anchor),b=order.indexOf(id);const range=order.slice(Math.min(a,b),Math.max(a,b)+1);
      this.ids=new Set(additive?[...this.ids,...range]:range);
    } else if(additive) {if(this.ids.has(id))this.ids.delete(id);else this.ids.add(id);this.anchor=id;}
    else this.replace(id);
  }
  key(id:string,order:string[],event:KeyboardEvent):boolean {
    const control=event.ctrlKey||event.metaKey;
    if(control && event.key.toLowerCase()==='a') {event.preventDefault();this.ids=new Set(order);this.focused=id;this.anchor=order[0]??null;return true;}
    if(event.key==='Escape') {event.preventDefault();this.clear();return true;}
    if(event.key===' ' && control) {event.preventDefault();this.choose(id,order,{ctrlKey:true});return true;}
    if(!['ArrowUp','ArrowDown','Home','End','Enter',' '].includes(event.key))return false;
    event.preventDefault();let index=order.indexOf(id);
    if(event.key==='Home')index=0;else if(event.key==='End')index=order.length-1;else if(event.key==='ArrowUp')index--;else if(event.key==='ArrowDown')index++;
    const next=order[Math.max(0,Math.min(order.length-1,index))];if(!next)return true;
    if(control && !event.shiftKey && event.key!=='Enter' && event.key!==' ')this.focused=next;
    else this.choose(next,order,{ctrlKey:control,shiftKey:event.shiftKey});
    return true;
  }
}

export function isTextEditing(event:KeyboardEvent):boolean {
  const target=event.composedPath()[0];
  if(!(target instanceof HTMLElement))return false;
  const field=target.closest('input,textarea,select,[contenteditable="true"]');
  return field instanceof HTMLInputElement?!['checkbox','radio','button','submit'].includes(field.type):Boolean(field);
}
