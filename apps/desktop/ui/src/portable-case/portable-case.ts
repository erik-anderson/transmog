import {attr, observable} from '@microsoft/webui-framework';
import {invoke} from '@tauri-apps/api/core';
import type {AutomationStatus} from '../models.js';
import {WorkspaceElement} from '../workspace-element.js';
import {describeError} from '../utilities.js';

type Selection={experimentId:string;variantId:string;revision:number};
type Scope={value:string;label:string;scope:unknown};
type Reference={artifactSha256:string;sourceKey:string;exchangeId:string|null;occurrence:string};
type Source={id:string;label:string;requestCount?:string;artifactSha256?:string;sourceKey?:string;dependency:{state:string;reason?:string};evidenceCompleteness:string};
type Context={id:string;kind:string;completeness:string;content:{state:string;sha256?:string}};
type FrozenSelection={experimentId:string;variantId:string;revision:string};
type SavedComparison={id:string;label:string;kind:string;engine:string;left:Reference[];right:Reference[];recordedSummary:string};
type Review={encrypted?:boolean;traffic?:{entries:number;retainedEntries:number;streamRecords:number;incompleteBodies:number}|null;manifest:{id:string;kind:string;title:string;sources?:Source[];rules?:unknown[];assets?:unknown[];experiments?:Array<{id:string;label?:string;variants?:Array<{id:string;label:string;revisions:Array<{revision:string;rules:Array<{kind:string;enabledIntent:boolean}>}>}>}>;comparisons?:SavedComparison[];notes?:Array<{id:string;text:string;source?:Reference|null}>;contexts?:Context[];markers?:unknown[];runs?:unknown[];objects?:Array<{role:string;bytes:string;sha256?:string;mediaType?:string}>};inventory:{missingDependencies:number;incompleteSources:number;experiments:Array<{id:string;state:string;reason:string|null}>};excluded:string[];activation:string};
type RuleReview={items:Array<{kind:string;portableId:string;state:string;localId:string}>;activation:string};
export class PortableCase extends WorkspaceElement {
  @attr({mode:'boolean'}) viewerMode=false;
  @observable caseStage='author';
  @observable caseComparisons:Array<{id:string;label:string;scope:unknown;included:boolean}>=[];
  @observable caseSavedComparisons:SavedComparison[]=[];
  @observable casePreviewPages:Array<{id:string;url:string}>=[];
  @observable casePreviewVariants:Array<{value:string;label:string;selection:FrozenSelection|null;available:boolean}>=[];
  @observable casePreviewDocument='';
  @observable casePreviewAvailable=false;
  private casePreviewLabel='';
  @observable casePreviewVariant='';
  previewForm!:HTMLFormElement;
  private viewRevision=0;
  private previewOperation='';

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
  @observable caseSources:Array<Source&{available:boolean;summary:string;completenessLabel:string}>=[];
  @observable caseExperiments:Array<{id:string;label:string;state:string;reason:string;available:boolean}>=[];
  @observable caseNotes:Array<{id:string;text:string;source?:Reference|null}>=[];
  @observable caseExcluded:Array<{text:string}>=[];
  @observable caseFacts:Array<{label:string;value:string}>=[];
  @observable caseCaptureFacts:Array<{label:string;value:string}>=[];
  @observable caseManifestText='';
  @observable caseImportItems:RuleReview['items']=[];
  @observable caseOrdinaryRules=false;
  @observable caseImportAvailable=false;
  @observable caseIsCase=true;
  @observable caseInspection=false;
  caseDialog!:HTMLDialogElement;
  caseScroll!:HTMLDivElement;
  authorForm!:HTMLFormElement;
  saveForm!:HTMLFormElement;
  importForm!:HTMLFormElement;
  private scopes:Scope[]=[];
  private returnFocus:HTMLElement|null=null;
  private connectedRevision=0;
  private openedReview:Review|null=null;
  private displayedReview:Review|null=null;
  private links=new Map<string,string>();
  private referenceKey(ref:Reference):string{return JSON.stringify([ref.artifactSha256,ref.sourceKey,ref.exchangeId,ref.occurrence]);}
  disconnectedCallback():void {this.connectedRevision++;super.disconnectedCallback();void invoke('close_case').catch(()=>undefined);}
  async show(mode:'export'|'open',scopes:Scope[],returnFocus:HTMLElement,comparisonId?:string):Promise<void>{
    this.viewRevision++;this.returnFocus=returnFocus;
    if(!this.caseDialog.open)this.caseDialog.showModal();
    if(this.caseBusy)return;
    if(mode==='export'&&!comparisonId&&this.caseStage==='export'&&!this.caseInspection)return;
    this.scopes=scopes;this.caseScopes=scopes;this.caseStatus='';
    if(mode==='open'){if(this.openedReview){this.display(this.openedReview,true);this.caseStage='inspect';return;}await this.openPackage();return;}
    this.caseStage='author';this.caseInspection=false;
    await this.run('Loading saved rule choices…',async()=>{
      const choices=await invoke<{rules:Array<{id:string;label:string}>;variants:Array<{label:string;selection:Selection}>;comparisons?:Array<{id:string;label:string;scope:unknown}>}>('case_export_choices');
      this.caseComparisons=(choices.comparisons??[]).map(c=>({...c,included:c.id===comparisonId}));
      if(comparisonId){const selected=this.caseComparisons.find(c=>c.id===comparisonId);if(!selected)throw new Error('This temporary comparison cannot be saved. Portable cases support recorded-request and capture-scope comparisons.');this.caseScopes=[{value:'comparison',label:'All requests required by this comparison',scope:selected.scope},...scopes];this.scopes=this.caseScopes;this.$flushUpdates();(this.authorForm.elements.namedItem('scope') as HTMLSelectElement).value='comparison';}
      this.caseRules=choices.rules;this.caseVariants=choices.variants.map(v=>({...v,value:JSON.stringify([v.selection.experimentId,v.selection.variantId,v.selection.revision])}));
    });
  }
  private async run(status:string,action:()=>Promise<void>):Promise<void>{
    if(this.caseBusy)return;this.caseBusy=true;this.caseStatus=status;const revision=this.connectedRevision,viewRevision=this.viewRevision;
    try{await action();}catch(error:unknown){
      const canceled=typeof error==='object'&&error!==null&&'category'in error&&error.category==='canceled'||describeError(error).toLowerCase().includes('canceled');
      this.caseStatus=canceled?'Case operation canceled.':describeError(error);
      if(!canceled&&revision===this.connectedRevision){if(this.caseDialog.open&&viewRevision===this.viewRevision)await this.showError('Portable case operation failed',this.caseStatus);else this.showNotice('Portable case operation failed',this.caseStatus,null,null);}
    }finally{if(revision===this.connectedRevision)this.caseBusy=false;}
  }
  async prepare(event:SubmitEvent):Promise<void>{
    event.preventDefault();const data=new FormData(this.authorForm);
    await this.run('Freezing and verifying selected case content…',async()=>{
      const scope=this.scopes.find(s=>s.value===String(data.get('scope')))?.scope??null;
      const review=await invoke<Review>('prepare_case_export',{input:{title:String(data.get('title')),scope,
        ruleIds:data.getAll('rule').map(String),selections:data.getAll('variant').map(v=>this.caseVariants.find(c=>c.value===v)!.selection),
        notes:String(data.get('notes')??''),includeContext:data.get('context')==='on',comparisonIds:data.getAll('comparison').map(String)}});
      this.display(review,false);this.caseStage='export';this.caseStatus='Review the frozen content before saving.';
    });
  }
  private display(review:Review,inspection:boolean):void{
    if(review!==this.displayedReview){this.caseScroll.scrollTop=0;this.displayedReview=review;}
    this.caseFormat='native';this.caseEncrypt=review.encrypted??false;this.saveForm.reset();(this.saveForm.elements.namedItem('encrypt') as HTMLInputElement).checked=this.caseEncrypt;
    const m=review.manifest,rules=m.rules??[],assets=m.assets??[],experiments=m.experiments??[],objects=m.objects??[];this.caseTitle=m.title;this.caseInspection=inspection;this.caseIsCase=m.kind==='case';
    this.caseSources=(m.sources??[]).map(s=>({...s,available:['present','locked'].includes(s.dependency.state),summary:[s.requestCount===undefined?'Request count not recorded':`${s.requestCount} requests`,s.dependency.state==='present'?'Included':s.dependency.state==='locked'?'Source password required':'Unavailable'].join(' · '),completenessLabel:s.evidenceCompleteness==='complete'?'Capture reports complete evidence.':s.evidenceCompleteness==='unknown'?'Capture completeness was not established.':s.evidenceCompleteness==='redacted'?'Some captured content was redacted.':'Some content was not fully captured or retained.'}));
    this.caseExperiments=experiments.map(e=>{const availability=review.inventory.experiments.find(v=>v.id===e.id);return{id:e.id,label:e.label??e.id,state:availability?.state==='ready-for-explicit-import'?'Ready to import disabled':'Unavailable in this build',reason:availability?.reason??'',available:availability?.state==='ready-for-explicit-import'};});
    this.caseContexts=(m.contexts??[]).map(c=>({...c,available:c.content.state==='present'&&objects.some(o=>o.sha256===c.content.sha256&&(o.mediaType==='application/json'||o.mediaType?.startsWith('text/')))}));this.caseContextText='';this.caseContextLabel='';
    this.caseNotes=m.notes??[];this.caseOrdinaryRules=rules.length>0;this.caseExcluded=review.excluded.map(text=>({text}));
    const requestCount=review.traffic?String(review.traffic.entries):this.caseSources.every(s=>s.requestCount!==undefined)?this.caseSources.reduce((n,s)=>n+BigInt(s.requestCount!),0n).toString():'Not recorded — inspect in Traffic';
    this.caseFacts=[{label:'Captured requests',value:requestCount},{label:'Captures',value:String(this.caseSources.length)}];
    for(const [label,count] of [['Saved rules',rules.length],['Response files',assets.length],['Experiments',experiments.length],['Source context files',m.contexts?.length??0],['Incident markers',m.markers?.length??0],['Notes',m.notes?.length??0],['Saved runs',m.runs?.length??0],['Unavailable files',review.inventory.missingDependencies]] as const)if(count)this.caseFacts.push({label,value:String(count)});
    this.caseCaptureFacts=review.traffic?[{label:'Stream events observed',value:String(review.traffic.streamRecords)},{label:'Body records without full content',value:String(review.traffic.incompleteBodies)}]:[];
    this.caseSavedComparisons=m.comparisons??[];
    this.casePreviewVariants=[{value:'',label:'Captured baseline',selection:null,available:true}];if(rules.some(r=>(r as {kind?:string}).kind==='auto-response'))this.casePreviewVariants.push({value:'rules',label:'Frozen saved auto-responses',selection:null,available:true});
    for(const e of experiments)for(const v of e.variants??[])for(const r of v.revisions)this.casePreviewVariants.push({value:JSON.stringify([e.id,v.id,r.revision]),label:`${e.label??e.id} / ${v.label} / revision ${r.revision}`,selection:{experimentId:e.id,variantId:v.id,revision:r.revision},available:r.rules.filter(rule=>rule.enabledIntent).every(rule=>rule.kind==='auto-response')});
    this.casePreviewPages=[];this.casePreviewDocument='';this.casePreviewVariant='';
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
      this.openedReview=review;this.casePreviewAvailable=false;this.casePreviewLabel='';this.links.clear();this.display(review,true);this.caseStage='inspect';this.caseStatus='Verified package. Evidence and scripts remain inert.';
    });
  }
  async inspect(sourceId:string):Promise<boolean>{
    let success=false;
    await this.run('Opening evidence in Traffic…',async()=>{
      const result=await this.withTracePassword('Unlock embedded source',password=>invoke<{links:Array<{reference:Reference;exchangeId:string}>}>('inspect_case_source',{sourceId,password}));
      if(result){success=true;for(const link of result.links)this.links.set(this.referenceKey(link.reference),link.exchangeId);this.$emit('case-evidence-opened',result);this.caseStatus='Evidence opened in this window’s Traffic. Closing this review keeps its lazy bodies available.';}
      else this.caseStatus='Evidence opening canceled.';
    });return success;
  }
  async inspectTraffic(sourceId:string):Promise<void>{
    const revision=this.viewRevision;
    if(!await this.inspect(sourceId)||revision!==this.viewRevision||!this.caseDialog.open)return;
    const source=this.caseSources.find(s=>s.id===sourceId);
    const first=[...this.links].find(([key])=>{const [artifact,sourceKey]=JSON.parse(key) as string[];return artifact===source?.artifactSha256&&sourceKey===source?.sourceKey;});
    this.close();if(first)this.$emit('case-reference-request',first[1]);
  }
  async choosePreview(sourceId:string):Promise<void>{
    const revision=this.viewRevision;if(!await this.inspect(sourceId)||revision!==this.viewRevision||!this.caseDialog.open)return;
    await this.run('Finding complete HTML responses in opened case evidence…',async()=>{const choices=await invoke<{pages:Array<{id:string;url:string}>;more?:boolean}>('case_preview_choices',{sourceId});if(revision!==this.viewRevision||!this.caseDialog.open)return;this.casePreviewPages=choices.pages;this.casePreviewDocument=choices.pages[0]?.id??'';this.casePreviewVariant='';this.caseStage='preview';this.caseStatus=choices.pages.length?(choices.more?'Showing 100 HTML documents. Export a narrower case to choose another document.':'Choose a document and frozen response variant.'):'No complete retained HTML response is available in this source.';this.$flushUpdates();this.caseScroll.scrollTop=0;this.previewForm.reset();});
  }
  setPreviewDocument(event:Event):void{this.casePreviewDocument=(event.target as HTMLSelectElement).value;}
  setPreviewVariant(event:Event):void{this.casePreviewVariant=(event.target as HTMLSelectElement).value;}
  async preview(event:SubmitEvent):Promise<void>{
    event.preventDefault();if(!this.casePreviewDocument)return;
    const id=this.casePreviewDocument,selection=this.casePreviewVariants.find(v=>v.value===this.casePreviewVariant)?.selection??null;
    await this.run('Preparing an isolated case preview…',async()=>{const operationId=crypto.randomUUID();this.previewOperation=operationId;try{const label=await invoke<string>('open_captured_page',{id,enableScripts:new FormData(this.previewForm).get('scripts')==='on',options:{casePreview:{selection,ordinary:this.casePreviewVariant==='rules',scope:String(new FormData(this.previewForm).get('scope'))}},sourceLabel:null,operationId});this.casePreviewLabel=label;this.casePreviewAvailable=true;this.$emit('case-preview-opened',{label,id});this.caseStatus='Case preview opened. Preview diagnostics show the actual engine, script choice and resource outcomes.';if(this.caseDialog.open)this.caseStage='inspect';}finally{this.previewOperation='';}});
  }
  previewDiagnostics():void{if(this.casePreviewLabel)this.$emit('case-preview-report-request',this.casePreviewLabel);}
  async openComparison(id:string):Promise<void>{
    const revision=this.viewRevision,saved=this.caseSavedComparisons.find(c=>c.id===id);if(!saved)return;
    const needed=new Set([...saved.left,...saved.right].map(r=>JSON.stringify([r.artifactSha256,r.sourceKey])));
    for(const source of this.caseSources){if(!needed.has(JSON.stringify([source.artifactSha256,source.sourceKey])))continue;if(!source.available){this.caseStatus='This comparison needs an unavailable source. Its recorded findings remain historical.';return;}if(!await this.inspect(source.id)||revision!==this.viewRevision||!this.caseDialog.open)return;}
    await this.run('Resolving saved comparison evidence…',async()=>{const resolved=await invoke<{kind:string;left:unknown;right:unknown;options:unknown}>('resolve_case_comparison',{id});if(revision!==this.viewRevision||!this.caseDialog.open)return;this.close();this.$emit('case-comparison-request',{...resolved,id});});
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
  async cancel():Promise<void>{if(this.previewOperation)await invoke('cancel_captured_page',{operationId:this.previewOperation});else await invoke('cancel_case_operation');this.caseStatus='Cancellation requested. Waiting for owned work to finish…';}
  close(event?:Event):void{event?.preventDefault();this.viewRevision++;if(this.previewOperation)void invoke('cancel_captured_page',{operationId:this.previewOperation});this.caseDialog.close();this.returnFocus?.focus({preventScroll:true});}
  async closePackage():Promise<void>{if(this.caseBusy)return;await invoke('close_case');this.openedReview=null;this.links.clear();this.caseStage='author';this.caseInspection=false;this.caseSources=[];this.caseContexts=[];this.caseContextText='';this.caseManifestText='';this.close();}
}
PortableCase.define('portable-case');
