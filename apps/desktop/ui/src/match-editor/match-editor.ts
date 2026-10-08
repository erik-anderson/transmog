import { observable } from '@microsoft/webui-framework';
import { WorkspaceElement } from '../workspace-element.js';
import type { QueryCondition, UrlCondition } from '../models.js';

export class MatchEditor extends WorkspaceElement {
  @observable condition:UrlCondition|null=null;
  @observable matchMode='exact';
  @observable queryMode='none';
  @observable regexScope='url';
  @observable queryHidden=true;
  @observable patternParts:Array<{text:string;token:boolean}>=[];
  @observable selectedPart='';
  @observable annotationAvailable=false;
  @observable matchEditorError='';
  @observable matchValidationError='';
  @observable annotationX='12px';
  @observable annotationY='80px';
  addressInput!:HTMLInputElement;
  queryInput!:HTMLTextAreaElement;
  caseInput!:HTMLInputElement;
  wholeInput!:HTMLInputElement;
  annotationInput!:HTMLInputElement;
  annotationType!:HTMLSelectElement;
  annotationPopover!:HTMLElement;
  private initialized=false;
  private selection:{start:number;end:number;token:boolean}|null=null;
  protected hydratedCallback():void {this.initialized=true;this.conditionChanged();}
  conditionChanged():void {
    if(!this.initialized) return;
    const condition=this.condition;
    this.matchValidationError='';this.matchEditorError='';
    this.matchMode=condition?.kind??'any';
    this.addressInput.value=condition?.kind==='exact'?condition.value:condition?.kind==='pattern'?condition.value.address:condition?.value.pattern??'';
    this.caseInput.checked=condition?.kind==='exact'?true:condition?.value.caseSensitive??true;
    this.wholeInput.checked=condition?.kind==='regex'?condition.value.whole:true;
    this.regexScope=condition?.kind==='regex'?condition.value.scope:'url';
    const query=condition?.kind==='pattern'?condition.value.query:condition?.kind==='regex'?condition.value.query:null;
    this.queryMode=query?.kind==='ignore'?'ignore':query?.kind==='parameters'?'parameters':query?.kind==='exact' && query.value!==null?'exact':'none';
    this.queryInput.value=query?.kind==='parameters'?query.value.map(item=>`${item.name}=${item.value}`).join('\n'):query?.kind==='exact'?query.value??'':'';
    this.renderContext();
  }
  get value():UrlCondition|null {
    if(this.matchMode==='any')return null;
    const query:QueryCondition=this.queryMode==='ignore'?{kind:'ignore'}:this.queryMode==='parameters'?{kind:'parameters',value:this.queryInput.value.split(/\r?\n/).filter(Boolean).map(line=>{const index=line.indexOf('=');if(index<1)throw new Error('Enter each required parameter as name=value.');return {name:line.slice(0,index).trim(),value:line.slice(index+1)};})}:{kind:'exact',value:this.queryMode==='none'?null:this.queryInput.value};
    if(this.matchMode==='exact') return {kind:'exact',value:this.addressInput.value};
    if(this.matchMode==='pattern') return {kind:'pattern',value:{address:this.addressInput.value,query,caseSensitive:this.caseInput.checked}};
    return {kind:'regex',value:{pattern:this.addressInput.value,scope:this.regexScope as 'url'|'path',whole:this.wholeInput.checked,caseSensitive:this.caseInput.checked,query:this.regexScope==='path'?query:null}};
  }
  changeMode(event:Event):void {
    const mode=(event.target as HTMLSelectElement).value;
    if(this.matchMode==='exact' && mode==='pattern') {
      const index=this.addressInput.value.indexOf('?');
      if(index>=0) {this.queryMode='exact';this.queryInput.value=this.addressInput.value.slice(index+1);this.addressInput.value=this.addressInput.value.slice(0,index);}
      else this.queryMode='none';
    } else if(this.matchMode==='pattern' && mode==='exact') {
      if(this.queryMode==='exact') this.addressInput.value+='?'+this.queryInput.value;
      if(this.queryMode==='ignore' || this.queryMode==='parameters' || this.addressInput.value.includes('{')) {this.matchEditorError='Exact mode treats every character literally. Review the URL before saving.';}
    } else if(mode==='regex' && this.matchMode!=='regex') {
      // Preserve literal matching on conversion; generalization is always explicit.
      if(this.matchMode==='pattern') {this.matchEditorError='Enter a regular expression; URL placeholders are specific to Pattern mode.';}
      else this.addressInput.value=this.addressInput.value.replace(/[.*+?^${}()|[\]\\]/g,'\\$&');
    }
    this.matchMode=mode;this.renderContext();this.notifyChange();
  }
  changeQuery(event:Event):void {this.queryMode=(event.target as HTMLSelectElement).value;this.notifyChange();}
  changeScope(event:Event):void {this.regexScope=(event.target as HTMLSelectElement).value;this.renderContext();this.notifyChange();}
  changed():void {
    if(this.matchMode==='pattern') {
      const index=this.addressInput.value.indexOf('?');
      if(index>=0) {this.queryMode='exact';this.queryInput.value=this.addressInput.value.slice(index+1);this.addressInput.value=this.addressInput.value.slice(0,index);}
    }
    this.matchEditorError='';this.renderContext();this.notifyChange();
  }
  private notifyChange():void {this.$emit('matcher-change');}
  private renderContext():void {
    this.queryHidden=this.matchMode==='any' || this.matchMode==='exact' || (this.matchMode==='regex' && this.regexScope==='url');
    const text=this.addressInput.value;const parts:Array<{text:string;token:boolean}>=[];
    let previous=0;
    if(this.matchMode==='pattern') for(const match of text.matchAll(/\{[^{}]*\}/g)) {parts.push({text:text.slice(previous,match.index),token:false},{text:match[0],token:true});previous=match.index!+match[0].length;}
    parts.push({text:text.slice(previous),token:false});this.patternParts=parts;this.inspectSelection();
  }
  inspectSelection():void {
    const input=this.addressInput;const start=input.selectionStart??0;const end=input.selectionEnd??start;
    this.selection=null;this.selectedPart='';this.annotationAvailable=false;
    if(this.matchMode!=='pattern') return;
    for(const match of input.value.matchAll(/\{[^{}]*\}/g)) {
      if(start>=match.index! && end<=match.index!+match[0].length) {this.selection={start:match.index!,end:match.index!+match[0].length,token:true};this.annotationAvailable=true;return;}
    }
    const pathStart=input.value.indexOf('/',input.value.indexOf('://')+3);
    if(end>start && start>pathStart && input.value[start-1]==='/' && (end===input.value.length || input.value[end]==='/')) {this.selection={start,end,token:false};this.selectedPart=input.value.slice(start,end);}
  }
  generalize(kind:string):void {
    const selected=this.selection;if(!selected)return;
    this.addressInput.setRangeText(kind==='digits'?'{:digits}':kind==='rest'?'{...}':'{}',selected.start,selected.end,'select');
    this.addressInput.focus();this.changed();
  }
  openAnnotation():void {
    const selected=this.selection;if(!selected?.token)return;
    const token=this.addressInput.value.slice(selected.start+1,selected.end-1);
    this.annotationType.value=token.endsWith('...')?'rest':token.endsWith(':digits')?'digits':'segment';
    this.annotationInput.value=token.endsWith('...')?token.slice(0,-3):token.split(':')[0]??'';
    const rect=this.addressInput.getBoundingClientRect();this.annotationX=Math.max(10,Math.min(rect.left,window.innerWidth-420))+'px';this.annotationY=Math.max(50,Math.min(rect.bottom+4,window.innerHeight-300))+'px';this.$flushUpdates();this.annotationPopover.showPopover();this.annotationInput.focus();
  }
  clearAnnotationError():void {this.annotationInput.setCustomValidity('');}
  annotationKeyboard(event:KeyboardEvent):void {if(event.key==='Enter'){event.preventDefault();this.saveAnnotation();}}
  saveAnnotation():void {
    const selected=this.selection;if(!selected)return;
    const label=this.annotationInput.value.trim();
    if(!/^[a-zA-Z0-9_]*$/.test(label)) {this.annotationInput.setCustomValidity('Use letters, digits and underscores, or leave empty.');this.annotationInput.reportValidity();return;}
    this.annotationInput.setCustomValidity('');
    const type=this.annotationType.value;
    this.addressInput.setRangeText('{'+label+(type==='digits'?':digits':type==='rest'?'...':'')+'}',selected.start,selected.end,'select');
    this.annotationPopover.hidePopover();this.addressInput.focus();this.changed();
  }
}
MatchEditor.define('match-editor');
