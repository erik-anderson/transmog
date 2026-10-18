import {observable} from '@microsoft/webui-framework';
import {invoke} from '@tauri-apps/api/core';
import {WorkspaceElement} from '../workspace-element.js';
import '../response-set-tour/response-set-tour.js';
import type {ResponseSetTour} from '../response-set-tour/response-set-tour.js';
import {pageIdentity} from '../response-set-page-picker/response-set-page-picker.js';
import {describeError} from '../utilities.js';
import type {AutomationStatus,AutomationRule,ResponseExperiment,ExperimentSelection,VariantEdit,ResponseSetGuideHistory,ResponseSetGuideUpdate,ExperimentPreviewPages} from '../models.js';

const tourSteps=[
  {title:'Organize related responses',copy:'A response set groups related response changes. Variants hold alternatives; revisions are saved versions. Selecting one changes only what you’re viewing.'},
  {title:'Edit and save',copy:'Choose Edit response to make changes. Saving creates a new revision; it does not change the revision used for live traffic.'},
  {title:'Preview saved changes',copy:'Preview this revision in a captured page without changing live traffic. After saving more edits, open an updated preview; refreshing keeps the earlier snapshot.'},
  {title:'Use it for live traffic',copy:'Apply the selected revision to new matching requests. To remove this set’s live changes, select Baseline and apply it. Other automation and pause settings stay unchanged.'},
];

export class ExperimentVariants extends WorkspaceElement {
  @observable overviewVisible=false;
  @observable preferencesReady=false;
  @observable guideHistory:ResponseSetGuideHistory='not-offered';
  @observable guidePending=false;
  @observable guideError='';
  @observable invitationVisible=false;
  @observable helpOpen=false;
  @observable tourStep=0;
  @observable tourRuleId='';
  @observable tourContext='';
  @observable stepNumber=0;
  @observable stepTitle='';
  @observable stepCopy='';
  @observable stepLast=false;
  guideHelpButton!:HTMLButtonElement;
  guideStartButton!:HTMLButtonElement;
  guideHelpPanel!:HTMLDialogElement;
  tourPanel!:ResponseSetTour;
  private guideEpoch=0;
  private offerChecked=false;
  overviewVisibleChanged():void {if(!this.overviewVisible){this.leaveGuide();this.pageEpoch++;if(this.copyDialog?.open)this.closeCopy();}else {void this.offerGuide();void this.refreshPage();}}
  preferencesReadyChanged():void {void this.offerGuide();}
  guideHistoryChanged():void {void this.offerGuide();}
  variantBusyChanged():void {if(this.variantBusy)this.leaveGuide();else void this.offerGuide();}
  disconnectedCallback():void {this.leaveGuide();super.disconnectedCallback();}
  private guideAvailable():boolean {return this.isConnected&&this.overviewVisible&&this.preferencesReady&&!!this.experimentId&&!this.variantBusy;}
  private rememberGuide(update:ResponseSetGuideUpdate):void {
    this.guideHistory=update.history;
    this.$emit('workspace-change',{patch:{responseSetGuide:update.history},committed:false});
  }
  private async offerGuide():Promise<void> {
    if(this.offerChecked||this.guidePending||!this.guideAvailable())return;
    this.offerChecked=true;
    if(this.guideHistory!=='not-offered')return;
    const epoch=this.guideEpoch;this.guidePending=true;
    try {const update=await invoke<ResponseSetGuideUpdate>('update_response_set_guide',{action:'offer'});this.rememberGuide(update);if(epoch===this.guideEpoch&&this.guideAvailable())this.invitationVisible=update.offered;}
    catch(error){if(epoch===this.guideEpoch)this.guideError='Tour invitation could not be remembered: '+describeError(error);}
    finally{this.guidePending=false;}
  }
  showGuideHelp():void {
    if(!this.overviewVisible)return;
    this.guideEpoch++;this.tourStep=0;this.helpOpen=true;this.guideError='';this.$flushUpdates();
    this.guideHelpPanel.scrollIntoView({block:'nearest'});this.guideHelpPanel.focus({preventScroll:true});
  }
  closeGuideHelp():void {this.guideEpoch++;this.helpOpen=false;this.restoreGuideFocus();}
  dismissGuideInvitation():void {this.guideEpoch++;this.invitationVisible=false;this.restoreGuideFocus();}
  async startGuide():Promise<void> {
    if(this.guidePending||!this.guideAvailable())return;
    // Pending Start stays focusable (aria-disabled in the template): native
    // disabling blurs it and would invalidate its own focus ownership below.
    const epoch=++this.guideEpoch,selection=this.selectionEpoch,source=this.getRootNode() instanceof ShadowRoot?(this.getRootNode() as ShadowRoot).activeElement:document.activeElement;
    this.guidePending=true;this.guideError='';
    try {
      const update=await invoke<ResponseSetGuideUpdate>('update_response_set_guide',{action:'start'});this.rememberGuide(update);
      if(epoch!==this.guideEpoch||selection!==this.selectionEpoch||!this.guideAvailable())return;
      const root=this.getRootNode(),active=root instanceof ShadowRoot?root.activeElement:document.activeElement;
      // A delayed preference write must not remove help underneath newly moved focus.
      if(active!==source)return;
      this.helpOpen=false;this.invitationVisible=false;this.setTourStep(1);
      this.tourPanel.focusNext();
    }catch(error){if(epoch===this.guideEpoch)this.guideError='Tour could not start: '+describeError(error);}
    finally{this.guidePending=false;}
  }
  private setTourStep(step:number):void {
    this.tourStep=step;const copy=tourSteps[step-1]!;this.stepNumber=step;this.stepTitle=copy.title;this.stepCopy=copy.copy;this.stepLast=step===4;
    this.updateTourContext();this.$flushUpdates();this.tourPanel.scrollIntoView({block:'nearest'});
  }
  nextTourStep():void {if(this.tourStep===4)this.endGuide();else if(this.tourStep){this.setTourStep(this.tourStep+1);this.tourPanel.focusNext();}}
  previousTourStep():void {if(this.tourStep>1){this.setTourStep(this.tourStep-1);this.tourPanel.focusNext();}}
  endGuide():void {this.guideEpoch++;this.tourStep=0;this.restoreGuideFocus();}
  private leaveGuide():void {this.guideEpoch++;this.tourStep=0;this.helpOpen=false;this.invitationVisible=false;}
  private restoreGuideFocus():void {this.$flushUpdates();if(this.overviewVisible&&this.isConnected){this.guideHelpButton.scrollIntoView({block:'nearest'});this.guideHelpButton.focus({preventScroll:true});}}
  guideKeyboard(event:KeyboardEvent):void {
    if(event.key==='Escape'&&(this.tourStep||this.helpOpen)){event.preventDefault();event.stopPropagation();if(this.tourStep)this.endGuide();else this.closeGuideHelp();}
    else if(this.helpOpen&&this.guideHelpPanel.contains(event.target as Node)){event.stopPropagation();if(event.key==='Delete'||((event.ctrlKey||event.metaKey)&&['a','z','y','s'].includes(event.key.toLowerCase())))event.preventDefault();}
  }
  private updateTourContext():void {
    this.tourRuleId=this.variantRules[0]?.id??'';
    this.tourContext=this.tourStep===2&&(!this.variantRules.length||!this.variantLatest||this.variantArchived||this.experimentArchived)?'No editable response is available in this selection. You can finish the tour without making changes.':'';
  }
  @observable status:AutomationStatus|null=null;
  @observable experimentItems:Array<{id:string;name:string}>=[];
  @observable variantItems:Array<{id:string;name:string}>=[];
  @observable revisionItems:number[]=[];
  @observable variantRules:Array<{id:string;name:string;enabled:boolean;asset:string}>=[];
  @observable experimentId='';
  @observable variantId='';
  @observable variantRevision=0;
  @observable variantBusy=false;
  @observable variantError='';
  @observable variantMessage='';
  @observable variantLiveText='';
  @observable variantViewingText='';
  @observable variantPaused=false;
  @observable variantArchived=false;
  @observable experimentArchived=false;
  @observable variantLatest=false;
  @observable variantUndoAvailable=false;
  @observable activationOutcome='';
  @observable activationRestore='';
  @observable previewReportAvailable=false;
  @observable previewReportSetId='';
  @observable copyPending=false;
  @observable copyCompleted=false;
  @observable copyError='';
  @observable copySource='';
  copyDialog!:HTMLDialogElement;
  copyName!:HTMLInputElement;
  copyButton!:HTMLButtonElement;
  copyCreateButton!:HTMLButtonElement;
  copyCloseButton!:HTMLButtonElement;
  activationButton!:HTMLButtonElement;
  undoButton!:HTMLButtonElement;
  private copySelection:ExperimentSelection|null=null;
  private copyEpoch=0;
  private copySourceKey='';
  @observable variantWarnings:string[]=[];
  @observable variantReviewed=false;
  @observable pageUrl='';
  @observable pageIdentity='';
  @observable pageAvailability='';
  private pageEpoch=0;
  async refreshPage():Promise<void> {
    const id=this.experimentId,page=this.experiment()?.previewPage,epoch=++this.pageEpoch;
    this.pageUrl=page?.url??'';this.pageIdentity=page?pageIdentity(page):'';this.pageAvailability=page?'Checking capture…':'';
    if(!page||!this.overviewVisible)return;
    try {const result=await invoke<ExperimentPreviewPages>('experiment_preview_pages',{id,selectedId:null,includePages:false});
      if(epoch===this.pageEpoch&&id===this.experimentId)this.pageAvailability=result.available?(result.sourceLabel?'Retained preview capture':'Captured response available'):'Capture unavailable';
    }catch{if(epoch===this.pageEpoch)this.pageAvailability='Capture availability could not be checked';}
  }
  changePage():void {
    if(this.tourStep||this.variantBusy)return;
    const id=this.experimentId,current=this.previewContext();
    this.$emit('page-association-request',{id,isCurrent:()=>id===this.experimentId&&current()});
  }
  experimentSelect!:HTMLSelectElement;
  newExperimentButton!:HTMLButtonElement;
  variantSelect!:HTMLSelectElement;
  variantName!:HTMLInputElement;
  experimentName!:HTMLInputElement;
  private reviewGeneration=-1;
  private selectionEpoch=0;
  private renderedExperiment='';
  private renderedVariant='';
  private renderedExperimentName='';
  private renderedVariantName='';

  statusChanged():void {if(this.reviewGeneration>=0&&this.reviewGeneration!==this.status?.generation){this.variantWarnings=[];this.variantReviewed=false;this.reviewGeneration=-1;}this.render();}
  private experiment():ResponseExperiment|undefined {return this.status?.experiments?.find(e=>e.id===this.experimentId);}
  private selection():ExperimentSelection {return {experimentId:this.experimentId,variantId:this.variantId,revision:this.variantRevision};}
  previewContext():()=>boolean {
    const epoch=this.selectionEpoch,selection=JSON.stringify(this.selection()),generation=this.status?.generation;
    return ()=>this.overviewVisible&&!this.tourStep&&!this.variantBusy&&epoch===this.selectionEpoch&&selection===JSON.stringify(this.selection())&&generation===this.status?.generation;
  }
  private rules():AutomationRule[] {return this.experiment()?.variants.find(v=>v.id===this.variantId)?.revisions.find(r=>r.revision===this.variantRevision)?.rules??[];}
  private clearReview():void {this.selectionEpoch++;this.variantWarnings=[];this.variantReviewed=false;this.reviewGeneration=-1;this.variantError='';}
  private render():void {
    const root=this.getRootNode(),focused=root instanceof ShadowRoot?root.activeElement:document.activeElement;
    const tourHadFocus=!!this.tourStep&&!!focused&&!!this.tourPanel?.contains(focused);
    const experiments=this.status?.experiments??[];
    this.experimentItems=experiments.map(e=>({id:e.id,name:e.name+(e.archived?' · Archived':'')}));
    if(!experiments.some(e=>e.id===this.experimentId)){this.experimentId=experiments[0]?.id??'';this.variantId='';}
    const experiment=this.experiment(),variants=experiment?.variants??[];
    if(!variants.some(v=>v.id===this.variantId)){this.variantId=variants[0]?.id??'';this.variantRevision=0;}
    const variant=variants.find(v=>v.id===this.variantId);
    this.variantItems=variants.map(v=>({id:v.id,name:v.name+(v.archived?' · Archived':'')}));
    this.revisionItems=variant?.revisions.map(r=>r.revision).reverse()??[];
    if(!this.revisionItems.includes(this.variantRevision))this.variantRevision=this.revisionItems[0]??0;
    this.variantRules=this.rules().map(r=>({id:r.id,name:r.displayName??r.id,enabled:r.enabled!==false,asset:r.request.responseAsset??''}));
    this.variantLatest=this.variantRevision===this.revisionItems[0];this.variantPaused=experiment?.paused??false;
    this.variantArchived=variant?.archived??false;this.experimentArchived=experiment?.archived??false;
    this.variantUndoAvailable=experiment?.undo?.generation===this.status?.generation;
    const restore=experiment?.undo?.selection,restoreName=variants.find(v=>v.id===restore?.variantId)?.name??restore?.variantId;
    this.activationRestore=restore&&restore.variantId!=='baseline'?`Restores ${restoreName} · revision ${restore.revision}.`:'Restores no changes from this set.';
    const active=experiment?.active,activeName=active?variants.find(v=>v.id===active.variantId)?.name??active.variantId:'Baseline';
    this.variantViewingText=`${variant?.name??'Baseline'} · revision ${this.variantRevision}`;
    const live=active&&active.variantId!=='baseline'?`${activeName} · revision ${active.revision}`:'No changes from this set';
    this.variantLiveText=`${live}${experiment?.paused?' · Response set paused':''}${this.status?.autoresponsesEnabled===false?' · Saved responses paused':''}.`;
    this.activationOutcome=`Last activation: ${live}.`;
    if(!this.experimentId)this.leaveGuide();
    this.updateTourContext();
    this.$flushUpdates();
    if(this.experimentName&&(this.renderedExperiment!==this.experimentId||this.experimentName.value===this.renderedExperimentName))this.experimentName.value=experiment?.name??'';
    if(this.variantName&&(this.renderedExperiment!==this.experimentId||this.renderedVariant!==this.variantId||this.variantName.value===this.renderedVariantName))this.variantName.value=variant?.name??'';
    this.renderedExperiment=this.experimentId;this.renderedVariant=this.variantId;this.renderedExperimentName=experiment?.name??'';this.renderedVariantName=variant?.name??'';
    if(tourHadFocus&&!focused?.isConnected&&this.tourStep&&this.overviewVisible)this.tourPanel.focusNext();
    void this.offerGuide();
    void this.refreshPage();
  }
  async reveal(selection:ExperimentSelection,focus=true):Promise<void> {this.experimentId=selection.experimentId;this.variantId=selection.variantId;this.variantRevision=selection.revision;this.variantMessage='';this.clearReview();this.render();if(focus)this.experimentSelect.focus();}
  selectExperiment(event:Event):void {this.experimentId=(event.currentTarget as HTMLSelectElement).value;this.variantId='';this.variantRevision=0;this.clearReview();this.render();}
  selectVariant(event:Event):void {this.variantId=(event.currentTarget as HTMLSelectElement).value;this.variantRevision=0;this.clearReview();this.render();}
  selectRevision(event:Event):void {this.variantRevision=Number((event.currentTarget as HTMLSelectElement).value);this.clearReview();this.render();}
  editRule(id:string):void {if(!this.tourStep)this.$emit('variant-edit',{selection:this.selection(),ruleId:id} satisfies VariantEdit);}
  addResponse():void {if(!this.tourStep)this.$emit('variant-edit',{selection:this.selection(),ruleId:null} satisfies VariantEdit);}
  newExperiment():void {if(!this.tourStep)this.$emit('variant-new',undefined);}
  previewVariant():void {if(!this.tourStep)this.$emit('variant-preview',{selection:this.selection(),ruleId:this.rules().find(r=>r.enabled)?.id??null} satisfies VariantEdit);}
  private async change(change:Record<string,unknown>,message:string,next?:ExperimentSelection):Promise<void> {
    if(this.tourStep||this.variantBusy||!this.status)return;
    this.variantBusy=true;this.variantError='';const epoch=this.selectionEpoch;
    try {
      const status=await invoke<AutomationStatus>('change_experiment',{generation:this.status.generation,change});
      this.$emit('variant-status',status);
      if(epoch!==this.selectionEpoch)return;
      if(this.status&&status.generation<this.status.generation){this.variantMessage='The operation completed; newer automation state is shown.';return;}
      this.status=status;if(next){this.experimentId=next.experimentId;this.variantId=next.variantId;this.variantRevision=next.revision;}
      this.variantMessage=message;this.clearReview();this.render();
    }catch(error:unknown){if(epoch===this.selectionEpoch)this.variantError=describeError(error);}
    finally{this.variantBusy=false;}
  }
  showPreviewDiagnostics():void {this.$emit('preview-report-request',undefined);}
  duplicate():void {
    if(this.copyPending||this.variantBusy||this.tourStep||!this.overviewVisible)return;
    const selection=this.selection(),variant=this.experiment()?.variants.find(v=>v.id===selection.variantId);if(!variant)return;
    const key=JSON.stringify(selection),keepName=this.copySourceKey===key&&!!this.copyError;
    this.copySelection=selection;this.copySourceKey=key;this.copySource=`${variant.name} · revision ${selection.revision}`;this.copyEpoch++;this.copyCompleted=false;
    if(!keepName){let prefix='',bytes=0;const encoder=new TextEncoder();for(const char of variant.name){const size=encoder.encode(char).length;if(bytes+size>123)break;prefix+=char;bytes+=size;}this.copyName.value=prefix+' copy';this.copyError='';}
    this.$flushUpdates();this.copyDialog.showModal();this.copyName.focus();this.copyName.select();
  }
  closeCopy(event?:Event):void {event?.preventDefault();this.copyEpoch++;this.copyDialog.close();if(this.overviewVisible&&this.copyButton?.isConnected)this.copyButton.focus({preventScroll:true});}
  copyPointerDown(event:PointerEvent):void {if(this.copyCompleted)event.preventDefault();}
  async createCopy(event:Event):Promise<void> {
    event.preventDefault();if(this.copyPending||this.copyCompleted||!this.status||!this.copySelection)return;
    const name=this.copyName.value.trim();if(!name){this.copyError='Enter a name for the new variant.';this.copyName.focus();return;}
    if(new TextEncoder().encode(name).length>128){this.copyError='The variant name is too long. Shorten it and try again.';this.copyName.focus();return;}
    const epoch=this.copyEpoch,selection=this.copySelection,selectionEpoch=this.selectionEpoch,variantId='v-'+crypto.randomUUID().replaceAll('-','');
    const active=(this.getRootNode() as ShadowRoot).activeElement;
    this.copyPending=true;this.copyError='';
    try {
      const status=await invoke<AutomationStatus>('change_experiment',{generation:this.status.generation,change:{action:'duplicate',selection,variantId,name}});
      this.$emit('variant-status',status);
      if(status.generation>=(this.status?.generation??0))this.status=status;
      if(epoch===this.copyEpoch&&this.copyDialog.open){
        if(active!==this.copyCreateButton&&(this.getRootNode() as ShadowRoot).activeElement===this.copyCreateButton)this.copyCloseButton.focus({preventScroll:true});
        this.copyCompleted=true;
      }
      const current=epoch===this.copyEpoch&&selectionEpoch===this.selectionEpoch&&this.overviewVisible&&this.copyDialog.open;
      if(current&&(this.getRootNode() as ShadowRoot).activeElement===active){
        this.experimentId=selection.experimentId;this.variantId=variantId;this.variantRevision=1;this.clearReview();this.render();this.variantMessage='Variant copy created. Live traffic is unchanged.';this.closeCopy();
      }else if(this.overviewVisible&&this.experimentId===selection.experimentId)this.variantMessage=`Variant copy “${name}” created. Live traffic is unchanged.`;
      else this.$emit('notice',{title:'Variant copy created',message:`${name}. Live traffic is unchanged.`,actionLabel:null,action:null});
    }catch(error){this.copyError=describeError(error);if(epoch!==this.copyEpoch||!this.copyDialog.open)this.$emit('notice',{title:'Variant copy failed',message:describeError(error),actionLabel:null,action:null});}
    finally{this.copyPending=false;}
  }
  rename(variant:boolean):void {void this.change({action:'rename',id:this.experimentId,variantId:variant?this.variantId:null,name:(variant?this.variantName:this.experimentName).value.trim()},'Name saved.');}
  archive(variant:boolean):void {void this.change({action:'archive',id:this.experimentId,variantId:variant?this.variantId:null,archived:variant?!this.variantArchived:!this.experimentArchived},'Archive state saved. Restore reverses this action.');}
  pause():void {void this.change({action:'pause',id:this.experimentId,paused:!this.variantPaused},this.variantPaused?'Response set resumed.':'Response set paused. Authored rules are unchanged.');}
  async undoActivation():Promise<void> {
    if(this.variantBusy)return;
    const root=this.getRootNode() as ShadowRoot,owned=root.activeElement===this.undoButton,id=this.experimentId;
    await this.change({action:'undo-activation',id},'Previous activation restored.');
    this.$flushUpdates();
    if(owned&&this.overviewVisible&&id===this.experimentId&&!this.variantUndoAvailable&&(!root.activeElement||root.activeElement===this.undoButton))this.activationButton.focus({preventScroll:true});
  }
  async positionVariant(before:boolean):Promise<void> {
    if(!this.status||!this.variantLatest||!this.rules().length)return;
    const owned=new Set(this.experiment()?.variants.flatMap(v=>v.revisions.flatMap(r=>r.rules.map(r=>r.id)))??[]);
    const positions=this.status.rules.filter(r=>!owned.has(r.id)).map(r=>r.priority),rules=this.rules();
    const first=before?Math.min(0,...positions)-rules.length:Math.max(0,...positions)+1;
    if(first<-2147483648||first+rules.length-1>2147483647){this.variantError='No rule position is available on that side. Activate Baseline on the conflicting response set, or change ordinary rule order.';return;}
    await this.change({action:'save',id:this.experimentId,variantId:this.variantId,expectedRevision:this.variantRevision,rules:rules.map((r,i)=>r.priority===first+i?r:{...r,priority:first+i,revision:r.revision+1})},`Revision ${this.variantRevision+1} saved. Live traffic is unchanged.`,{...this.selection(),revision:this.variantRevision+1});
  }
  async activate():Promise<void> {
    if(this.tourStep||this.variantBusy||!this.status)return;
    if(!this.variantReviewed||this.reviewGeneration!==this.status.generation){
      this.variantBusy=true;this.variantError='';const epoch=this.selectionEpoch,generation=this.status.generation;
      try {const warnings=await invoke<string[]>('experiment_overlaps',{selection:this.selection()});if(epoch!==this.selectionEpoch||generation!==this.status?.generation)return;this.variantWarnings=warnings;this.variantReviewed=true;this.reviewGeneration=generation;if(warnings.length)return;}
      catch(error:unknown){if(epoch===this.selectionEpoch)this.variantError=describeError(error);return;}
      finally{this.variantBusy=false;}
    }
    await this.change({action:'activate',selection:this.selection(),acknowledgeOverlaps:this.variantWarnings.length>0},'');
  }
  async changeRule(id:string,action:string):Promise<void> {
    if(!this.variantLatest||this.variantId==='baseline')return;
    let rules=structuredClone(this.rules());const index=rules.findIndex(r=>r.id===id);if(index<0)return;
    if(action==='remove')rules=rules.filter(r=>r.id!==id);
    else if(action==='toggle'){rules[index]!.enabled=!rules[index]!.enabled;rules[index]!.revision++;}
    else {const next=index+(action==='up'?-1:1);if(next<0||next>=rules.length)return;[rules[index],rules[next]]=[rules[next]!,rules[index]!];const positions=this.rules().map(r=>r.priority).sort((a,b)=>a-b);rules=rules.map((r,i)=>r.priority===positions[i]?r:{...r,priority:positions[i]!,revision:r.revision+1});}
    await this.change({action:'save',id:this.experimentId,variantId:this.variantId,expectedRevision:this.variantRevision,rules},`Revision ${this.variantRevision+1} saved. Live traffic is unchanged.`,{...this.selection(),revision:this.variantRevision+1});
  }
}
ExperimentVariants.define('experiment-variants');
