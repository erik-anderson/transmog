import {WebUIElement,attr,observable} from '@microsoft/webui-framework';
import type * as Monaco from 'monaco-editor/editor/editor.api';
import {loadMonaco,waitForStyles} from '../editor-runtime.js';
import {describeError} from '../utilities.js';

export interface BodyCommit {value:string;save:boolean;}
export type BodyLanguage='plaintext'|'html'|'css'|'javascript'|'json';
let modelSequence=0;

export function responseBodyLanguage(mediaType:string,value:string):BodyLanguage {
  const mime=mediaType.split(';')[0]!.trim().toLowerCase();
  if(['text/html','application/xhtml+xml'].includes(mime))return 'html';
  if(mime==='text/css')return 'css';
  if(/(?:java|ecma)script/.test(mime))return 'javascript';
  if(mime==='application/json'||mime.endsWith('+json'))return 'json';
  const start=value.trimStart().slice(0,4096);
  if(/^<!doctype\s+html\b|^<(?:html|head|body|div|p|span|script|style)\b/i.test(start))return 'html';
  if(/^(?:[.#][\w-]+|body|html|:root)\s*\{[^}]*[\w-]+\s*:/s.test(start))return 'css';
  if(/^(?:["']use strict["'];|(?:export\s+)?(?:const|let|var|function|class)\b|import\s)/.test(start))return 'javascript';
  if(/^\{\s*"[^"\n]+"\s*:|^\[\s*(?:\{|\[|"|\d|true|false|null|\])/.test(start))return 'json';
  return 'plaintext';
}

/** One lazily allocated syntax editor for the active response-body dialog. */
export class ResponseBodyEditor extends WebUIElement {
  @attr theme='system';
  @observable bodyLanguage:BodyLanguage='plaintext';
  @observable bodyEditorReady=false;
  @observable bodyEditorLoaded=false;
  @observable bodyCanSave=false;
  @observable bodyBusy=false;
  @observable bodyFormatting=false;
  @observable bodyError='';
  @observable bodyEditorStatus='';
  surface!:HTMLDialogElement;
  fallbackInput!:HTMLTextAreaElement;
  editorHost!:HTMLDivElement;
  bodyEditorStyles!:HTMLLinkElement;
  languageSelect!:HTMLSelectElement;
  private editor:Monaco.editor.IStandaloneCodeEditor|null=null;
  private model:Monaco.editor.ITextModel|null=null;
  private monaco:typeof Monaco|null=null;
  private generation=0;
  private opener:HTMLElement|null=null;
  private systemTheme=matchMedia('(prefers-color-scheme: dark)');
  private highContrast=matchMedia('(forced-colors: active)');
  private themeListener=()=>this.themeChanged();

  protected hydratedCallback():void {this.systemTheme.addEventListener('change',this.themeListener);this.highContrast.addEventListener('change',this.themeListener);}
  open(value:string,language:BodyLanguage,canSave:boolean,opener:HTMLElement):void {
    this.disposeEditor();const generation=++this.generation;
    this.opener=opener;this.bodyLanguage=language;this.bodyCanSave=canSave;this.bodyBusy=false;this.bodyFormatting=false;this.bodyError='';
    this.bodyEditorStatus='Loading syntax editor…';this.$flushUpdates();this.fallbackInput.value=value;this.languageSelect.value=language;
    this.surface.showModal();this.fallbackInput.focus();void this.ensureEditor(generation);
  }
  private async ensureEditor(generation:number):Promise<void> {
    try {
      this.monaco=(await loadMonaco()).monaco;
      if(generation!==this.generation||!this.isConnected||!this.surface.open)return;
      this.bodyEditorLoaded=true;this.$flushUpdates();await waitForStyles(this.bodyEditorStyles);
      if(generation!==this.generation||!this.isConnected||!this.surface.open)return;
      const focusFallback=this.getRootNode() instanceof ShadowRoot?(this.getRootNode() as ShadowRoot).activeElement===this.fallbackInput:document.activeElement===this.fallbackInput;
      this.model=this.monaco.editor.createModel(this.fallbackInput.value,this.bodyLanguage,this.monaco.Uri.parse(`file:///transmog-responses/editor-${++modelSequence}/body.js`));
      this.bodyEditorReady=true;this.$flushUpdates();
      this.editor=this.monaco.editor.create(this.editorHost,{model:this.model,automaticLayout:true,minimap:{enabled:false},accessibilitySupport:'on',ariaLabel:'Response body text',tabFocusMode:true,theme:this.editorTheme(),wordWrap:'on'});
      this.bodyEditorStatus='Language affects highlighting and formatting. Content type is set in Response.';
      if(focusFallback)this.editor.focus();
    }catch(error:unknown){
      if(generation!==this.generation)return;
      this.disposeEditor();this.bodyEditorLoaded=false;this.bodyEditorStatus='Syntax editor could not load. You can still edit plain text.';
      this.bodyError=describeError(error);this.$flushUpdates();
    }
  }
  changeLanguage(event:Event):void {
    const language=(event.currentTarget as HTMLSelectElement).value as BodyLanguage;
    if(!['plaintext','html','css','javascript','json'].includes(language))return;
    this.bodyLanguage=language;if(this.model)this.monaco?.editor.setModelLanguage(this.model,language);
  }
  async format():Promise<void> {
    if(!this.editor||this.bodyBusy||this.bodyFormatting)return;
    const root=this.getRootNode(),opener=root instanceof ShadowRoot?root.activeElement:document.activeElement;
    const generation=this.generation;this.bodyFormatting=true;this.bodyError='';
    try {
      const action=this.editor.getAction('editor.action.formatDocument');
      if(!action)throw new Error('Formatting is unavailable for this language.');
      const deadline=Date.now()+10000;
      while(!action.isSupported()&&Date.now()<deadline){
        await new Promise<void>(resolve=>setTimeout(resolve,25));
        if(generation!==this.generation)return;
      }
      if(!action.isSupported())throw new Error('The formatter could not load. Try again.');
      this.editor.pushUndoStop();
      await action.run();
      if(generation===this.generation){
        this.editor.pushUndoStop();
        if((root instanceof ShadowRoot?root.activeElement:document.activeElement)===opener)this.editor.focus();
      }
    }catch(error:unknown){if(generation===this.generation)this.bodyError='Content could not be formatted: '+describeError(error);}
    finally{if(generation===this.generation)this.bodyFormatting=false;}
  }
  commit(save:boolean):void {
    if(this.bodyBusy||this.bodyFormatting||(save&&!this.bodyCanSave))return;
    const value=this.model?.getValue()??this.fallbackInput.value;
    if(value.length>16777216){this.bodyError='Body exceeds the text editor limit. Serve it from a file instead.';return;}
    this.$emit('body-commit',{value,save} satisfies BodyCommit);
  }
  keyboard(event:KeyboardEvent):void {
    if((event.ctrlKey||event.metaKey)&&event.key.toLowerCase()==='s'){event.preventDefault();if(this.bodyCanSave)this.commit(true);}
  }
  cancel(event?:Event):void {event?.preventDefault();if(!this.bodyBusy)this.close();}
  close():void {if(this.surface.open)this.surface.close();}
  closed():void {
    if(this.surface.open)return;
    this.generation++;this.disposeEditor();this.$emit('body-editor-closed');
    const opener=this.opener;this.opener=null;if(opener?.isConnected&&opener.getClientRects().length)opener.focus();
  }
  private disposeEditor():void {this.editor?.dispose();this.editor=null;this.model?.dispose();this.model=null;this.bodyEditorReady=false;}
  private editorTheme():string {const dark=this.theme==='dark'||(this.theme==='system'&&this.systemTheme.matches);return this.highContrast.matches?(dark?'hc-black':'hc-light'):(dark?'vs-dark':'vs');}
  themeChanged():void {this.monaco?.editor.setTheme(this.editorTheme());}
  bodyBusyChanged():void {this.editor?.updateOptions({readOnly:this.bodyBusy});}
  disconnectedCallback():void {this.generation++;this.disposeEditor();this.systemTheme.removeEventListener('change',this.themeListener);this.highContrast.removeEventListener('change',this.themeListener);super.disconnectedCallback();}
}
ResponseBodyEditor.define('response-body-editor');
