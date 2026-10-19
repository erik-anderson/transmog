import {observable} from '@microsoft/webui-framework';
import {invoke} from '@tauri-apps/api/core';
import {WorkspaceElement} from '../workspace-element.js';
import {pageIdentity} from '../response-set-page-picker/response-set-page-picker.js';
import {describeError} from '../utilities.js';
import type {ResponseSet,ExperimentPreviewPage,ExperimentPreviewPages,ExperimentSelection} from '../models.js';
import type {SavedDemo,SavedDemoPreparation,SavedDemoPreviewIntent} from '../saved-demo-models.js';

function size(bytes:number):string {return bytes<1024?`${bytes} B`:bytes<1024*1024?`${(bytes/1024).toFixed(1)} KiB`:`${(bytes/1024/1024).toFixed(1)} MiB`;}

/** Durable demo management stays beside the saved response set that owns it. */
export class ResponseSetDemos extends WorkspaceElement {
  @observable ownerName='';
  @observable savingAllowed=true;
  @observable rows:Array<SavedDemo&{modified:string;summary:string}>=[];
  @observable loading=false;
  @observable listError='';
  @observable message='';
  @observable name='';
  @observable pages:Array<ExperimentPreviewPage&{identity:string}>=[];
  @observable selectedId='';
  @observable scope='original-trace';
  @observable pagesLoading=false;
  @observable pagesMore=false;
  @observable busy=false;
  @observable saveError='';
  @observable replacingName='';
  @observable review:SavedDemoPreparation|null=null;
  @observable reviewSummary='';
  @observable missingRows:Array<{id:string;method:string;url:string;reason:string}>=[];
  @observable omittedMissing=0;
  @observable deleteName='';
  @observable deletePending=false;
  @observable deleteError='';
  @observable deleteCompleted=false;
  listDialog!:HTMLDialogElement;
  saveDialog!:HTMLDialogElement;
  deleteDialog!:HTMLDialogElement;
  nameInput!:HTMLInputElement;
  prepareButton!:HTMLButtonElement;
  commitButton!:HTMLButtonElement;
  listCloseButton!:HTMLButtonElement;
  private owner:ResponseSet|null=null;
  private selection:ExperimentSelection|null=null;
  private replace:SavedDemo|null=null;
  private deleting:SavedDemo|null=null;
  private current:()=>boolean=()=>false;
  private returnFocus:HTMLElement|null=null;
  private listEpoch=0;
  private saveEpoch=0;
  private deleteEpoch=0;
  private operation='';
  private closingAll=false;
  private capturedSelectionId='';
  private focusEpoch=0;
  protected hydratedCallback():void {this.addEventListener('focusin',()=>this.focusEpoch++);}

  show(owner:ResponseSet,mode:'list'|'save',current:()=>boolean,savingAllowed=true,capturedSelectionId=''):void {
    const source=this.activeElement(),changed=this.owner?.id!==owner.id||this.owner?.incarnation!==owner.incarnation;
    this.closeAll();
    if(changed){this.rows=[];this.listError='';this.pages=[];this.selectedId='';this.replace=null;this.saveError='';}
    this.owner=structuredClone(owner);this.ownerName=owner.name;
    this.selection={experimentId:owner.id,incarnation:owner.incarnation,variantId:'',revision:owner.revision};
    this.current=current;this.returnFocus=source;this.message='';this.savingAllowed=savingAllowed;this.capturedSelectionId=capturedSelectionId;
    if(mode==='save')this.startSave();else {this.listError='';this.$flushUpdates();this.listDialog.showModal();void this.loadList();}
  }
  private activeElement():HTMLElement|null {
    let active:Element|null=document.activeElement;
    while(active?.shadowRoot?.activeElement)active=active.shadowRoot.activeElement;
    return active instanceof HTMLElement?active:null;
  }
  private restoreFocus():void {if(!this.closingAll&&this.returnFocus?.isConnected&&this.returnFocus.getClientRects().length)this.returnFocus.focus({preventScroll:true});}
  private validOwner():boolean {return !!this.owner&&this.current()&&this.isConnected;}
  private ownsDemo(demo:SavedDemo):boolean {return this.validOwner()&&demo.ownerId===this.owner?.id&&demo.ownerIncarnation===this.owner?.incarnation;}
  private currentDemo(id:string):SavedDemo|undefined {
    if(this.loading||this.listError)return;
    return this.rows.find(row=>row.id===id&&this.ownsDemo(row));
  }
  async loadList():Promise<boolean> {
    if(!this.owner)return false;const owner=this.owner.id,epoch=++this.listEpoch;this.loading=true;this.listError='';
    try {const demos=await invoke<SavedDemo[]>('saved_demos',{ownerId:owner});if(epoch!==this.listEpoch||!this.validOwner())return false;
      this.rows=demos.map(demo=>({...demo,modified:new Date(demo.modifiedAt).toLocaleString(),summary:`${demo.available} available responses · ${demo.skipped} unavailable · ${size(demo.bytes)}`}));
      return true;
    }catch(error){if(epoch===this.listEpoch)this.listError=describeError(error);return false;}
    finally{if(epoch===this.listEpoch)this.loading=false;}
  }
  startSave(id=''):void {
    if(!this.validOwner()||this.busy||!this.savingAllowed)return;
    const replace=id?this.currentDemo(id):undefined;
    if(id&&!replace)return;
    this.replace=replace??null;this.replacingName=replace?.name??'';this.name=replace?.name??'';
    this.selectedId='';this.scope='original-trace';this.pages=[];this.pagesMore=false;this.saveError='';this.review=null;this.reviewSummary='';this.missingRows=[];
    const epoch=++this.saveEpoch;this.pagesLoading=true;this.$flushUpdates();this.nameInput.value=this.name;this.saveDialog.showModal();this.nameInput.focus();
    void this.loadPages(epoch);
  }
  private async loadPages(epoch:number):Promise<void> {
    const owner=this.owner?.id;if(!owner)return;
    try {const result=await invoke<ExperimentPreviewPages>('experiment_preview_pages',{id:owner,selectedId:this.capturedSelectionId||null,includePages:true});
      if(epoch!==this.saveEpoch||!this.saveDialog.open||!this.validOwner())return;
      this.pages=result.pages.map(page=>({...page,identity:pageIdentity(page)}));this.pagesMore=result.more;
    }catch(error){if(epoch===this.saveEpoch)this.saveError='Captured pages could not be read: '+describeError(error);}
    finally{if(epoch===this.saveEpoch)this.pagesLoading=false;}
  }
  nameChangedByUser(event:Event):void {this.name=(event.currentTarget as HTMLInputElement).value;}
  selectPage(id:string):void {if(!this.busy&&!this.review)this.selectedId=id;}
  changeScope(event:Event):void {if(!this.review)this.scope=(event.currentTarget as HTMLSelectElement).value;}
  async prepare(event:Event):Promise<void> {
    event.preventDefault();if(this.busy||!this.validOwner()||!this.selection||!this.owner)return;
    const name=this.name.trim();if(!name||Array.from(name).length>160||/[\u0000-\u001f\u007f]/.test(name)){this.saveError='Enter a demo name of 1–160 characters.';this.nameInput.focus();return;}
    if(!this.pages.some(page=>page.entryId===this.selectedId)){this.saveError='Choose a complete captured HTML page.';return;}
    const epoch=this.saveEpoch,operation=crypto.randomUUID(),focused=this.activeElement();this.operation=operation;this.busy=true;this.saveError='';
    try {const prepared=await invoke<SavedDemoPreparation>('prepare_demo_save',{operationId:operation,input:{ownerId:this.owner.id,name,entryId:this.selectedId,
      options:{variant:this.selection,scope:this.scope},replace:this.replace?{id:this.replace.id,revision:this.replace.revision}:null}});
      if(epoch!==this.saveEpoch||!this.saveDialog.open||!this.validOwner()){void invoke('cancel_demo_save',{token:prepared.token}).catch(()=>{});return;}
      const ownsFocus=this.activeElement()===focused;
      this.review=prepared;this.reviewSummary=`${prepared.report.available} available responses · ${prepared.report.skipped} unavailable · ${size(prepared.report.bytes)}. Response changes and linked-file bytes are frozen for this save.`;
      this.missingRows=prepared.report.resources.filter(row=>row.bytes===null).map((row,index)=>({id:`${row.entryId}-${index}`,method:row.method,url:row.url,reason:row.decision}));
      this.omittedMissing=Math.max(0,prepared.report.skipped-this.missingRows.length);this.busy=false;this.$flushUpdates();if(ownsFocus)this.commitButton.focus({preventScroll:true});
    }catch(error){if(epoch===this.saveEpoch)this.saveError=describeError(error);}
    finally{if(epoch===this.saveEpoch){this.busy=false;this.operation='';}}
  }
  changeChoices():void {
    if(this.busy)return;const token=this.review?.token;this.review=null;this.saveError='';if(token)void invoke('cancel_demo_save',{token}).catch(()=>{});
    this.$flushUpdates();this.nameInput.focus();
  }
  async commit():Promise<void> {
    const review=this.review;if(this.busy||!review||!this.validOwner())return;
    const epoch=this.saveEpoch,operation=crypto.randomUUID();this.operation=operation;this.busy=true;this.saveError='';
    try {const demo=await invoke<SavedDemo>('commit_demo_save',{token:review.token,reviewedMissing:true,operationId:operation});
      this.$emit('demo-saved',{ownerId:demo.ownerId,demo});this.message=`Saved ${demo.name}.`;
      if(epoch===this.saveEpoch&&this.saveDialog.open){this.review=null;this.saveDialog.close();this.saveEpoch++;if(this.listDialog.open)void this.loadList();else this.restoreFocus();}
    }catch(error){if(epoch===this.saveEpoch)this.saveError=describeError(error);else if(!describeError(error).includes('canceled'))this.showNotice('Demo save failed',describeError(error),null,null);}
    finally{if(epoch===this.saveEpoch||this.operation===operation){this.busy=false;this.operation='';}}
  }
  cancelSave(event?:Event):void {
    event?.preventDefault();this.saveEpoch++;const token=this.review?.token;this.review=null;
    if(this.operation)void invoke('cancel_demo_operation',{operationId:this.operation}).catch(()=>{});
    if(token)void invoke('cancel_demo_save',{token}).catch(()=>{});
    this.operation='';this.busy=false;this.pagesLoading=false;this.saveDialog?.close();if(!this.listDialog?.open)this.restoreFocus();
  }
  saveClosed():void {if(!this.saveDialog.open&&(this.review||this.operation))this.cancelSave();}
  openDemo(id:string):void {
    const demo=this.currentDemo(id);if(!demo||demo.availabilityError||this.busy)return;
    const current=this.current;this.closeList();this.$emit('saved-demo-preview',{ownerId:demo.ownerId,demo,isCurrent:current} satisfies SavedDemoPreviewIntent);
  }
  askDelete(id:string):void {
    const demo=this.currentDemo(id);if(!demo)return;
    this.deleting=demo;this.deleteName=demo.name;this.deleteError='';this.deleteCompleted=false;this.deleteEpoch++;this.$flushUpdates();this.deleteDialog.showModal();
  }
  async confirmDelete():Promise<void> {
    const demo=this.deleting;if(!demo||this.deletePending||this.deleteCompleted||!this.ownsDemo(demo))return;const epoch=this.deleteEpoch,source=this.activeElement();this.deletePending=true;this.deleteError='';
    try {await invoke('delete_saved_demo',{ownerId:demo.ownerId,demo:{id:demo.id,revision:demo.revision}});
      this.message=`Deleted ${demo.name}.`;const current=epoch===this.deleteEpoch&&this.deleteDialog.open&&this.validOwner(),ownsFocus=current&&this.activeElement()===source;
      if(current){this.deleteCompleted=true;if(ownsFocus)this.deleteDialog.close();}
      const focus=this.focusEpoch,refreshed=await this.loadList();this.$flushUpdates();
      if(ownsFocus&&refreshed&&epoch===this.deleteEpoch&&this.listDialog.open&&this.validOwner()&&focus===this.focusEpoch)this.listCloseButton.focus({preventScroll:true});
    }catch(error){if(epoch===this.deleteEpoch)this.deleteError=describeError(error);else this.showNotice('Demo deletion failed',describeError(error),null,null);}
    finally{this.deletePending=false;}
  }
  cancelDelete(event?:Event):void {event?.preventDefault();const restore=this.deleteCompleted&&this.deleteDialog?.contains(this.activeElement());this.deleteEpoch++;this.deleting=null;this.deleteDialog?.close();if(restore&&this.listDialog.open&&!this.closingAll)this.listCloseButton.focus({preventScroll:true});}
  closeList(event?:Event):void {event?.preventDefault();this.listEpoch++;this.loading=false;this.listDialog?.close();this.restoreFocus();}
  listClosed():void {if(!this.listDialog.open){this.listEpoch++;this.loading=false;}}
  closeAll():void {this.closingAll=true;this.cancelSave();this.cancelDelete();this.closeList();this.owner=null;this.selection=null;this.current=()=>false;this.closingAll=false;}
  disconnectedCallback():void {this.closeAll();super.disconnectedCallback();}
}
ResponseSetDemos.define('response-set-demos');
