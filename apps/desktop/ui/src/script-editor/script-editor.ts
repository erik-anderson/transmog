import initialState from '../initial-state.json';
import '../editor-surface/editor-surface.js';
import { attr, observable } from '@microsoft/webui-framework';
import { invoke } from '@tauri-apps/api/core';
import { WorkspaceElement } from '../workspace-element.js';
import type { ScriptDraft, ScriptStatus, ScriptCandidate, SelectedResponse } from '../models.js';
import { describeError, optionalText, parseHeaderLines } from '../utilities.js';
import { activeDraft, editingDraft, scriptFingerprint, scriptSettings } from './script-state.js';
import type * as Monaco from 'monaco-editor/editor/editor.api';
import { loadMonaco, waitForStyles } from '../editor-runtime.js';

const SCRIPT_TEMPLATE = `import type { Action, Context, Request } from "transmog:api/v1";

export function onRequestHead(_context: Context, request: Request): Action {
  if (request.host === "example.test") {
    return {
      action: "headers",
      operations: [{ operation: "set", name: "User-Agent", value: "Transmog/1.0" }],
    };
  }
  return { action: "continue" };
}
`;
interface ValidatedDraft { token: ScriptCandidate; draft: ScriptDraft; version: number; }

export class ScriptEditor extends WorkspaceElement {
  @attr theme = 'system';
  @attr({mode:'boolean'}) active = false;
  @observable selection:SelectedResponse|null=null;
  @observable scriptBusy=false;
  @observable scriptReady=false;
  @observable scriptDirty=true;
  @observable scriptUnapplied=true;
  @observable scriptActive=false;
  @observable activeScriptText='Paused';
  @observable scriptText=initialState.scriptText;
  @observable statusKind='';
  @observable editorLoaded=initialState.editorLoaded;
  @observable diffVisible=initialState.diffVisible;
  @observable diffStale=false;
  @observable activeSettings='';
  @observable draftSettings='';
  @observable testHandlers:Array<{name:string}>=[{name:'onRequestHead'}];
  @observable testResult='';
  @observable testEvidence='';
  @observable testOutdated=false;
  scriptTestUrl!:HTMLInputElement;
  scriptTestMethod!:HTMLInputElement;
  scriptTestHeaders!:HTMLTextAreaElement;
  scriptTestBody!:HTMLTextAreaElement;
  scriptTestHandler!:HTMLSelectElement;
  scriptMenu!:HTMLDivElement;
  scriptMore!:HTMLButtonElement;
  monacoStyles!:HTMLLinkElement;
  scriptForm!:HTMLFormElement;
  scriptEditor!:HTMLDivElement;
  scriptDiffEditor!:HTMLDivElement;
  private editVersion=0;
  private testVersion=0;
  private savedFingerprint='';
  private activeFingerprint='';
  private loadedDraft:ScriptDraft|null=null;
  private sourceEditor:Monaco.editor.IStandaloneCodeEditor|null=null;
  private draftModel:Monaco.editor.ITextModel|null=null;
  private revisionDiff:Monaco.editor.IStandaloneDiffEditor|null=null;
  private diffModels:Monaco.editor.ITextModel[]=[];
  private candidate:ValidatedDraft|null=null;
  private scriptRevision=1;
  private loading:Promise<void>|null=null;
  private monaco:typeof Monaco|null=null;
  private declarationsDisposable:Monaco.IDisposable|null=null;
  private readonly systemTheme=window.matchMedia('(prefers-color-scheme: dark)');
  private readonly forcedColors=window.matchMedia('(forced-colors: active)');
  private readonly systemThemeChanged=():void=>{this.themeChanged();};

  markScriptChanged():void {
    this.editVersion++;
    this.candidate=null;
    this.testOutdated=Boolean(this.testResult);
    this.diffStale=this.diffVisible;
    const model=this.sourceEditor?.getModel();
    if(model)this.monaco?.editor.setModelMarkers(model,'transmog-rust',[]);
    this.updateDraftState();
  }
  markTestChanged():void {this.testVersion++;this.testOutdated=Boolean(this.testResult);}
  private updateDraftState():void {
    if(!this.scriptReady)return;
    const draft=this.scriptDraft();
    const fingerprint=scriptFingerprint(draft);
    this.scriptDirty=fingerprint!==this.savedFingerprint;
    this.scriptUnapplied=fingerprint!==this.activeFingerprint;
    this.testHandlers=draft.handlers.map(name=>({name}));
  }
  scriptKeyboard(event:KeyboardEvent):void {
    if((event.ctrlKey||event.metaKey)&&event.key.toLowerCase()==='s'){
      event.preventDefault();void this.saveScript();
    }
  }
  private async runScript(message:string,operation:()=>Promise<void>):Promise<void> {
    if(this.scriptBusy||!this.scriptReady)return;
    this.scriptBusy=true;this.scriptText=message;this.statusKind='progress';
    try{await operation();}finally{this.scriptBusy=false;}
  }
  async saveScript():Promise<void>{await this.runScript('Saving draft…',()=>this.saveDraft());}
  async validateScript(event?:Event):Promise<void>{event?.preventDefault();this.closeMenu();await this.runScript('Validating current source and capabilities…',async()=>{await this.validateDraft();});}
  async testScript():Promise<void>{await this.runScript('Checking source and running the sandbox…',()=>this.testDraft());}
  async primaryAction():Promise<void>{if(this.scriptActive&&!this.scriptUnapplied)await this.disableScript();else await this.activateScript();}
  async activateScript():Promise<void>{await this.runScript('Validating and saving changes before activation…',()=>this.activateDraft());}
  async disableScript():Promise<void>{await this.runScript('Pausing script…',()=>this.disableActive());}
  async compareActiveScript():Promise<void>{this.closeMenu();await this.runScript('Preparing comparison…',()=>this.compareDraft());}
  private closeMenu():void {if(this.scriptMenu?.matches(':popover-open')){this.scriptMenu.hidePopover();this.scriptMore.focus();}}
  menuClosed(event:ToggleEvent):void{const root=this.getRootNode() as Document|ShadowRoot;if(event.newState==='closed'&&this.scriptMenu.contains(root.activeElement))this.scriptMore.focus();}
  closeComparison():void{this.diffVisible=false;this.disposeComparison();this.focusEditor();}
  private disposeComparison():void{this.revisionDiff?.dispose();this.revisionDiff=null;for(const model of this.diffModels)model.dispose();this.diffModels=[];}
  useSelectedTestRequest():void {
    const request=this.selection?.detail.requests.find(head=>head.boundary==='client-request');
    if(!request)return;
    this.scriptTestUrl.value=request.target??'';this.scriptTestMethod.value=request.method??'GET';
    this.scriptTestHeaders.value=request.headers.filter(header=>!header.sensitive).map(header=>header.name+': '+header.value).join('\n');
    this.scriptTestBody.value='';this.markTestChanged();
  }
  protected hydratedCallback():void {
    this.systemTheme.addEventListener('change',this.systemThemeChanged);
    this.forcedColors.addEventListener('change',this.systemThemeChanged);
    if(this.active)void this.ensureEditor();
  }
  activeChanged():void{if(this.active)void this.ensureEditor();}
  focusEditor():void{this.sourceEditor?.layout();this.sourceEditor?.focus();}
  themeChanged():void{this.monaco?.editor.setTheme(this.monacoTheme());}
  private monacoTheme():'vs'|'vs-dark'|'hc-black'|'hc-light'{
    const dark=this.theme==='dark'||(this.theme==='system'&&this.systemTheme.matches);
    return this.forcedColors.matches?(dark?'hc-black':'hc-light'):(dark?'vs-dark':'vs');
  }
  private async ensureEditor():Promise<void>{
    if(this.loading)return this.loading;
    if(this.sourceEditor)return;
    this.loading=(async()=>{
      try{this.monaco=(await loadMonaco()).monaco;await this.initializeScriptEditor();}
      catch(error:unknown){
        this.sourceEditor?.dispose();this.sourceEditor=null;this.draftModel?.dispose();this.draftModel=null;
        this.scriptText=`Script editor could not open: ${describeError(error)}`;this.statusKind='error';
        await this.reportFrontendIssue('script-editor-initialization-failed',error);
      }
    })();
    try{await this.loading;}finally{this.loading=null;}
  }
  disconnectedCallback():void {
    this.editVersion++;this.testVersion++;this.scriptReady=false;
    this.systemTheme.removeEventListener('change',this.systemThemeChanged);
    this.forcedColors.removeEventListener('change',this.systemThemeChanged);
    this.sourceEditor?.dispose();this.sourceEditor=null;this.draftModel?.dispose();this.draftModel=null;
    this.disposeComparison();this.declarationsDisposable?.dispose();this.declarationsDisposable=null;
    super.disconnectedCallback();
  }
  private async initializeScriptEditor():Promise<void>{
    if(this.sourceEditor!==null||this.scriptEditor===undefined)return;
    const [{ModuleKind,ModuleResolutionKind,ScriptTarget,typescriptDefaults},declarations,status]=await Promise.all([
      loadMonaco(),invoke<string>('script_declarations'),invoke<ScriptStatus>('script_status'),
    ]);
    if(!this.isConnected)return;
    this.editorLoaded=true;this.$flushUpdates();await waitForStyles(this.monacoStyles);
    if(!this.isConnected)return;
    typescriptDefaults.setCompilerOptions({allowNonTsExtensions:true,module:ModuleKind.ESNext,moduleResolution:ModuleResolutionKind.NodeJs,noEmit:true,strict:true,target:ScriptTarget.ESNext});
    typescriptDefaults.setDiagnosticsOptions({noSemanticValidation:false,noSyntaxValidation:false});
    this.declarationsDisposable=typescriptDefaults.addExtraLib(declarations,'file:///transmog-script-api/v1.d.ts');
    const draft=editingDraft(status);
    this.loadedDraft=draft??null;
    this.draftModel=this.monaco!.editor.createModel(draft?.source??SCRIPT_TEMPLATE,'typescript',this.monaco!.Uri.parse('file:///transmog-scripts/draft/main.ts'));
    this.sourceEditor=this.monaco!.editor.create(this.scriptEditor,{model:this.draftModel,automaticLayout:true,accessibilitySupport:'on',ariaLabel:'Traffic script TypeScript source',minimap:{enabled:false},tabFocusMode:true,theme:this.monacoTheme()});
    if(draft){
      this.scriptRevision=draft.revision;
      const set=(name:string,value:string|boolean)=>{const input=this.scriptForm.elements.namedItem(name) as HTMLInputElement;if(typeof value==='boolean')input.checked=value;else input.value=value;};
      set('scriptHost',draft.prefilter.host??'');set('scriptPath',draft.prefilter.pathPrefix??'');set('scriptHeaders',draft.capabilities.writeHeaders.join(', '));
      set('readBodies',draft.capabilities.readBodies);set('writeBody',draft.capabilities.writeBody);set('respond',draft.capabilities.respond);set('abort',draft.capabilities.abort);set('sensitive',draft.capabilities.readSensitiveHeaders);
      this.savedFingerprint=scriptFingerprint(draft);
    }
    this.sourceEditor.onDidChangeModelContent(()=>this.markScriptChanged());
    this.scriptReady=true;this.$flushUpdates();this.renderScriptStatus(status);this.updateDraftState();
    this.scriptText=draft?'Ready to edit. Changes apply to new requests when saved and enabled.':'Start with this example, then save and enable it for new requests.';
  }
  private async saveDraft():Promise<void>{
    const version=this.editVersion;
    try{
      const {draft,generation}=await this.prepareDraft();
      if(!this.current(version))return this.changedDuring('saving');
      const status=await invoke<ScriptStatus>('save_script',{draft,expectedGeneration:generation});
      if(!this.isConnected)return;
      this.savedFingerprint=scriptFingerprint(draft);this.candidate=null;this.renderScriptStatus(status);this.updateDraftState();
      this.feedback(this.current(version)?'Draft saved. The active script is unchanged.':'Earlier edits were saved. Newer edits are still unsaved.');
    }catch(error:unknown){this.scriptFailure('Save failed',error);await this.showError('Script draft could not be saved',this.scriptText);}
  }
  private current(version:number):boolean{return this.isConnected&&version===this.editVersion;}
  private changedDuring(operation:string):void {this.feedback(`The draft changed during ${operation}. Run the action again to use the current edits.`);}
  private async validateDraft():Promise<ValidatedDraft|null>{
    const version=this.editVersion;
    try{
      const {draft,generation}=await this.prepareDraft();
      if(!this.current(version)){this.changedDuring('validation');return null;}
      const token=await invoke<ScriptCandidate>('validate_script',{draft,expectedGeneration:generation});
      if(!this.current(version)){this.changedDuring('validation');return null;}
      this.candidate={token,draft,version};
      const model=this.sourceEditor?.getModel();if(model)this.monaco!.editor.setModelMarkers(model,'transmog-rust',[]);
      this.feedback('Current source and capabilities are valid. Save & enable applies them; sandbox testing is optional.');
      return this.candidate;
    }catch(error:unknown){if(this.current(version))this.scriptFailure('Validation failed',error);else this.changedDuring('validation');return null;}
  }
  private async testDraft():Promise<void>{
    const testVersion=this.testVersion;
    const version=this.editVersion;
    this.testOutdated=Boolean(this.testResult);
    const input={url:this.scriptTestUrl.value,method:this.scriptTestMethod.value,headers:this.scriptTestHeaders.value,body:this.scriptTestBody.value,handler:this.scriptTestHandler.value};
    try{
      const candidate=this.candidate??await this.validateDraft();
      if(!candidate||!this.current(version))return;
      const handler=candidate.draft.handlers.find(name=>name===input.handler)??candidate.draft.handlers[0];
      const target=new URL(input.url);if(!['http:','https:'].includes(target.protocol))throw new Error('Use an HTTP or HTTPS test URL');
      if(!handler)throw new Error('Export at least one supported handler');
      this.scriptText='Running the sandbox for this input…';this.statusKind='progress';
      const action=await invoke<Record<string,unknown>>('test_script',{candidateId:candidate.token.candidateId,invocation:{
        context:{exchangeId:'editor-test',nowUnixMs:0,handler},
        request:{method:input.method,scheme:target.protocol.slice(0,-1),host:target.hostname,port:Number(target.port||(target.protocol==='https:'?443:80)),path:target.pathname,query:target.search?target.search.slice(1):null,
          headers:parseHeaderLines(input.headers).map(header=>({name:header.name,value:Array.from(new TextEncoder().encode(header.value)),sensitive:false}))},
        response:handler.startsWith('onResponse')?{status:200,headers:[]}:null,
        body:handler.endsWith('Body')?{bytes:Array.from(new TextEncoder().encode(input.body)),truncated:false}:null,
      }});
      if(!this.isConnected)return;
      this.testOutdated=!this.current(version)||testVersion!==this.testVersion;
      this.testResult=`Completed for this input · ${handler} · Action: ${String(action.action??'continue')}`;
      this.testEvidence=`${input.method} ${input.url}\n${handler.startsWith('onResponse')?'Response fixture: 200, no response headers\n':''}${input.headers?`Request headers:\n${input.headers}\n`:''}${handler.endsWith('Body')?`Input body:\n${input.body}\n`:''}\nReturned action:\n${JSON.stringify(action,null,2)}`;
      if(this.testEvidence.length>32768)this.testEvidence=this.testEvidence.slice(0,32768)+'\n[Evidence preview shortened to 32,768 characters]';
      this.feedback(this.testOutdated?'Sandbox completed for earlier edits or input. Run it again to check the current version.':'Sandbox completed for this input. No request was sent; the script was not enabled.');
    }catch(error:unknown){if(this.current(version)&&testVersion===this.testVersion){this.testResult='Sandbox did not complete for this input';this.testEvidence=describeError(error);this.testOutdated=false;this.scriptFailure('Sandbox test aborted',error);}else this.feedback('The earlier sandbox test failed after its draft or input changed. Test the current version.');}
  }
  private async activateDraft():Promise<void>{
    const version=this.editVersion;
    try{
      // Always revalidate against the current workspace generation, including after Save draft.
      const candidate=await this.validateDraft();
      if(!candidate||!this.current(version))return;
      this.scriptText='Starting the script and saving this version…';this.statusKind='progress';
      const status=await invoke<ScriptStatus>('activate_script',{candidateId:candidate.token.candidateId});
      if(!this.isConnected)return;
      this.candidate=null;this.savedFingerprint=scriptFingerprint(candidate.draft);this.scriptRevision=candidate.draft.revision+1;
      this.renderScriptStatus(status);this.updateDraftState();
      this.feedback(this.current(version)?'Saved and enabled for new requests.':'The validated version was saved and enabled. Newer edits are still unsaved and unapplied.');
    }catch(error:unknown){this.scriptFailure('Activation failed',error);}
  }
  private async disableActive():Promise<void>{
    try{const status=await invoke<ScriptStatus>('disable_script',{scriptId:'user-script'});if(!this.isConnected)return;this.candidate=null;this.renderScriptStatus(status);this.updateDraftState();this.feedback('Script paused for new requests. Your draft is preserved.');}
    catch(error:unknown){this.scriptFailure('Pause failed',error);}
  }
  private async compareDraft():Promise<void>{
    const version=this.editVersion;
    try{
      const draft=this.scriptDraft();const status=await invoke<ScriptStatus>('script_status');
      if(!this.current(version)){this.changedDuring('comparison');return;}
      const active=status.active.find(revision=>revision.manifest.id===draft.id);
      if(!active)throw new Error('This script is not currently enabled');
      this.disposeComparison();this.diffModels=[this.monaco!.editor.createModel(active.source,'typescript'),this.monaco!.editor.createModel(draft.source,'typescript')];
      this.activeSettings=scriptSettings(activeDraft(active));this.draftSettings=scriptSettings(draft);this.diffStale=false;
      this.diffVisible=true;this.$flushUpdates();
      this.revisionDiff=this.monaco!.editor.createDiffEditor(this.scriptDiffEditor,{automaticLayout:true,accessibilitySupport:'on',ariaLabel:'Enabled script and current draft comparison',readOnly:true,theme:this.monacoTheme()});
      const [original,modified]=this.diffModels;if(!original||!modified)throw new Error('Script comparison could not be prepared');
      this.revisionDiff.setModel({original,modified});this.feedback('Comparing active source and settings with the current draft.');
    }catch(error:unknown){this.scriptFailure('Script comparison unavailable',error);}
  }
  private async prepareDraft():Promise<{draft:ScriptDraft;generation:number}>{
    const draft=this.scriptDraft();const status=await invoke<ScriptStatus>('script_status');
    const revisions=[...status.saved.filter(item=>item.id===draft.id).map(item=>item.revision),...status.active.filter(item=>item.manifest.id===draft.id).map(item=>item.manifest.revision+1)];
    const revision=Math.max(this.scriptRevision,...revisions);if(!Number.isSafeInteger(revision))throw new Error('Script revision limit reached');
    this.scriptRevision=revision;return {draft:{...draft,revision},generation:status.generation};
  }
  private scriptDraft():ScriptDraft{
    if(!this.sourceEditor)throw new Error('Script editor is still loading');
    const data=new FormData(this.scriptForm);const source=this.sourceEditor.getValue();
    const handlers=(['onRequestHead','onRequestBody','onResponseHead','onResponseBody'] as const).filter(handler=>new RegExp(`\\bexport\\s+function\\s+${handler}\\b`).test(source));
    const writeHeaders=[...new Set(String(data.get('scriptHeaders')??'').split(',').map(value=>value.trim().toLowerCase()).filter(value=>value.length>0))];
    return {id:'user-script',revision:this.scriptRevision,source,handlers,
      prefilter:{method:this.loadedDraft?.prefilter.method??null,host:optionalText(data.get('scriptHost')),pathPrefix:optionalText(data.get('scriptPath'))},
      capabilities:{readSensitiveHeaders:data.get('sensitive')==='on',readBodies:data.get('readBodies')==='on',writeHeaders,writeBody:data.get('writeBody')==='on',respond:data.get('respond')==='on',abort:data.get('abort')==='on'},
      limits:this.loadedDraft?.limits??{maxInputBodyBytes:1024*1024,maxOutputBytes:1024*1024,maxLogBytes:16*1024,maxHeapBytes:64*1024*1024,maxDurationMs:100},priority:this.loadedDraft?.priority??0};
  }
  private renderScriptStatus(status:ScriptStatus):void{
    this.$emit('script-activity-change',{generation:status.generation,activeCount:status.active.length});
    const active=status.active.find(item=>item.manifest.id==='user-script');this.scriptActive=Boolean(active);this.activeScriptText=active?'On':'Paused';
    const fingerprint=active?scriptFingerprint(activeDraft(active)):'';
    if(this.diffVisible&&fingerprint!==this.activeFingerprint)this.diffStale=true;
    this.activeFingerprint=fingerprint;
  }
  private feedback(message:string):void{this.scriptText=message;this.statusKind='success';}
  private scriptFailure(prefix:string,error:unknown):void{
    this.candidate=null;const message=describeError(error);
    const conflict=/revision must increase|candidate is unknown|workspace changed|newer script draft/.test(message);
    const recovery=conflict?'The workspace changed. Run the action again to validate the current edits.':prefix==='Validation failed'||prefix==='Sandbox test aborted'?'Review the source, capabilities, and test input, then retry.':'Your draft and the previously active script are preserved. Try the action again.';
    this.scriptText=`${prefix}: ${message}\n\n${recovery}`;this.statusKind='error';
    if(prefix!=='Validation failed')return;
    const model=this.sourceEditor?.getModel();if(model)this.monaco!.editor.setModelMarkers(model,'transmog-rust',[{severity:this.monaco!.MarkerSeverity.Error,message,startLineNumber:1,startColumn:1,endLineNumber:1,endColumn:Math.max(2,model.getLineMaxColumn(1))}]);
  }
}
ScriptEditor.define('script-editor');
