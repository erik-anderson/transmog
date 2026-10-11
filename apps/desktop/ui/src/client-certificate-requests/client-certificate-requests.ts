import initialState from '../initial-state.json';
import {attr, observable} from '@microsoft/webui-framework';
import {invoke} from '@tauri-apps/api/core';
import {WorkspaceElement} from '../workspace-element.js';
import type {ClientCertificatePending, ClientCertificateRequestDetail, ClientCertificateStatus} from '../models.js';
import {describeError} from '../utilities.js';
type PendingRow=ClientCertificatePending & {origin:string;countLabel:string;selected:boolean};
export class ClientCertificateRequests extends WorkspaceElement {
  @attr({attribute:'viewer-mode',mode:'boolean'}) viewerMode=false;
  @observable certificatePending:PendingRow[]=[];
  @observable certificateBlockedCount=0;
  @observable certificateBlockedLabel='';
  @observable certificatePanelOpen=false;
  @observable certificateRequest:ClientCertificateRequestDetail|null=null;
  @observable certificateRequestId='';
  @observable certificateChoiceId='';
  @observable certificateRequestExpired=false;
  @observable certificateDecisionBusy=false;
  @observable certificateDetailBusy=false;
  @observable certificateRequestText=initialState.certificateRequestText;
  @observable certificateRequestError='';
  certificateRequestPanel!:HTMLElement;
  certificateRequestButton!:HTMLButtonElement;
  private generation=0;
  private detailGeneration=0;
  private refreshing=false;
  private timer:number|undefined;
  protected hydratedCallback():void {void this.refresh();}
  private schedule():void {window.clearTimeout(this.timer);if(this.isConnected)this.timer=window.setTimeout(()=>void this.refresh(),this.certificateBlockedCount||this.certificatePanelOpen?500:1000);}
  private apply(status:ClientCertificateStatus):void {
    this.certificatePending=status.pending.map(pending=>({...pending,origin:`${pending.host.includes(':')?'['+pending.host+']':pending.host}:${pending.port}`,countLabel:`${pending.connections} connection${pending.connections===1?'':'s'}`,selected:pending.id===this.certificateRequestId}));
    this.certificateBlockedCount=status.pending.reduce((count,pending)=>count+pending.connections,0);
    this.certificateBlockedLabel=this.certificateBlockedCount?`${this.certificateBlockedCount} connection${this.certificateBlockedCount===1?'':'s'} waiting for a client certificate`:'Certificate requests';
    if(this.certificateRequestId&&!status.pending.some(pending=>pending.id===this.certificateRequestId)) {
      const newlyExpired=!this.certificateRequestExpired;
      this.certificateRequestExpired=true;this.detailGeneration++;
      this.certificateDetailBusy=false;
      if(newlyExpired&&!this.certificateRequestText)this.certificateRequestText='This connection is no longer waiting. It may have closed, timed out, or been cancelled by a policy change.';
    }
  }
  async refresh():Promise<void> {
    if(this.refreshing||this.certificateDecisionBusy){this.schedule();return;}
    this.refreshing=true;const generation=this.generation;
    try {const status=await invoke<ClientCertificateStatus>('client_certificate_status');if(generation===this.generation&&this.isConnected)this.apply(status);}
    catch(error:unknown){if(generation===this.generation&&this.certificatePanelOpen)this.certificateRequestError='Pending connections could not be refreshed: '+describeError(error);}
    finally {this.refreshing=false;this.schedule();}
  }
  async togglePanel():Promise<void> {
    if(this.viewerMode){await invoke('open_main_window').catch(error=>{this.diagnostic=describeError(error);});return;}
    if(this.certificatePanelOpen){this.certificateRequestPanel.hidePopover();return;}
    this.certificateRequestPanel.showPopover();
    if(!this.certificateRequestId&&this.certificatePending[0])void this.choose(this.certificatePending[0].id);
  }
  panelToggled(event:ToggleEvent):void {
    this.certificatePanelOpen=event.newState==='open';
    if(this.certificatePanelOpen&&!this.certificateRequestId&&this.certificatePending[0])void this.choose(this.certificatePending[0].id);
    if(!this.certificatePanelOpen&&!this.certificateBlockedCount){const root=this.getRootNode();const focused=root instanceof ShadowRoot?root.activeElement:document.activeElement;if(focused===this.certificateRequestButton||focused&&this.certificateRequestPanel.contains(focused))this.$emit('client-certificate-panel-closed');}
  }
  closePanel():void {this.certificateRequestPanel.hidePopover();}
  async choose(id:string):Promise<void> {
    const generation=++this.detailGeneration;
    this.certificateRequestId=id;this.certificateRequestExpired=false;this.certificateChoiceId='';
    this.certificateRequest=null;this.certificateRequestText='';this.certificateRequestError='';this.certificateDetailBusy=true;
    try {const detail=await invoke<ClientCertificateRequestDetail>('client_certificate_request',{id});
      if(generation===this.detailGeneration&&this.certificateRequestId===id&&this.isConnected)this.certificateRequest=detail;
    } catch(error:unknown){if(generation===this.detailGeneration){this.certificateRequestExpired=true;this.certificateRequestError=describeError(error);}}
    finally {if(generation===this.detailGeneration)this.certificateDetailBusy=false;}
  }
  certificateRequestIdChanged():void {this.certificatePending=this.certificatePending.map(pending=>({...pending,selected:pending.id===this.certificateRequestId}));}
  chooseCertificate(event:Event):void {this.certificateChoiceId=(event.target as HTMLSelectElement).value;}
  async decide(action:string):Promise<void> {
    if(this.certificateDecisionBusy||this.certificateRequestExpired||!this.certificateRequest)return;
    if(action==='certificate'&&!this.certificateChoiceId)return;
    const id=this.certificateRequestId;this.generation++;this.certificateDecisionBusy=true;this.certificateRequestError='';
    try {
      const decision=action==='certificate'?{action,identity:this.certificateChoiceId}:{action};
      const status=await invoke<ClientCertificateStatus>('decide_client_certificate',{id,decision});
      if(!this.isConnected)return;
      if(this.certificateRequestId===id){this.certificateRequestText=action==='cancel'?'Connection cancelled.':action==='noCertificate'?'Continuing the same connection without a certificate.':'Selected certificate allowed for this connection. The server will decide whether to accept it.';this.certificateRequestExpired=true;}
      this.apply(status);
    } catch(error:unknown){if(this.isConnected)this.certificateRequestError=describeError(error);}
    finally {this.certificateDecisionBusy=false;void this.refresh();}
  }
  settings():void {this.closePanel();this.$emit('client-certificate-settings-request');}
  dismissStatus():void {this.certificateRequestError='';this.certificateRequestText='';}
  disconnectedCallback():void {window.clearTimeout(this.timer);this.generation++;this.detailGeneration++;super.disconnectedCallback();}
}
ClientCertificateRequests.define('client-certificate-requests');
