import { WebUIElement, attr, observable } from '@microsoft/webui-framework';
import { invoke } from '@tauri-apps/api/core';
import { describeError } from '../utilities.js';
import type { HexPreview } from '../hex-viewer/hex-viewer.js';

interface Field { label:string; value:string; }
interface Certificate { sha256:string; subject:string; fields:Field[]; pem:string; derBase64:string; unavailable:string; }
interface Connection { id:string; title:string; fields:Field[]; serverChain:string[]; clientChain:string[]; errors:string[]; }
interface Inspection { defaultConnectionId?:string; connections:Connection[]; certificates:Certificate[]; }
interface ChainRow { id:string; label:string; selected:boolean; }

export class CertificateInspector extends WebUIElement {
  @observable certificateBusy=false;
  @observable certificateStatus='';
  @observable certificateConnections:Array<{id:string;title:string;selected:boolean}>=[];
  @observable certificateConnectionFields:Field[]=[];
  @observable certificateErrors:Array<{id:number;text:string}>=[];
  @observable certificateServerChain:ChainRow[]=[];
  @observable certificateClientChain:ChainRow[]=[];
  @observable certificateFields:Field[]=[];
  @observable certificateSubject='Select a certificate';
  @observable certificateUnavailable='';
  @observable certificatePem='';
  @observable certificateHex:HexPreview|null=null;
  @observable certificateTab='details';
  @observable certificateHasBytes=false;
  @attr section='all';
  certificateRepresentation!:HTMLElement;
  private inspection:Inspection|null=null;
  private connectionId='';
  private certificateId='';
  private generation=0;

  async inspect(id:string):Promise<void> {
    const generation=++this.generation;
    this.inspection=null;this.connectionId='';this.certificateId='';
    this.certificateConnections=[];this.certificateConnectionFields=[];this.certificateErrors=[];
    this.certificateServerChain=[];this.certificateClientChain=[];this.clearCertificate();
    this.certificateBusy=true;this.certificateStatus='Loading recorded TLS context…';
    try {
      const inspection=await invoke<Inspection>('inspect_certificates',{id});
      if(generation!==this.generation||!this.isConnected)return;
      this.inspection=inspection;this.certificateConnections=inspection.connections.map(connection=>({id:connection.id,title:connection.title,selected:false}));
      const preferred=inspection.connections.find(connection=>connection.id===inspection.defaultConnectionId)
        ??inspection.connections.find(connection=>connection.title==='Transmog → server')??inspection.connections[0];
      this.selectConnection(preferred?.id??'');
      this.certificateStatus='';
    }catch(error){if(generation===this.generation)this.certificateStatus=describeError(error);}
    finally{if(generation===this.generation)this.certificateBusy=false;}
  }
  changeConnection(event:Event):void {this.selectConnection((event.currentTarget as HTMLSelectElement).value);}
  selectConnection(id:string):void {
    this.connectionId=id;
    const connection=this.inspection?.connections.find(connection=>connection.id===id);
    this.certificateConnections=this.certificateConnections.map(connection=>({...connection,selected:connection.id===id}));
    this.certificateConnectionFields=connection?.fields??[];this.certificateErrors=(connection?.errors??[]).map((text,id)=>({id,text}));
    this.selectCertificate(connection?.serverChain[0]??connection?.clientChain[0]??'');
  }
  selectCertificate(id:string):void {
    this.certificateId=id;
    const connection=this.inspection?.connections.find(connection=>connection.id===this.connectionId);
    const rows=(ids:string[])=>ids.map((id,index)=>({id,label:(index===0?'Leaf · ':'Issuer · ')+(this.inspection?.certificates.find(certificate=>certificate.sha256===id)?.subject??id),selected:id===this.certificateId}));
    this.certificateServerChain=rows(connection?.serverChain??[]);this.certificateClientChain=rows(connection?.clientChain??[]);
    const certificate=this.inspection?.certificates.find(certificate=>certificate.sha256===id);
    this.clearCertificate();
    if(!certificate)return;
    this.certificateSubject=certificate.subject;this.certificateFields=certificate.fields;this.certificatePem=certificate.pem;
    this.certificateUnavailable=certificate.unavailable;this.certificateHasBytes=!!certificate.derBase64;
    this.certificateHex=certificate.derBase64?{key:certificate.sha256,bytesBase64:certificate.derBase64,offset:0,truncated:false}:null;
  }
  private clearCertificate():void {
    this.certificateSubject='No public certificate was recorded';this.certificateFields=[];this.certificatePem='';
    this.certificateUnavailable='';this.certificateHex=null;this.certificateHasBytes=false;this.certificateTab='details';
  }
  showTab(tab:string):void {this.certificateTab=tab;this.$flushUpdates();this.certificateRepresentation.scrollIntoView({block:'nearest'});}
  async copyPem():Promise<void> {
    const generation=this.generation;
    try{await navigator.clipboard.writeText(this.certificatePem);if(generation===this.generation)this.certificateStatus='Public certificate PEM copied.';}
    catch(error){if(generation===this.generation)this.certificateStatus=describeError(error);}
  }
  cancelInspection():void {++this.generation;this.certificateBusy=false;}
  disconnectedCallback():void {this.cancelInspection();super.disconnectedCallback();}
}
CertificateInspector.define('certificate-inspector');
