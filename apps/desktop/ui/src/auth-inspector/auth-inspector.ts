import {WebUIElement,attr,observable} from '@microsoft/webui-framework';
import {invoke} from '@tauri-apps/api/core';
import {listen,type UnlistenFn} from '@tauri-apps/api/event';
import type {AuthField,AuthFinding,AuthRelated,AuthRelatedEntry,AuthSelection,AuthSourceNavigation,AuthView} from './models.js';
import {authBodyFormats} from './models.js';
import {copyGesture, copyTree, fieldCopyActions, nameCopyAction, requestCopy, CopyError, type CopyTreeNode} from '../inspection-copy.js';
import {selectedConverterText} from '../converter-selection.js';
import {authArtifact,type AuthArtifact} from './presentation.js';

let generation=0n;
const next=():string=>(++generation).toString();
const group=(finding:AuthFinding):AuthFinding=>{
  const children=finding.children.map(group);
  const fields=finding.fields.map(field=>({...field,revealed:!!field.revealed,visibilityLabel:(field.revealed?'Hide ':'Reveal ')+field.name}));
  return {...finding,fields,readableFields:fields.filter(field=>!technical(field)),technicalFields:fields.filter(technical),children};
};
const technical=(field:AuthField):boolean=>/^(Original |Decoded |Compact token)/.test(field.name)||field.name.includes(' · original field');
export class AuthInspector extends WebUIElement {
  @attr slot='message-auth';
  @attr({mode:'boolean'}) active=false;
  @attr({mode:'boolean'}) visible=false;
  @observable selection:AuthSelection|null=null;
  @observable authBusy=false;
  @observable authError='';
  @observable authNotice='';
  @observable authFindings:AuthFinding[]=[];
  @observable authArtifacts:AuthArtifact[]=[];
  authFindingsChanged():void {this.authArtifacts=this.authFindings.map(authArtifact);}
  @observable authCoverage:AuthView['inspection']['coverage']=[];
  @observable authCoverageSummary='';
  authCoverageChanged():void {
    const complete=this.authCoverage.filter(source=>source.status==='complete').length,total=this.authCoverage.length;
    this.authCoverageSummary=!total?'Source coverage unavailable':complete===total?'All '+total+' scanned sources complete':complete+' of '+total+' scanned sources complete · limited coverage';
  }
  @observable authDiagnostics:string[]=[];
  @observable authRelated:AuthRelated|null=null;
  @observable authRelatedBusy=false;
  @observable interpretation='auto';
  @observable authBodyFormat='';
  @observable authBodyHint='';
  @observable authBodyResult='';
  private view:AuthView|null=null;
  private returnPosition:{key:string;interpretation:string;top:number}|null=null;
  private relatedOriginTop=0;
  relatedButton!:HTMLButtonElement;
  closeRelated():void {this.authRelated=null;this.$flushUpdates();const scroll=this.querySelector('.auth-scroll');if(scroll)scroll.scrollTop=this.relatedOriginTop;this.relatedButton.focus({preventScroll:true});}
  restorePosition(top:number):void {if(this.selection){this.returnPosition={key:this.selection.key,interpretation:this.interpretation,top};this.applyPosition();}}
  private applyPosition():void {
    const position=this.returnPosition;if(!position||this.authBusy||!this.view)return;
    this.returnPosition=null;
    if(position.key!==this.selection?.key||position.interpretation!==this.interpretation)return;
    this.$flushUpdates();const scroll=this.querySelector('.auth-scroll');if(scroll)scroll.scrollTop=position.top;
  }
  private pending:{slot:string;generation:string;pageId:string}|null=null;
  private key='';
  private evidenceKey='';
  private revision=0;
  private queued=false;
  private ready=false;
  private suspended=false;
  private nativeUnlisten:UnlistenFn|null=null;
  private nativeListener=0;
  private timer:ReturnType<typeof setTimeout>|null=null;
  private readonly visibility=():void=>{if(!document.hidden)this.suspended=false;this.schedule();};
  selectionChanged():void {this.schedule();}
  interpretationChanged():void {
    const format=authBodyFormats[this.interpretation];
    this.authBodyFormat=format?.label??'';
    this.authBodyHint=format?'This viewer expects the entire decoded body to be '+format.expected+'. Auth fields inside JSON, forms or HTML are detected automatically.':'';
    this.schedule();
  }
  activeChanged():void {if(this.active)this.suspended=false;this.schedule();}
  visibleChanged():void {this.schedule();}
  slotChanged():void {this.schedule();}
  protected hydratedCallback():void {
    this.ready=true;document.addEventListener('visibilitychange',this.visibility);const listener=++this.nativeListener;
    void listen('visualizers-suspended',()=>{this.suspended=true;this.stop();this.key='';this.authError='Auth inspection paused while the window was hidden. Retry inspection.';}).then(unlisten=>{if(this.isConnected&&listener===this.nativeListener)this.nativeUnlisten=unlisten;else void Promise.resolve(unlisten()).catch(()=>undefined);}).catch(()=>undefined);
    this.schedule();
  }
  private schedule():void {
    if(!this.ready||this.queued)return;this.queued=true;
    queueMicrotask(()=>{this.queued=false;this.reconcile();});
  }
  private reconcile():void {
    const selection=this.selection;
    const evidenceKey=selection?JSON.stringify([this.slot,selection.key,selection.sessionId,selection.boundary,this.active?this.interpretation:'auto']):'';
    const key=this.isConnected&&this.visible&&!document.hidden&&!this.suspended&&selection?JSON.stringify([evidenceKey,this.active]):'';
    if(this.key===key)return;
    this.stop(!key||evidenceKey!==this.evidenceKey);this.key=key;this.evidenceKey=evidenceKey;
    if(!key||!selection)return;
    // Coalesce live body/head updates before admitting any source work.
    this.authBusy=true;const revision=this.revision;
    this.timer=setTimeout(()=>{this.timer=null;void this.inspect(selection,revision);},60);
  }
  private stop(resetCount=true):void {
    this.revision++;if(this.timer!==null){clearTimeout(this.timer);this.timer=null;}
    const pending=this.pending;this.pending=null;
    if(pending)void invoke('cancel_auth',{slot:pending.slot,generation:next(),pageId:pending.pageId}).catch(()=>undefined);
    this.view=null;this.authFindings=[];this.authCoverage=[];this.authDiagnostics=[];this.authRelated=null;this.authBusy=false;this.authRelatedBusy=false;this.authError='';this.authNotice='';this.authBodyResult='';
    if(resetCount)this.$emit('auth-count',{count:null});
  }
  private current(revision:number):boolean {return this.isConnected&&this.visible&&!document.hidden&&revision===this.revision;}
  private async inspect(selection:AuthSelection,revision:number):Promise<void> {
    try {
      const pageId=await invoke<string>('visualizer_page');
      if(!this.current(revision))return;
      const request={slot:this.slot,generation:next(),pageId,sessionId:selection.sessionId,boundary:selection.boundary,scanBody:true,interpretation:this.active?this.interpretation:'auto'};
      this.pending=request;
      const view=await invoke<AuthView>('inspect_auth',{request});
      if(!this.current(revision)||this.pending?.generation!==request.generation)return;
      const format=authBodyFormats[request.interpretation];
      const body=view.inspection.findings.filter(finding=>finding.source.kind==='body');
      const rejected=(finding:AuthFinding):boolean=>!!format&&finding.source.kind==='body'&&finding.format==='Explicit auth interpretation';
      this.view=view;this.authFindings=view.inspection.findings.filter(finding=>!rejected(finding)).map(group);this.authCoverage=view.inspection.coverage;this.authDiagnostics=view.inspection.diagnostics;
      if(format)this.authBodyResult=body.some(rejected)?'No '+format.label+' recognized in this body.':
        body.length?'Body inspected as '+format.label+'. Review the decoded fields and status below.':
          'Body could not be interpreted as '+format.label+'. '+(this.authCoverage.find(source=>source.source==='body')?.detail??'No retained body was available.');
      this.$emit('auth-count',{count:this.authFindings.length,complete:this.authCoverage.every(source=>source.status==='complete')});
    } catch {if(this.current(revision)){this.authError='Auth evidence could not be inspected or changed while loading. Retry the current selection.';this.view=null;this.$emit('auth-count',{count:null,failed:true});}}
    finally {if(this.current(revision)){this.authBusy=false;this.applyPosition();}}
  }
  retry():void {this.suspended=false;this.key='';this.reconcile();}
  closeBody():void {this.$emit('auth-body-close',null);}
  private field(id:string,findings=this.authFindings):AuthField|undefined {for(const finding of findings){const field=finding.fields.find(field=>field.id===id);if(field)return field;const child=this.field(id,finding.children);if(child)return child;}return undefined;}
  private changeField(id:string,change:Partial<AuthField>):void {
    const update=(findings:AuthFinding[]):AuthFinding[]=>findings.map(finding=>({...finding,fields:finding.fields.map(field=>field.id===id?{...field,...change}:field),children:update(finding.children)}));
    this.authFindings=update(this.authFindings).map(group);
  }
  async reveal(id:string):Promise<void> {
    const view=this.view,field=this.field(id);if(!view||!field||field.busy)return;
    if(field.revealed){this.changeField(id,{revealed:false,value:'••••••••',error:''});return;}
    const revision=this.revision;this.changeField(id,{busy:true,error:''});
    try {const value=await invoke<string>('auth_value',{handle:view.handle,field:id});if(this.current(revision)&&this.active&&this.view===view)this.changeField(id,{value,revealed:true});}
    catch {if(this.current(revision))this.changeField(id,{error:'Value is unavailable or the evidence changed. Retry inspection.'});}
    finally {if(this.current(revision))this.changeField(id,{busy:false});}
  }
  async source(id:string):Promise<void> {
    const view=this.view;if(!view)return;const revision=this.revision;
    try {const source=await invoke<AuthSourceNavigation>('auth_source',{handle:view.handle,finding:id});if(this.current(revision)&&this.active)this.$emit('auth-source',source);}
    catch {if(this.current(revision))this.authNotice='Original source is no longer available. Retry inspection.';}
  }
  fieldContext(id: string, event: MouseEvent | KeyboardEvent): void {
    if (!copyGesture(event) || selectedConverterText(event)) return;
    const view = this.view, field = this.field(id), revision = this.revision;
    if (!view || !field) return;
    requestCopy(event, {source: event.currentTarget as HTMLElement,
      current: () => this.current(revision) && this.active && this.view === view,
      note: !field.available ? 'The value was not retained or was redacted.' : field.sensitive ? 'Value actions copy the actual retained value, including sensitive material.' : undefined,
      actions: fieldCopyActions(field.name, field.available ? () => invoke<string>('auth_value', {handle: view.handle, field: id}) : null)});
  }
  findingContext(id: string, event: MouseEvent | KeyboardEvent): void {
    if (!copyGesture(event) || selectedConverterText(event)) return;
    const view = this.view, revision = this.revision;
    const find = (nodes: AuthFinding[]): AuthFinding | undefined => {
      for (const node of nodes) { if (node.id === id) return node; const child = find(node.children); if (child) return child; }
      return undefined;
    };
    const finding = find(this.authFindings); if (!view || !finding) return;
    const artifact=authArtifact(finding);
    const hasSecrets = (node: AuthFinding): boolean => node.fields.some(field => field.available && field.sensitive) || node.children.some(hasSecrets);
    const subtree = async (secrets: boolean, current: () => boolean): Promise<string> => {
      const fields: AuthField[] = [];
      const collect = (node: AuthFinding): void => { fields.push(...node.fields.filter(field => field.available && (secrets || !field.sensitive))); node.children.forEach(collect); };
      collect(finding);
      const values = await invoke<string[]>('auth_values', {handle: view.handle, fields: fields.map(field => field.id)});
      if (!current() || values.length !== fields.length) throw new CopyError('The Auth evidence changed. Open the copy menu again.');
      const actual = new Map(fields.map((field, index) => [field.id, values[index]!]));
      const tree = (node: AuthFinding): CopyTreeNode => ({name: node.format + ' · ' + node.role,
        children: [...node.fields.map(field => ({name: field.name, value: !field.available ? '[unavailable]' :
          field.sensitive && !secrets ? '[masked]' : actual.get(field.id)!})), ...node.children.map(tree)]});
      return copyTree(tree(finding));
    };
    requestCopy(event, {source: event.currentTarget as HTMLElement,
      current: () => this.current(revision) && this.active && this.view === view,
      actions: [nameCopyAction(artifact.format + ' · ' + artifact.context),
        ...(finding.fields.length || finding.children.length ? [{id: 'subtree', label: 'Copy subtree', read: (current: () => boolean) => subtree(false, current)}] : []),
        ...(hasSecrets(finding) ? [{id: 'secrets', label: 'Copy subtree including secrets', read: (current: () => boolean) => subtree(true, current)}] : [])]});
  }
  async related():Promise<void> {
    const view=this.view;if(!view||this.authRelatedBusy)return;const revision=this.revision;this.authRelatedBusy=true;
    const scroll=this.querySelector('.auth-scroll'),top=scroll?.scrollTop??0;
    if(!this.authRelated)this.relatedOriginTop=top;
    try {const related=await invoke<AuthRelated>('related_auth',{handle:view.handle});if(this.current(revision)&&this.active){const unmoved=scroll?.scrollTop===top;this.authRelated=related;this.$flushUpdates();if(unmoved&&scroll)scroll.scrollTop=0;}}
    catch {if(this.current(revision))this.authNotice='Related evidence is unavailable or changed. Retry inspection.';}
    finally {if(this.current(revision))this.authRelatedBusy=false;}
  }
  navigate(entry:AuthRelatedEntry):void {const revision=this.revision;if(!this.view||!this.selection||!this.current(revision))return;this.$emit('auth-traffic-request',{id:entry.sessionId,boundary:entry.boundary,isCurrent:()=>this.current(revision),origin:{id:this.selection.sessionId,boundary:this.selection.boundary,interpretation:this.interpretation,scrollTop:this.relatedOriginTop}});}
  disconnectedCallback():void {this.ready=false;this.nativeListener++;const unlisten=this.nativeUnlisten;this.nativeUnlisten=null;if(unlisten)void Promise.resolve(unlisten()).catch(()=>undefined);document.removeEventListener('visibilitychange',this.visibility);this.stop();this.key='';super.disconnectedCallback();}
}
AuthInspector.define('auth-inspector');
