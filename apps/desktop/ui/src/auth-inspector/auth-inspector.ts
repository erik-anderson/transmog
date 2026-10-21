import {WebUIElement,attr,observable} from '@microsoft/webui-framework';
import {invoke} from '@tauri-apps/api/core';
import {listen,type UnlistenFn} from '@tauri-apps/api/event';
import type {AuthField,AuthFinding,AuthRelated,AuthRelatedEntry,AuthSelection,AuthSourceNavigation,AuthView} from './models.js';
import {copyGesture, copyTree, fieldCopyActions, requestCopy, CopyError, type CopyTreeNode} from '../inspection-copy.js';
import {selectedConverterText} from '../converter-selection.js';

let generation=0n;
const next=():string=>(++generation).toString();
const group=(finding:AuthFinding):AuthFinding=>{
  const children=finding.children.map(group);
  const fields=finding.fields.map(field=>({...field,revealed:!!field.revealed,visibilityLabel:(field.revealed?'Hide ':'Reveal ')+field.name}));
  const summary=finding.fields.filter(field=>!field.sensitive&&/^(iss|aud|Username|Domain|Target name|Ticket realm|Service principal|client_id|scope|response_type)/.test(field.name)).slice(0,4).map(field=>field.name+': '+field.value.slice(0,180)).join(' · ')||children.find(child=>child.summary)?.summary||'';
  return {...finding,fields,summary,readableFields:fields.filter(field=>!technical(field)),technicalFields:fields.filter(technical),children};
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
  @observable authCoverage:AuthView['inspection']['coverage']=[];
  @observable authDiagnostics:string[]=[];
  @observable authRelated:AuthRelated|null=null;
  @observable authRelatedBusy=false;
  @observable authInterpretation='auto';
  private view:AuthView|null=null;
  private pending:{slot:string;generation:string;pageId:string}|null=null;
  private key='';
  private revision=0;
  private queued=false;
  private ready=false;
  private suspended=false;
  private nativeUnlisten:UnlistenFn|null=null;
  private nativeListener=0;
  private timer:ReturnType<typeof setTimeout>|null=null;
  private readonly visibility=():void=>{if(!document.hidden)this.suspended=false;this.schedule();};
  selectionChanged(previous:AuthSelection|null|undefined):void {if(previous?.sessionId!==this.selection?.sessionId||previous?.boundary!==this.selection?.boundary)this.authInterpretation='auto';this.schedule();}
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
    const key=this.isConnected&&this.visible&&!document.hidden&&!this.suspended&&selection?JSON.stringify([this.slot,selection.key,selection.sessionId,selection.boundary,this.active,this.authInterpretation]):'';
    if(this.key===key)return;
    this.stop();this.key=key;
    if(!key||!selection)return;
    // Coalesce live body/head updates before admitting any source work.
    this.authBusy=true;const revision=this.revision;
    this.timer=setTimeout(()=>{this.timer=null;void this.inspect(selection,revision);},60);
  }
  private stop():void {
    this.revision++;if(this.timer!==null){clearTimeout(this.timer);this.timer=null;}
    const pending=this.pending;this.pending=null;
    if(pending)void invoke('cancel_auth',{slot:pending.slot,generation:next(),pageId:pending.pageId}).catch(()=>undefined);
    this.view=null;this.authFindings=[];this.authCoverage=[];this.authDiagnostics=[];this.authRelated=null;this.authBusy=false;this.authRelatedBusy=false;this.authError='';this.authNotice='';
    this.$emit('auth-count',{count:null});
  }
  private current(revision:number):boolean {return this.isConnected&&this.visible&&!document.hidden&&revision===this.revision;}
  private async inspect(selection:AuthSelection,revision:number):Promise<void> {
    try {
      const pageId=await invoke<string>('visualizer_page');
      if(!this.current(revision))return;
      const request={slot:this.slot,generation:next(),pageId,sessionId:selection.sessionId,boundary:selection.boundary,scanBody:this.active,interpretation:this.active?this.authInterpretation:'auto'};
      this.pending=request;
      const view=await invoke<AuthView>('inspect_auth',{request});
      if(!this.current(revision)||this.pending?.generation!==request.generation)return;
      this.view=view;this.authFindings=view.inspection.findings.map(group);this.authCoverage=view.inspection.coverage;this.authDiagnostics=view.inspection.diagnostics;
      this.$emit('auth-count',{count:this.authFindings.length,complete:this.authCoverage.every(source=>source.status==='complete')});
    } catch {if(this.current(revision)){this.authError='Auth evidence could not be inspected or changed while loading. Retry the current selection.';this.view=null;}}
    finally {if(this.current(revision))this.authBusy=false;}
  }
  retry():void {this.suspended=false;this.key='';this.reconcile();}
  changeInterpretation(event:Event):void {this.authInterpretation=(event.currentTarget as HTMLSelectElement).value;this.key='';this.reconcile();}
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
      actions: [...fieldCopyActions(finding.format + ' · ' + finding.role, null),
        ...(finding.fields.length || finding.children.length ? [{id: 'subtree', label: 'Copy subtree', read: (current: () => boolean) => subtree(false, current)}] : []),
        ...(hasSecrets(finding) ? [{id: 'secrets', label: 'Copy subtree including secrets', read: (current: () => boolean) => subtree(true, current)}] : [])]});
  }
  async related():Promise<void> {
    const view=this.view;if(!view||this.authRelatedBusy)return;const revision=this.revision;this.authRelatedBusy=true;
    try {const related=await invoke<AuthRelated>('related_auth',{handle:view.handle});if(this.current(revision)&&this.active)this.authRelated=related;}
    catch {if(this.current(revision))this.authNotice='Related evidence is unavailable or changed. Retry inspection.';}
    finally {if(this.current(revision))this.authRelatedBusy=false;}
  }
  navigate(entry:AuthRelatedEntry):void {const revision=this.revision;if(!this.view||!this.current(revision))return;this.$emit('auth-traffic-request',{id:entry.sessionId,boundary:entry.boundary,isCurrent:()=>this.current(revision)});}
  disconnectedCallback():void {this.ready=false;this.nativeListener++;const unlisten=this.nativeUnlisten;this.nativeUnlisten=null;if(unlisten)void Promise.resolve(unlisten()).catch(()=>undefined);document.removeEventListener('visibilitychange',this.visibility);this.stop();this.key='';super.disconnectedCallback();}
}
AuthInspector.define('auth-inspector');
