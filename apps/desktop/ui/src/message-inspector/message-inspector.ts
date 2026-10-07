import { WebUIElement, attr, observable } from '@microsoft/webui-framework';
import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import type { BodyInspection, HeadView, SessionDetail } from '../models.js';
import { describeError } from '../utilities.js';
import { formatBytes } from '../table-model.js';

const stages:Record<string,string> = {'client-request':'Original request','upstream-request':'Request sent to origin','upstream-response':'Origin response','client-response':'Response sent to client'};

export class MessageInspector extends WebUIElement {
  @attr side = 'response';
  @attr split = '35';
  @observable detail: SessionDetail | null = null;
  @observable mode = 'body';
  @observable boundaries: Array<{id:string;label:string}> = [];
  @observable boundary = '';
  @observable headerRows: Array<{id:string;name:string;value:string}> = [];
  @observable headSummary = 'Select an exchange';
  @observable bodyText = 'Select an exchange to preview its body.';
  @observable bodyFacts = '';
  @observable viewerLabel = 'Auto';
  @observable imageUrl = '';
  @observable detailsText = '';
  @observable loading = false;
  @observable viewer = 'auto';
  @observable headerShare = '35%';
  decodeInput!: HTMLInputElement;
  bodyLimit!: HTMLInputElement;
  private generation = 0;
  private previousId: string | null = null;

  sideChanged(): void { this.mode = this.side === 'request' ? 'headers' : 'body'; this.updateMessage(); }
  splitChanged(): void { this.headerShare = this.split+'%'; }
  detailChanged(): void {
    if (this.detail?.id !== this.previousId) { this.previousId = this.detail?.id ?? null; this.viewer = 'auto'; this.boundary = ''; this.imageUrl = ''; }
    this.updateMessage();
  }
  private heads(): HeadView[] { return this.side === 'request' ? this.detail?.requests ?? [] : this.detail?.responses ?? []; }
  private updateMessage(): void {
    const heads = this.heads();
    const expected = this.side === 'request' ? 'client-request' : 'client-response';
    this.boundaries = heads.map((head) => ({id:head.boundary,label:stages[head.boundary] ?? head.boundary}));
    if (!heads.some((head) => head.boundary === this.boundary)) this.boundary = heads.find((head) => head.boundary === expected)?.boundary ?? heads[0]?.boundary ?? expected;
    const head = heads.find((head) => head.boundary === this.boundary);
    this.headSummary = head ? this.side === 'request' ? (head.method ?? '')+' '+(head.target ?? '') : 'HTTP '+head.status+' · '+(stages[head.boundary] ?? head.boundary) : this.detail ? 'Waiting for '+this.side+' headers' : 'Select an exchange';
    this.headerRows = (head?.headers ?? []).map((header,index) => ({id:String(index),name:header.name,value:header.sensitive ? '[redacted]' : header.value}));
    this.detailsText = this.detail ? JSON.stringify({terminal:this.detail.terminal,route:this.detail.routeSelection,attempts:this.detail.routeAttempts,hookEffects:this.detail.hookEffects,diagnostics:this.detail.diagnostics,sequenceLoss:this.detail.sequenceLoss},null,2) : 'No exchange selected.';
    if (this.detail && (this.mode === 'body' || this.mode === 'split')) void this.inspectBody();
    else { this.generation++; this.bodyText = 'Select an exchange to preview its body.'; this.bodyFacts = ''; }
  }
  showMode(mode:string): void { this.mode = mode; if (mode === 'body' || mode === 'split') void this.inspectBody(); }
  changeBoundary(event:Event): void { this.boundary = (event.currentTarget as HTMLSelectElement).value; this.updateMessage(); }
  changeViewer(event:Event): void { this.viewer = (event.currentTarget as HTMLSelectElement).value; void this.inspectBody(); }
  resizeHeaders(event:CustomEvent<{value:number;committed:boolean}>): void {
    this.headerShare = event.detail.value+'%';
    this.$emit('inspector-resize',{side:this.side,...event.detail});
  }
  async inspectBody(): Promise<void> {
    const generation = ++this.generation;
    const body = this.detail?.storedBodies.find((body) => body.boundary === this.boundary);
    this.imageUrl = ''; this.loading = false;
    if (!this.detail) return;
    const head = this.heads().find((head) => head.boundary === this.boundary);
    this.bodyFacts = body ? (body.mediaType ?? 'Unknown content type')+' · '+formatBytes(body.retainedBytes)+' retained' : '';
    this.viewerLabel = this.viewer === 'auto' ? 'Auto' : this.viewer;
    if (this.side === 'response' && head?.status === 304 && !body?.retainedBytes) {
      this.bodyText = '304 Not Modified — no response body is expected. The client uses its cached representation.';
      this.viewerLabel = 'Auto · no body'; return;
    }
    if (!body || !body.retainedBytes) { this.bodyText = body?.observedBytes === 0 ? 'This '+this.side+' has no body.' : body?.reason ?? 'No '+this.side+' body was retained for this exchange.'; return; }
    if (['disabled','evicted','quota-omitted'].includes(body.availability)) { this.bodyText = body.reason ?? 'The retained body is '+body.availability+'.'; return; }
    this.loading = true; this.bodyText = 'Loading preview…';
    try {
      const inspection = await invoke<BodyInspection>('inspect_body',{request:{sessionId:this.detail.id,boundary:this.boundary,representation:this.viewer,decodeContent:this.decodeInput?.checked ?? true,offset:0,maxBytes:Number(this.bodyLimit?.value ?? 262144)}});
      if (generation !== this.generation || !this.isConnected) return;
      const labels:Record<string,string> = {'formatted-json':'JSON','original-text':'Text','bytes':'Hex','image':'Image','metadata':'Metadata','unavailable':'Unavailable'};
      this.viewerLabel = (this.viewer === 'auto' ? 'Auto · ' : '')+(labels[inspection.representation] ?? inspection.representation);
      this.bodyFacts = (inspection.metadata.mediaType ?? 'Unknown content type')+' · '+formatBytes(inspection.displayBytes)+(inspection.decoded ? ' · decoded' : '')+(inspection.truncated ? ' · preview truncated' : '');
      this.bodyText = inspection.display || inspection.warning || (inspection.representation === 'image' ? '' : 'No body content.');
      if (inspection.warning && inspection.display) this.bodyText = inspection.warning+'\n\n'+inspection.display;
      if (inspection.previewHandle) this.imageUrl = convertFileSrc('preview/'+inspection.previewHandle,'transmog-preview');
    } catch (error:unknown) {
      if (generation === this.generation) this.bodyText = 'Preview unavailable: '+describeError(error)+'. Try the Hex or Metadata viewer.';
    } finally { if (generation === this.generation) this.loading = false; }
  }
  async copyHeaders(): Promise<void> { await this.copy(this.headerRows.map((row) => row.name+': '+row.value).join('\r\n'),'Headers copied.'); }
  async copyBody(): Promise<void> { await this.copy(this.bodyText,'Body preview copied.'); }
  private async copy(text:string,message:string): Promise<void> {
    try { await navigator.clipboard.writeText(text); this.$emit('diagnostic',message); }
    catch { this.$emit('diagnostic','Select the text and use Copy.'); }
  }
  disconnectedCallback(): void { this.generation++; super.disconnectedCallback(); }
}
MessageInspector.define('message-inspector');
