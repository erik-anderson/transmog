import { observable } from '@microsoft/webui-framework';
import { WorkspaceElement } from '../workspace-element.js';
import type { AutomationStatus } from '../models.js';
import { activeSavedResponseCount } from '../automation-state.js';

export class AutoresponseSwitch extends WorkspaceElement {
  @observable state:AutomationStatus|null=null;
  @observable busy=false;
  @observable autoresponsesLabel='Saved responses · Loading…';
  @observable autoresponsesOn=false;
  @observable autoresponsesPressed='false';
  stateChanged():void {
    this.autoresponsesOn=this.state?.autoresponsesEnabled??true;
    this.autoresponsesPressed=String(this.autoresponsesOn);
    const count=activeSavedResponseCount(this.state);
    this.autoresponsesLabel=this.state?`Saved responses · ${this.autoresponsesOn?'On':'Paused'} (${count} active)`:'Saved responses · Loading…';
  }
  toggle():void {if(!this.busy && this.state)this.$emit('toggle-autoresponses');}
}
AutoresponseSwitch.define('autoresponse-switch');
