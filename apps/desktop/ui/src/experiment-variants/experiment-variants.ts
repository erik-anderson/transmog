import {observable} from '@microsoft/webui-framework';
import {invoke} from '@tauri-apps/api/core';
import {WorkspaceElement} from '../workspace-element.js';
import {describeError} from '../utilities.js';
import type {AutomationStatus,AutomationRule,ResponseExperiment,ExperimentSelection,VariantEdit} from '../models.js';

export class ExperimentVariants extends WorkspaceElement {
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
  @observable variantPaused=false;
  @observable variantArchived=false;
  @observable experimentArchived=false;
  @observable variantLatest=false;
  @observable variantUndoAvailable=false;
  @observable variantWarnings:string[]=[];
  @observable variantReviewed=false;
  experimentSelect!:HTMLSelectElement;
  variantSelect!:HTMLSelectElement;
  variantName!:HTMLInputElement;
  experimentName!:HTMLInputElement;
  duplicateName!:HTMLInputElement;
  private reviewGeneration=-1;
  private selectionEpoch=0;
  private renderedExperiment='';
  private renderedVariant='';
  private renderedExperimentName='';
  private renderedVariantName='';

  statusChanged():void {if(this.reviewGeneration>=0&&this.reviewGeneration!==this.status?.generation){this.variantWarnings=[];this.variantReviewed=false;this.reviewGeneration=-1;}this.render();}
  private experiment():ResponseExperiment|undefined {return this.status?.experiments?.find(e=>e.id===this.experimentId);}
  private selection():ExperimentSelection {return {experimentId:this.experimentId,variantId:this.variantId,revision:this.variantRevision};}
  private rules():AutomationRule[] {return this.experiment()?.variants.find(v=>v.id===this.variantId)?.revisions.find(r=>r.revision===this.variantRevision)?.rules??[];}
  private clearReview():void {this.selectionEpoch++;this.variantWarnings=[];this.variantReviewed=false;this.reviewGeneration=-1;this.variantError='';}
  private render():void {
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
    const active=experiment?.active,activeName=active?variants.find(v=>v.id===active.variantId)?.name??active.variantId:'Baseline';
    this.variantLiveText=`${experiment?.paused?'Experiment paused':this.status?.autoresponsesEnabled===false?'All auto-responses paused':'Live'} · ${activeName}${active?' · revision '+active.revision:''}${experiment?.paused&&this.status?.autoresponsesEnabled===false?' · All auto-responses paused':''}. Selecting or saving a variant does not change live traffic.`;
    this.$flushUpdates();
    if(this.experimentName&&(this.renderedExperiment!==this.experimentId||this.experimentName.value===this.renderedExperimentName))this.experimentName.value=experiment?.name??'';
    if(this.variantName&&(this.renderedExperiment!==this.experimentId||this.renderedVariant!==this.variantId||this.variantName.value===this.renderedVariantName))this.variantName.value=variant?.name??'';
    this.renderedExperiment=this.experimentId;this.renderedVariant=this.variantId;this.renderedExperimentName=experiment?.name??'';this.renderedVariantName=variant?.name??'';
  }
  async reveal(selection:ExperimentSelection):Promise<void> {this.experimentId=selection.experimentId;this.variantId=selection.variantId;this.variantRevision=selection.revision;this.clearReview();this.render();this.experimentSelect.focus();}
  selectExperiment(event:Event):void {this.experimentId=(event.currentTarget as HTMLSelectElement).value;this.variantId='';this.variantRevision=0;this.clearReview();this.render();}
  selectVariant(event:Event):void {this.variantId=(event.currentTarget as HTMLSelectElement).value;this.variantRevision=0;this.clearReview();this.render();}
  selectRevision(event:Event):void {this.variantRevision=Number((event.currentTarget as HTMLSelectElement).value);this.clearReview();this.render();}
  editRule(id:string):void {this.$emit('variant-edit',{selection:this.selection(),ruleId:id} satisfies VariantEdit);}
  addResponse():void {this.$emit('variant-edit',{selection:this.selection(),ruleId:null} satisfies VariantEdit);}
  newExperiment():void {this.$emit('variant-new',undefined);}
  previewVariant():void {this.$emit('variant-preview',{selection:this.selection(),ruleId:this.rules().find(r=>r.enabled)?.id??null} satisfies VariantEdit);}
  private async change(change:Record<string,unknown>,message:string,next?:ExperimentSelection):Promise<void> {
    if(this.variantBusy||!this.status)return;
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
  duplicate():void {const name=this.duplicateName.value.trim();if(!name){this.variantError='Enter a name for the duplicate.';this.duplicateName.focus();return;}const variantId='v-'+crypto.randomUUID().replaceAll('-','');void this.change({action:'duplicate',selection:this.selection(),variantId,name},'Variant duplicated. Live traffic is unchanged.',{experimentId:this.experimentId,variantId,revision:1});}
  rename(variant:boolean):void {void this.change({action:'rename',id:this.experimentId,variantId:variant?this.variantId:null,name:(variant?this.variantName:this.experimentName).value.trim()},'Name saved.');}
  archive(variant:boolean):void {void this.change({action:'archive',id:this.experimentId,variantId:variant?this.variantId:null,archived:variant?!this.variantArchived:!this.experimentArchived},'Archive state saved. Restore reverses this action.');}
  pause():void {void this.change({action:'pause',id:this.experimentId,paused:!this.variantPaused},this.variantPaused?'Experiment resumed.':'Experiment paused. Authored rules are unchanged.');}
  undoActivation():void {void this.change({action:'undo-activation',id:this.experimentId},'Previous activation restored.');}
  async positionVariant(before:boolean):Promise<void> {
    if(!this.status||!this.variantLatest||!this.rules().length)return;
    const owned=new Set(this.experiment()?.variants.flatMap(v=>v.revisions.flatMap(r=>r.rules.map(r=>r.id)))??[]);
    const positions=this.status.rules.filter(r=>!owned.has(r.id)).map(r=>r.priority),rules=this.rules();
    const first=before?Math.min(0,...positions)-rules.length:Math.max(0,...positions)+1;
    if(first<-2147483648||first+rules.length-1>2147483647){this.variantError='No rule position is available on that side. Activate Baseline on the conflicting experiment, or change ordinary rule order.';return;}
    await this.change({action:'save',id:this.experimentId,variantId:this.variantId,expectedRevision:this.variantRevision,rules:rules.map((r,i)=>r.priority===first+i?r:{...r,priority:first+i,revision:r.revision+1})},'New revision saved with the selected live position. Activate it explicitly.',{...this.selection(),revision:this.variantRevision+1});
  }
  async activate():Promise<void> {
    if(this.variantBusy||!this.status)return;
    if(!this.variantReviewed||this.reviewGeneration!==this.status.generation){
      this.variantBusy=true;this.variantError='';const epoch=this.selectionEpoch,generation=this.status.generation;
      try {const warnings=await invoke<string[]>('experiment_overlaps',{selection:this.selection()});if(epoch!==this.selectionEpoch||generation!==this.status?.generation)return;this.variantWarnings=warnings;this.variantReviewed=true;this.reviewGeneration=generation;if(warnings.length)return;}
      catch(error:unknown){if(epoch===this.selectionEpoch)this.variantError=describeError(error);return;}
      finally{this.variantBusy=false;}
    }
    await this.change({action:'activate',selection:this.selection(),acknowledgeOverlaps:this.variantWarnings.length>0},'Saved revision activated for new live requests. Pause settings are preserved.');
  }
  async changeRule(id:string,action:string):Promise<void> {
    if(!this.variantLatest||this.variantId==='baseline')return;
    let rules=structuredClone(this.rules());const index=rules.findIndex(r=>r.id===id);if(index<0)return;
    if(action==='remove')rules=rules.filter(r=>r.id!==id);
    else if(action==='toggle'){rules[index]!.enabled=!rules[index]!.enabled;rules[index]!.revision++;}
    else {const next=index+(action==='up'?-1:1);if(next<0||next>=rules.length)return;[rules[index],rules[next]]=[rules[next]!,rules[index]!];const positions=this.rules().map(r=>r.priority).sort((a,b)=>a-b);rules=rules.map((r,i)=>r.priority===positions[i]?r:{...r,priority:positions[i]!,revision:r.revision+1});}
    await this.change({action:'save',id:this.experimentId,variantId:this.variantId,expectedRevision:this.variantRevision,rules},'New variant revision saved. Live traffic is unchanged.',{...this.selection(),revision:this.variantRevision+1});
  }
}
ExperimentVariants.define('experiment-variants');
