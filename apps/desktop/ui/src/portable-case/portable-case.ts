import {attr, observable} from '@microsoft/webui-framework';
import {invoke} from '@tauri-apps/api/core';
import type {AutomationStatus} from '../models.js';
import {WorkspaceElement} from '../workspace-element.js';
import {describeError} from '../utilities.js';

type Selection={experimentId:string;variantId:string;revision:number};
type RuleChoice={id:string;label:string;ruleId:string|null;selection:Selection|null;relevance:'matched'|'unmatched'|'unknown'|'not-assessed';unavailable?:boolean};
type ChoiceResponse={choices:RuleChoice[];requestCount:number;evidenceSelected:boolean;comparisons:Array<{id:string;label:string;scope:unknown}>};
type Scope={value:string;label:string;scope:unknown};
type Reference={artifactSha256:string;sourceKey:string;exchangeId:string|null;occurrence:string};
type Source={id:string;label:string;requestCount?:string;artifactSha256?:string;sourceKey?:string;dependency:{state:string;reason?:string};evidenceCompleteness:string};
type Context={id:string;kind:string;completeness:string;content:{state:string;sha256?:string}};
type FrozenSelection={experimentId:string;variantId:string;revision:string};
type SavedComparison={id:string;label:string;kind:string;engine:string;left:Reference[];right:Reference[];recordedSummary:string};
type Review={encrypted?:boolean;traffic?:{entries:number;retainedEntries:number;streamRecords:number;incompleteBodies:number}|null;manifest:{id:string;kind:string;title:string;sources?:Source[];rules?:unknown[];assets?:unknown[];experiments?:Array<{id:string;label?:string;variants?:Array<{id:string;label:string;revisions:Array<{revision:string;rules:Array<{kind:string;enabledIntent:boolean}>}>}>}>;comparisons?:SavedComparison[];notes?:Array<{id:string;text:string;source?:Reference|null}>;contexts?:Context[];markers?:unknown[];runs?:unknown[];objects?:Array<{role:string;bytes:string;sha256?:string;mediaType?:string}>};inventory:{missingDependencies:number;incompleteSources:number;experiments:Array<{id:string;state:string;reason:string|null}>};excluded:string[];activation:string};
type RuleReview={source:{rules?:Array<{id:string;label:string}>;experiments:Array<{id:string;label:string;variants:Array<{revisions:Array<{rules:Array<{id:string;label:string}>}>}>}>};items:Array<{kind:string;portableId:string;state:string;localId:string}>;activation:string};
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

  @observable caseBusy=false;
  @observable caseStatus='';
  @observable caseContexts:Array<Context&{available:boolean}>=[];
  @observable caseContextText='';
  @observable caseContextLabel='';
  @observable caseTitle='Investigation';
  @observable caseScopes:Scope[]=[];
  @observable caseRuleChoices:Array<RuleChoice&{selected:boolean;kindLabel:string;matchLabel:string}>=[];
  @observable caseChoicesBusy=false;
  @observable caseChoiceMessage='';
  @observable caseChoiceError='';
  @observable caseChoiceFilterAvailable=false;
  @observable caseShowAll=false;
  private allChoices:RuleChoice[]=[];
  private selectedChoices=new Set<string>();
  private pinnedChoices=new Set<string>();
  private choiceRevision=0;
  @observable caseSources:Array<Source&{available:boolean;summary:string;completenessLabel:string}>=[];
  @observable caseExperiments:Array<{id:string;label:string;state:string;reason:string;available:boolean}>=[];
  @observable caseNotes:Array<{id:string;text:string;source?:Reference|null}>=[];
  @observable caseExcluded:Array<{text:string}>=[];
  @observable caseFacts:Array<{label:string;value:string}>=[];
  @observable caseCaptureFacts:Array<{label:string;value:string}>=[];
  @observable caseManifestText='';
  @observable caseImportItems:Array<RuleReview['items'][number]&{label:string;description:string}>=[];
  @observable caseImportSummary='';
  @observable caseSummary='';
  @observable caseSelectedCount=0;
  @observable caseComparisonCount=0;
  @observable caseSelectedSource='';
  @observable caseSourceAvailable=false;
  @observable caseSourceStatus='';
  caseMore!:HTMLElement;
  caseMoreButton!:HTMLButtonElement;
  comparisonChoices!:HTMLDetailsElement;
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
  async show(mode:'export'|'open'|'current',scopes:Scope[],returnFocus:HTMLElement,comparisonId?:string):Promise<void>{
    this.viewRevision++;this.returnFocus=returnFocus;
    if(!this.caseDialog.open)this.caseDialog.showModal();
    if(this.caseBusy)return;
    if(mode==='export'&&!comparisonId&&this.caseStage==='export'&&!this.caseInspection)return;
    this.scopes=scopes;this.caseScopes=scopes;this.caseStatus='';
    if(mode==='current'&&this.openedReview){this.display(this.openedReview,true);this.caseStage='inspect';return;}
    if(mode==='open'||mode==='current'){await this.openPackage();return;}
    this.caseStage='author';this.caseInspection=false;
    this.$flushUpdates();
    await this.refreshChoices(comparisonId);
  }
  async refreshChoices(comparisonId?:string):Promise<void>{
    const revision=++this.choiceRevision,view=this.viewRevision;
    const scope=comparisonId?null:this.scopes.find(s=>s.value===String(new FormData(this.authorForm).get('scope')))?.scope??null;
    this.caseChoicesBusy=true;this.caseChoiceError='';this.caseChoiceFilterAvailable=scope!==null;
    try{
      const response=await invoke<ChoiceResponse>('case_export_choices',{scope,comparisonIds:comparisonId?[comparisonId]:new FormData(this.authorForm).getAll('comparison').map(String)});
      if(revision!==this.choiceRevision||view!==this.viewRevision||!this.caseDialog.open)return;
      const available=new Set(response.choices.map(c=>c.id));
      const retained=this.allChoices.filter(c=>this.selectedChoices.has(c.id)&&!available.has(c.id)).map(c=>({...c,unavailable:true}));
      this.allChoices=[...response.choices,...retained];
      this.pinnedChoices.clear();
      const included=new Set(new FormData(this.authorForm).getAll('comparison').map(String));
      this.caseComparisons=(response.comparisons??[]).map(c=>({...c,included:comparisonId?c.id===comparisonId:included.has(c.id)}));
      this.caseComparisonCount=this.caseComparisons.filter(c=>c.included).length;
      this.caseChoiceFilterAvailable=response.evidenceSelected;
      this.caseChoiceMessage=response.evidenceSelected?`${response.requestCount} included requests.`:'No traffic selected. You can include rules, experiments and notes.';
      this.renderChoices();
      if(comparisonId){const selected=this.caseComparisons.find(c=>c.id===comparisonId);if(!selected)throw new Error('This comparison is no longer available. Compare the requests again before saving it.');this.caseScopes=[{value:'comparison',label:'All requests required by this comparison',scope:selected.scope},...this.scopes.filter(s=>s.value!=='comparison')];this.scopes=this.caseScopes;this.$flushUpdates();(this.authorForm.elements.namedItem('scope') as HTMLSelectElement).value='comparison';await this.refreshChoices();this.comparisonChoices.open=true;}
    }catch(error:unknown){if(revision!==this.choiceRevision||view!==this.viewRevision||!this.caseDialog.open)return;this.caseChoiceError=describeError(error);this.caseChoiceMessage='Matches are unavailable. Your selections are unchanged.';this.allChoices=this.allChoices.map(c=>({...c,relevance:'unknown'}));this.caseShowAll=true;this.renderChoices();}
    finally{if(revision===this.choiceRevision)this.caseChoicesBusy=false;}
  }
  private renderChoices():void{
    this.caseSelectedCount=this.selectedChoices.size;
    this.caseRuleChoices=this.allChoices.filter(c=>!this.caseChoiceFilterAvailable||this.caseShowAll||c.relevance==='matched'||this.selectedChoices.has(c.id)||this.pinnedChoices.has(c.id))
      .sort((a,b)=>Number(b.relevance==='matched')-Number(a.relevance==='matched'))
      .map(c=>({...c,selected:this.selectedChoices.has(c.id),kindLabel:c.selection?'Experiment version':'Saved rule',matchLabel:c.unavailable?'No longer saved — deselect to continue':c.relevance==='matched'?'Matches':c.relevance==='unmatched'?'No captured match':c.relevance==='unknown'?'Could not assess':'Not checked'}));
  }
  setShowAll(event:Event):void{this.caseShowAll=(event.target as HTMLInputElement).checked;this.pinnedChoices.clear();this.renderChoices();}
  setChoice(id:string,event:Event):void{if((event.target as HTMLInputElement).checked)this.selectedChoices.add(id);else this.selectedChoices.delete(id);this.pinnedChoices.add(id);this.renderChoices();}
  private async run(status:string,action:()=>Promise<void>):Promise<void>{
    if(this.caseBusy)return;this.caseBusy=true;this.caseStatus=status;const revision=this.connectedRevision,viewRevision=this.viewRevision;
    try{await action();}catch(error:unknown){
      const canceled=typeof error==='object'&&error!==null&&'category'in error&&error.category==='canceled'||describeError(error).toLowerCase().includes('canceled');
      this.caseStatus=canceled?'Case operation canceled.':describeError(error);
      if(!canceled&&revision===this.connectedRevision){if(this.caseDialog.open&&viewRevision===this.viewRevision)await this.showError('Case operation failed',this.caseStatus);else this.showNotice('Case operation failed',this.caseStatus,null,null);}
    }finally{if(revision===this.connectedRevision){this.caseBusy=false;if(!this.caseDialog.open)this.$emit('case-review-closed');}}
  }
  async prepare(event:SubmitEvent):Promise<void>{
    event.preventDefault();const data=new FormData(this.authorForm);
    if(this.caseChoicesBusy)return;
    await this.run('Preparing selected content…',async()=>{
      const selected=this.allChoices.filter(c=>this.selectedChoices.has(c.id));
      if(selected.some(c=>c.unavailable))throw new Error('Some selected content is no longer saved. Deselect it before continuing.');
      const scope=this.scopes.find(s=>s.value===String(data.get('scope')))?.scope??null;
      const review=await invoke<Review>('prepare_case_export',{input:{title:String(data.get('title')),scope,
        ruleIds:selected.flatMap(c=>c.ruleId?[c.ruleId]:[]),selections:selected.flatMap(c=>c.selection?[c.selection]:[]),
        notes:String(data.get('notes')??''),includeContext:data.get('context')==='on',comparisonIds:data.getAll('comparison').map(String)}});
      this.display(review,false);this.caseStage='export';this.caseStatus='Ready to save.';
    });
  }
  private display(review:Review,inspection:boolean):void{
    const sameReview=review===this.displayedReview;
    if(!sameReview){this.caseScroll.scrollTop=0;this.displayedReview=review;}
    this.saveForm.reset();(this.saveForm.elements.namedItem('encrypt') as HTMLInputElement).checked=review.encrypted??false;
    const m=review.manifest,rules=m.rules??[],assets=m.assets??[],experiments=m.experiments??[],objects=m.objects??[];this.caseTitle=m.title;this.caseInspection=inspection;this.caseIsCase=m.kind==='case';
    this.caseSources=(m.sources??[]).map(s=>({...s,available:['present','locked'].includes(s.dependency.state),summary:[s.requestCount===undefined?'Request count not recorded':`${s.requestCount} request${s.requestCount==='1'?'':'s'}`,s.dependency.state==='present'?'Included':s.dependency.state==='locked'?'Source password required':'Unavailable'].join(' · '),completenessLabel:s.evidenceCompleteness==='complete'?'Capture reports complete evidence.':s.evidenceCompleteness==='unknown'?'Capture completeness was not established.':s.evidenceCompleteness==='redacted'?'Some captured content was redacted.':'Some content was not fully captured or retained.'}));
    this.caseExperiments=experiments.map(e=>{const availability=review.inventory.experiments.find(v=>v.id===e.id);return{id:e.id,label:e.label??e.id,state:availability?.state==='ready-for-explicit-import'?'Ready to import':'Unavailable in this build',reason:availability?.reason??'',available:availability?.state==='ready-for-explicit-import'};});
    this.caseContexts=(m.contexts??[]).map(c=>({...c,kind:c.kind==='capture-metadata'?'Capture information':c.kind,available:c.content.state==='present'&&objects.some(o=>o.sha256===c.content.sha256&&(o.mediaType==='application/json'||o.mediaType?.startsWith('text/')))}));this.caseContextText='';this.caseContextLabel='';
    this.caseNotes=m.notes??[];this.caseOrdinaryRules=rules.length>0;this.caseExcluded=review.excluded.map(text=>({text}));
    const requestCount=review.traffic?String(review.traffic.entries):this.caseSources.every(s=>s.requestCount!==undefined)?this.caseSources.reduce((n,s)=>n+BigInt(s.requestCount!),0n).toString():null;
    this.caseSummary=[requestCount===null?'Request count not recorded':`${requestCount} request${requestCount==='1'?'':'s'}`,`${this.caseSources.length} capture${this.caseSources.length===1?'':'s'}`,experiments.length?`${experiments.length} experiment${experiments.length===1?'':'s'}`:'',rules.length?`${rules.length} saved rule${rules.length===1?'':'s'}`:'',review.inventory.missingDependencies?`${review.inventory.missingDependencies} unavailable file${review.inventory.missingDependencies===1?'':'s'}`:''].filter(Boolean).join(' · ');
    if(!sameReview||!this.caseSources.some(s=>s.id===this.caseSelectedSource))this.caseSelectedSource=this.caseSources.find(s=>s.available)?.id??this.caseSources[0]?.id??'';
    this.updateSource();
    this.caseFacts=[{label:'Captured requests',value:requestCount??'Not recorded — inspect in Traffic'},{label:'Captures',value:String(this.caseSources.length)}];
    for(const [label,count] of [['Saved rules',rules.length],['Response files',assets.length],['Experiments',experiments.length],['Source context files',m.contexts?.length??0],['Incident markers',m.markers?.length??0],['Notes',m.notes?.length??0],['Saved runs',m.runs?.length??0],['Unavailable files',review.inventory.missingDependencies]] as const)if(count)this.caseFacts.push({label,value:String(count)});
    this.caseCaptureFacts=review.traffic?[{label:'Stream events observed',value:String(review.traffic.streamRecords)},{label:'Body records without full content',value:String(review.traffic.incompleteBodies)}]:[];
    this.caseSavedComparisons=m.comparisons??[];
    this.casePreviewVariants=[{value:'',label:'Captured baseline',selection:null,available:true}];if(rules.some(r=>(r as {kind?:string}).kind==='auto-response'))this.casePreviewVariants.push({value:'rules',label:'Saved auto-responses',selection:null,available:true});
    for(const e of experiments)for(const v of e.variants??[])for(const r of v.revisions)this.casePreviewVariants.push({value:JSON.stringify([e.id,v.id,r.revision]),label:`${e.label??e.id} / ${v.label} / revision ${r.revision}`,selection:{experimentId:e.id,variantId:v.id,revision:r.revision},available:r.rules.filter(rule=>rule.enabledIntent).every(rule=>rule.kind==='auto-response')});
    this.casePreviewPages=[];this.casePreviewDocument='';this.casePreviewVariant='';
    this.caseImportAvailable=this.caseOrdinaryRules||this.caseExperiments.some(e=>e.available);
    this.caseManifestText=JSON.stringify(m,null,2);this.caseImportItems=[];
  }
  async save(event:SubmitEvent):Promise<void>{
    event.preventDefault();const data=new FormData(this.saveForm);let password:string|null=null;
    await this.run('Saving and verifying the case…',async()=>{
      try{
        if(data.get('encrypt')==='on'){
          password=await this.promptTracePassword('Protect the whole case',true,'This protects the manifest, notes, context and every object in the saved package.');if(password===null){this.caseStatus='Save canceled.';return;}
        }
        const destination=await invoke<string|null>(this.caseInspection?'save_open_case':'save_case_export',{password});
        this.caseStatus=destination?`Saved ${destination}`:'Save canceled.';
        if(destination)this.showNotice('Case saved',destination,null,null);
      }finally{password=null;}
    });
  }
  async showPath(path:string,returnFocus:HTMLElement):Promise<void>{
    if(this.caseBusy)return;
    this.viewRevision++;this.returnFocus=returnFocus;
    if(!this.caseDialog.open)this.caseDialog.showModal();
    await this.openPackage(path);
  }
  selectSource(event:Event):void{this.caseSelectedSource=(event.target as HTMLSelectElement).value;this.updateSource();}
  private updateSource():void{const source=this.caseSources.find(s=>s.id===this.caseSelectedSource);this.caseSourceAvailable=source?.available??false;this.caseSourceStatus=source?.dependency.reason??(source?.available?'Open this capture to inspect its requests.':'This capture is unavailable.');}
  private closeMore():void{if(this.caseMore?.matches(':popover-open')){this.caseMore.hidePopover();this.caseMoreButton.focus();}}
  showSaveCopy():void{this.closeMore();this.caseStage='copy';this.$flushUpdates();this.caseScroll.scrollTop=this.caseScroll.scrollHeight;(this.saveForm.elements.namedItem('encrypt') as HTMLInputElement).focus();}
  async openPackage(path?:string):Promise<void>{
    this.closeMore();
    await this.run(path?'Opening case…':'Choose a case or rule collection…',async()=>{
      const chosen=path??await invoke<string|null>('choose_case');if(chosen===null){this.caseStatus='Open canceled.';return;}
      const review=await this.withTracePassword('Unlock case',password=>invoke<Review>('open_case',{password,path:path??null}));
      if(!review){this.caseStatus='Open canceled.';return;}
      this.openedReview=review;this.casePreviewAvailable=false;this.casePreviewLabel='';this.links.clear();this.display(review,true);this.caseStage='inspect';this.caseStatus='Opening this case does not enable its rules or contact captured sites.';this.$emit('case-open-change',true);
    });
  }
  async inspect(sourceId:string):Promise<boolean>{
    let success=false;
    await this.run('Opening evidence in Traffic…',async()=>{
      const result=await this.withTracePassword('Unlock embedded source',password=>invoke<{links:Array<{reference:Reference;exchangeId:string}>}>('inspect_case_source',{sourceId,password}));
      if(result){success=true;for(const link of result.links)this.links.set(this.referenceKey(link.reference),link.exchangeId);this.$emit('case-evidence-opened',result);this.caseStatus='Capture opened in Traffic.';}
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
    await this.run('Finding complete HTML responses in opened case evidence…',async()=>{const choices=await invoke<{pages:Array<{id:string;url:string}>;more?:boolean}>('case_preview_choices',{sourceId});if(revision!==this.viewRevision||!this.caseDialog.open)return;this.casePreviewPages=choices.pages;this.casePreviewDocument=choices.pages[0]?.id??'';this.casePreviewVariant='';this.caseStage='preview';this.caseStatus=choices.pages.length?(choices.more?'Showing 100 HTML documents. Export a narrower case to choose another document.':'Choose a page and saved responses.'):'No complete retained HTML response is available in this source.';this.$flushUpdates();this.caseScroll.scrollTop=0;this.previewForm.reset();});
  }
  setPreviewDocument(event:Event):void{this.casePreviewDocument=(event.target as HTMLSelectElement).value;}
  setPreviewVariant(event:Event):void{this.casePreviewVariant=(event.target as HTMLSelectElement).value;}
  async preview(event:SubmitEvent):Promise<void>{
    event.preventDefault();if(!this.casePreviewDocument)return;
    const id=this.casePreviewDocument,selection=this.casePreviewVariants.find(v=>v.value===this.casePreviewVariant)?.selection??null;
    await this.run('Preparing an isolated case preview…',async()=>{const operationId=crypto.randomUUID();this.previewOperation=operationId;try{const label=await invoke<string>('open_captured_page',{id,enableScripts:new FormData(this.previewForm).get('scripts')==='on',options:{casePreview:{selection,ordinary:this.casePreviewVariant==='rules',scope:String(new FormData(this.previewForm).get('scope'))}},sourceLabel:null,operationId});this.casePreviewLabel=label;this.casePreviewAvailable=true;this.$emit('case-preview-opened',{label,id});this.caseStatus='Case preview opened. Preview diagnostics show the actual engine, script choice and resource outcomes.';if(this.caseDialog.open)this.caseStage='inspect';}finally{this.previewOperation='';}});
  }
  previewDiagnostics():void{this.closeMore();if(this.casePreviewLabel)this.$emit('case-preview-report-request',this.casePreviewLabel);}
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
  async inspectContext(id:string):Promise<void>{await this.run('Reading original context…',async()=>{this.caseContextText=await invoke<string>('read_case_context',{id});this.caseContextLabel=this.caseContexts.find(c=>c.id===id)?.kind??id;this.caseStatus='Saved context shown.';});}
  async workingCopy():Promise<void>{
    this.closeMore();
    await this.run('Extracting case contents…',async()=>{
      const path=await invoke<string|null>('save_case_working_copy');this.caseStatus=path?`Folder saved to ${path}`:'Extraction canceled.';
    });
  }
  async reviewImport(event:SubmitEvent):Promise<void>{
    event.preventDefault();const data=new FormData(this.importForm);
    await this.run('Reviewing disabled rule import…',async()=>{
      const review=await invoke<RuleReview>('review_case_rules',{experimentIds:data.getAll('experiment').map(String),ordinary:data.get('ordinary')==='on'});
      this.caseImportItems=review.items.map(item=>({...item,label:(item.kind==='rule'?[...(review.source.rules??[]),...review.source.experiments.flatMap(e=>e.variants.flatMap(v=>v.revisions.flatMap(r=>r.rules)))]:item.kind==='experiment'?review.source.experiments:[]).find(row=>row.id===item.portableId)?.label??(item.kind==='asset'?'Response file':item.kind),description:item.state==='conflicting'?'Adds a separate copy; keeps the existing definition.':item.state==='identical-source-definition'?'Adds a separate copy of an existing definition.':'Adds a new '+(item.kind==='asset'?'response file':item.kind)+'.'}));
      this.caseImportSummary=['rule','experiment','asset'].map(kind=>{const count=review.items.filter(item=>item.kind===kind).length;return count?`${count} ${kind==='asset'?'response file':kind}${count===1?'':'s'}`:'';}).filter(Boolean).join(' · ');
      this.caseStage='import';this.caseStatus='Review what will be added.';
    });
  }
  async importRules():Promise<void>{await this.run('Importing reviewed rules disabled…',async()=>{
    const status=await invoke<AutomationStatus>('import_case_rules');this.$emit('automation-state-change',status);this.caseStage='inspect';this.caseImportItems=[];this.caseStatus='Rules imported and turned off. Open Automation to review or enable them.';
    this.showNotice('Rules imported',this.caseStatus,null,null);
  });}
  back():void{if(!this.caseBusy){this.caseStage=this.caseInspection?'inspect':'author';this.caseImportItems=[];}}
  async cancel():Promise<void>{if(this.previewOperation)await invoke('cancel_captured_page',{operationId:this.previewOperation});else await invoke('cancel_case_operation');this.caseStatus='Cancellation requested. Waiting for owned work to finish…';}
  close(event?:Event):void{event?.preventDefault();this.viewRevision++;this.choiceRevision++;this.caseChoicesBusy=false;if(this.previewOperation)void invoke('cancel_captured_page',{operationId:this.previewOperation});this.closeMore();this.caseDialog.close();this.returnFocus?.focus({preventScroll:true});this.$emit('case-review-closed');}
  async closePackage():Promise<void>{if(this.caseBusy)return;this.closeMore();await invoke('close_case');this.$emit('case-open-change',false);this.openedReview=null;this.links.clear();this.caseStage='author';this.caseInspection=false;this.caseSources=[];this.caseContexts=[];this.caseContextText='';this.caseManifestText='';this.close();}
}
PortableCase.define('portable-case');
