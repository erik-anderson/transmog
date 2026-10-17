import {attr,observable,WebUIElement} from '@microsoft/webui-framework';

/** A native nested popover; commands stay in the owning workspace's template. */
export class ActionSubmenu extends WebUIElement {
  @attr label='';
  @observable submenuExpanded='false';
  trigger!:HTMLButtonElement;
  panel!:HTMLElement;
  items!:HTMLSlotElement;
  private commands():HTMLButtonElement[] {return this.items.assignedElements().filter((element):element is HTMLButtonElement=>element instanceof HTMLButtonElement&&!element.hidden&&!element.disabled);}
  focus(options?:FocusOptions):void {this.trigger.focus(options);}
  close(restore=false):void {this.panel.hidePopover();if(restore)this.trigger.focus({preventScroll:true});}
  toggled(event:ToggleEvent):void {this.submenuExpanded=String(event.newState==='open');}
  triggerKey(event:KeyboardEvent):void {
    if(event.key==='ArrowRight'){event.preventDefault();event.stopPropagation();this.panel.showPopover();this.commands()[0]?.focus();}
  }
  panelKey(event:KeyboardEvent):void {
    const commands=this.commands(),index=commands.indexOf(event.target as HTMLButtonElement);
    if(event.key==='Escape'||event.key==='ArrowLeft'){event.preventDefault();event.stopPropagation();this.close(true);}
    else if(['ArrowDown','ArrowUp','Home','End'].includes(event.key)){
      event.preventDefault();event.stopPropagation();
      const next=event.key==='Home'?0:event.key==='End'?commands.length-1:(index+(event.key==='ArrowDown'?1:-1)+commands.length)%commands.length;
      commands[next]?.focus();
    }
  }
  command(event:Event):void {
    if(!(event.target instanceof HTMLButtonElement))return;
    const active=(this.getRootNode() as Document|ShadowRoot).activeElement;
    this.close(active instanceof Node&&this.contains(active));
  }
}
ActionSubmenu.define('action-submenu');
