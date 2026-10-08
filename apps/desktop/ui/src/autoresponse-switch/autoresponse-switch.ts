import { observable } from '@microsoft/webui-framework';
import { WorkspaceElement } from '../workspace-element.js';
import type { AutomationStatus } from '../models.js';

export class AutoresponseSwitch extends WorkspaceElement {
  @observable state:AutomationStatus|null=null;
  @observable busy=false;
  @observable autoresponsesLabel='Auto-responses · Loading…';
  @observable autoresponsesOn=false;
  @observable autoresponsesPressed='false';
  stateChanged():void {
    this.autoresponsesOn=this.state?.autoresponsesEnabled??true;
    this.autoresponsesPressed=String(this.autoresponsesOn);
    const count=this.state?.rules.filter(rule=>rule.enabled!==false && rule.request.responseAsset!==null).length??0;
    this.autoresponsesLabel=this.state?`Auto-responses · ${this.autoresponsesOn?'On':'Paused'} (${count} enabled)`:'Auto-responses · Loading…';
  }
  toggle():void {if(!this.busy && this.state)this.$emit('toggle-autoresponses');}
}
AutoresponseSwitch.define('autoresponse-switch');
