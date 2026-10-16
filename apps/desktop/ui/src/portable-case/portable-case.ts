import {attr, observable} from '@microsoft/webui-framework';
import {invoke} from '@tauri-apps/api/core';
import type {AutomationStatus} from '../models.js';
import {WorkspaceElement} from '../workspace-element.js';
import {describeError} from '../utilities.js';

type Selection={experimentId:string;variantId:string;revision:number};
type Scope={value:string;label:string;scope:unknown};
type Reference={artifactSha256:string;sourceKey:string;exchangeId:string|null;occurrence:string};
type Source={id:string;label:string;artifactSha256?:string;sourceKey?:string;dependency:{state:string;reason?:string};evidenceCompleteness:string};
type Context={id:string;kind:string;completeness:string;content:{state:string;sha256?:string}};
type Review={encrypted?:boolean;traffic?:{entries:number;retainedEntries:number;streamRecords:number;incompleteBodies:number}|null;manifest:{id:string;kind:string;title:string;sources?:Source[];rules?:unknown[];assets?:unknown[];experiments?:Array<{id:string;label?:string}>;notes?:Array<{id:string;text:string;source?:Reference|null}>;contexts?:Context[];markers?:unknown[];runs?:unknown[];objects?:Array<{role:string;bytes:string;sha256?:string;mediaType?:string}>};inventory:{missingDependencies:number;incompleteSources:number;experiments:Array<{id:string;state:string;reason:string|null}>};excluded:string[];activation:string};
type RuleReview={items:Array<{kind:string;portableId:string;state:string;localId:string}>;activation:string};
export class PortableCase extends WorkspaceElement {
  @attr({mode:'boolean'}) viewerMode=false;
  @observable caseStage='author';
  @observable caseFormat='native';
  @observable caseEncrypt=false;
  @observable caseBusy=false;
  @observable caseStatus='';
  @observable caseContexts:Array<Context&{available:boolean}>=[];
  @observable caseContextText='';
  @observable caseContextLabel='';
  @observable caseTitle='Portable investigation';
  @observable caseScopes:Scope[]=[];
  @observable caseRules:Array<{id:string;label:string}>=[];
  @observable caseVariants:Array<{value:string;label:string;selection:Selection}>=[];
  @observable caseSources:Array<Source&{available:boolean}>=[];
  @observable caseExperiments:Array<{id:string;label:string;state:string;reason:string;available:boolean}>=[];
  @observable caseNotes:Array<{id:string;text:string;source?:Reference|null}>=[];
  @observable caseExcluded:Array<{text:string}>=[];
  @observable caseFacts:Array<{label:string;value:string}>=[];
  @observable caseManifestText='';
  @observable caseImportItems:RuleReview['items']=[];
  @observable caseOrdinaryRules=false;
  @observable caseImportAvailable=false;
  @observable caseIsCase=true;
  @observable caseInspection=false;
  caseDialog!:HTMLDialogElement;
  authorForm!:HTMLFormElement;
  saveForm!:HTMLFormElement;
  importForm!:HTMLFormElement;
  workingConsent!:HTMLInputElement;
  private scopes:Scope[]=[];
  private returnFocus:HTMLElement|null=null;
  private connectedRevision=0;
  private openedReview:Review|null=null;
  private links=new Map<string,string>();
  private referenceKey(ref:Reference):string{return JSON.stringify([ref.artifactSha256,ref.sourceKey,ref.exchangeId,ref.occurrence]);}
  disconnectedCallback():void {this.connectedRevision++;super.disconnectedCallback();void invoke('close_case').catch(()=>undefined);}
  async show(mode:'export'|'open',scopes:Scope[],returnFocus:HTMLElement):Promise<void>{
    this.returnFocus=returnFocus;
    if(!this.caseDialog.open)this.caseDialog.showModal();
    if(this.caseBusy)return;
    if(mode==='export'&&this.caseStage==='export'&&!this.caseInspection)return;
    this.scopes=scopes;this.caseScopes=scopes;this.caseStatus='';
    if(mode==='open'){if(this.openedReview){this.display(this.openedReview,true);this.caseStage='inspect';return;}await this.openPackage();return;}
    this.caseStage='author';this.caseInspection=false;
    await this.run('Loading saved rule choices…',async()=>{
      const choices=await invoke<{rules:Array<{id:string;label:string}>;variants:Array<{label:string;selection:Selection}>}>('case_export_choices');
      this.caseRules=choices.rules;this.caseVariants=choices.variants.map(v=>({...v,value:JSON.stringify([v.selection.experimentId,v.selection.variantId,v.selection.revision])}));
    });
  }
  private async run(status:string,action:()=>Promise<void>):Promise<void>{
    if(this.caseBusy)return;this.caseBusy=true;this.caseStatus=status;const revision=this.connectedRevision;
    try{await action();}catch(error:unknown){
      const canceled=typeof error==='object'&&error!==null&&'category'in error&&error.category==='canceled';
      this.caseStatus=canceled?'Case operation canceled.':describeError(error);
      if(!canceled&&revision===this.connectedRevision)await this.showError('Portable case operation failed',this.caseStatus);
    }finally{if(revision===this.connectedRevision)this.caseBusy=false;}
  }
  async prepare(event:SubmitEvent):Promise<void>{
    event.preventDefault();const data=new FormData(this.authorForm);
    await this.run('Freezing and verifying selected case content…',async()=>{
      const scope=this.scopes.find(s=>s.value===String(data.get('scope')))?.scope??null;
      const review=await invoke<Review>('prepare_case_export',{input:{title:String(data.get('title')),scope,
        ruleIds:data.getAll('rule').map(String),selections:data.getAll('variant').map(v=>this.caseVariants.find(c=>c.value===v)!.selection),
        notes:String(data.get('notes')??''),includeContext:data.get('context')==='on'}});
      this.display(review,false);this.caseStage='export';this.caseStatus='Review the frozen content before saving.';
    });
  }
  private display(review:Review,inspection:boolean):void{
    this.caseFormat='native';this.caseEncrypt=review.encrypted??false;this.saveForm.reset();(this.saveForm.elements.namedItem('encrypt') as HTMLInputElement).checked=this.caseEncrypt;
    const m=review.manifest,rules=m.rules??[],assets=m.assets??[],experiments=m.experiments??[],objects=m.objects??[];this.caseTitle=m.title;this.caseInspection=inspection;this.caseIsCase=m.kind==='case';
    this.caseSources=(m.sources??[]).map(s=>({...s,available:['present','locked'].includes(s.dependency.state)}));
    this.caseExperiments=experiments.map(e=>{const availability=review.inventory.experiments.find(v=>v.id===e.id);return{id:e.id,label:e.label??e.id,state:availability?.state==='ready-for-explicit-import'?'Ready to import disabled':'Unavailable in this build',reason:availability?.reason??'',available:availability?.state==='ready-for-explicit-import'};});
    this.caseContexts=(m.contexts??[]).map(c=>({...c,available:c.content.state==='present'&&objects.some(o=>o.sha256===c.content.sha256&&(o.mediaType==='application/json'||o.mediaType?.startsWith('text/')))}));this.caseContextText='';this.caseContextLabel='';
    this.caseNotes=m.notes??[];this.caseOrdinaryRules=rules.length>0;this.caseExcluded=review.excluded.map(text=>({text}));
    this.caseFacts=[{label:'Content kind',value:m.kind==='rules'?'Rule collection':'Investigation case'},{label:'Saved experiments',value:String(experiments.length)},{label:'Evidence sources',value:String(m.sources?.length??0)},{label:'Rules / response assets',value:`${rules.length} / ${assets.length}`},
      {label:'Original context objects / markers',value:`${m.contexts?.length??0} / ${m.markers?.length??0}`},{label:'Authored notes / retained runs',value:`${m.notes?.length??0} / ${m.runs?.length??0}`},
      {label:'Missing or unavailable dependencies',value:String(review.inventory.missingDependencies)},{label:'Sources with incomplete or unknown evidence',value:String(review.inventory.incompleteSources)},
      {label:'Activation',value:review.activation}];
    if(review.traffic)this.caseFacts=[{label:'Included / retained entries',value:`${review.traffic.entries} / ${review.traffic.retainedEntries}`},{label:'Retained stream records / incomplete body observations',value:`${review.traffic.streamRecords} / ${review.traffic.incompleteBodies}`},...this.caseFacts];
    this.caseImportAvailable=this.caseOrdinaryRules||this.caseExperiments.some(e=>e.available);
    this.caseManifestText=JSON.stringify(m,null,2);this.caseImportItems=[];
  }
  setFormat(event:Event):void{this.caseFormat=(event.target as HTMLSelectElement).value;if(this.caseFormat==='bare-zip'){this.caseEncrypt=false;(this.saveForm.elements.namedItem('encrypt') as HTMLInputElement).checked=false;}}
  setEncryption(event:Event):void{this.caseEncrypt=(event.target as HTMLInputElement).checked;}
  async save(event:SubmitEvent):Promise<void>{
    event.preventDefault();const data=new FormData(this.saveForm);const format=String(data.get('format'));let password:string|null=null;
    await this.run('Saving and verifying the portable case…',async()=>{
      try{
        if(data.get('encrypt')==='on'){
          if(format!=='native')throw new Error('Encryption requires the native transfer format.');
          password=await this.promptTracePassword('Protect the whole case',true,'This protects the manifest, notes, context and every object in the saved package.');if(password===null){this.caseStatus='Save canceled.';return;}
        }
        const destination=await invoke<string|null>(this.caseInspection?'save_open_case':'save_case_export',{format,password});
        this.caseStatus=destination?`Saved ${destination}`:'Save canceled.';
        if(destination)this.showNotice('Portable case saved',destination,null,null);
      }finally{password=null;}
    });
  }
  async openPackage():Promise<void>{
    await this.run('Choose a portable case or rule collection…',async()=>{
      const chosen=await invoke<string|null>('choose_case');if(chosen===null){this.caseStatus='Open canceled.';return;}
      const review=await this.withTracePassword('Unlock portable package',password=>invoke<Review>('open_case',{password}));
      if(!review){this.caseStatus='Open canceled.';return;}
      this.openedReview=review;this.links.clear();this.display(review,true);this.caseStage='inspect';this.caseStatus='Verified package. Evidence and scripts remain inert.';
    });
  }
  async inspect(sourceId:string):Promise<void>{
    await this.run('Opening evidence in Traffic…',async()=>{
      const result=await this.withTracePassword('Unlock embedded source',password=>invoke<{links:Array<{reference:Reference;exchangeId:string}>}>('inspect_case_source',{sourceId,password}));
      if(result){for(const link of result.links)this.links.set(this.referenceKey(link.reference),link.exchangeId);this.$emit('case-evidence-opened',result);this.caseStatus='Evidence opened in this window’s Traffic. Closing this review keeps its lazy bodies available.';}
      else this.caseStatus='Evidence opening canceled.';
    });
  }
  async inspectNote(id:string):Promise<void>{
    const ref=this.caseNotes.find(n=>n.id===id)?.source;if(!ref)return;
    const key=this.referenceKey(ref);
    if(!this.links.has(key)){const source=this.caseSources.find(s=>s.artifactSha256===ref.artifactSha256&&s.sourceKey===ref.sourceKey);if(!source?.available){this.caseStatus='This note’s original source is unavailable.';return;}await this.inspect(source.id);}
    const exchangeId=this.links.get(key);if(!exchangeId){this.caseStatus='The linked request was not retained or could not be identified in this source.';return;}
    if(this.caseDialog.open){this.close();this.$emit('case-reference-request',exchangeId);}
  }
  async inspectContext(id:string):Promise<void>{await this.run('Reading original context…',async()=>{this.caseContextText=await invoke<string>('read_case_context',{id});this.caseContextLabel=this.caseContexts.find(c=>c.id===id)?.kind??id;this.caseStatus='Original context shown as inert text. No configuration commands were run.';});}
  async workingCopy():Promise<void>{
    if(!this.workingConsent.checked){await this.showError('Plaintext working copy','Acknowledge that the new folder contains decrypted evidence, notes and context.');return;}
    await this.run('Creating a verified plaintext working copy…',async()=>{
      const path=await invoke<string|null>('save_case_working_copy');this.caseStatus=path?`Working copy saved to ${path}`:'Working copy canceled.';
    });
  }
  async reviewImport(event:SubmitEvent):Promise<void>{
    event.preventDefault();const data=new FormData(this.importForm);
    await this.run('Reviewing disabled rule import…',async()=>{
      const review=await invoke<RuleReview>('review_case_rules',{experimentIds:data.getAll('experiment').map(String),ordinary:data.get('ordinary')==='on'});
      this.caseImportItems=review.items;this.caseStage='import';this.caseStatus='Every imported definition receives a fresh local identity. Existing content stays in place.';
    });
  }
  async importRules():Promise<void>{await this.run('Importing reviewed rules disabled…',async()=>{
    const status=await invoke<AutomationStatus>('import_case_rules');this.$emit('automation-state-change',status);this.caseStage='inspect';this.caseImportItems=[];this.caseStatus='Imported rules disabled. Named experiments are paused and unselected. Open Automation to review or activate them.';
    this.showNotice('Rules imported disabled',this.caseStatus,null,null);
  });}
  back():void{if(!this.caseBusy){this.caseStage=this.caseInspection?'inspect':'author';this.caseImportItems=[];}}
  async cancel():Promise<void>{await invoke('cancel_case_operation');this.caseStatus='Cancellation requested. Waiting for owned work to finish…';}
  close(event?:Event):void{event?.preventDefault();this.caseDialog.close();this.returnFocus?.focus({preventScroll:true});}
  async closePackage():Promise<void>{if(this.caseBusy)return;await invoke('close_case');this.openedReview=null;this.links.clear();this.caseStage='author';this.caseInspection=false;this.caseSources=[];this.caseContexts=[];this.caseContextText='';this.caseManifestText='';this.close();}
}
PortableCase.define('portable-case');
