import { WebUIElement, attr, observable } from '@microsoft/webui-framework';
import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import type { BodyInspection, HeadView, SessionDetail, StoredBodyMetadata } from '../models.js';
import { describeError } from '../utilities.js';
import { formatBytes } from '../table-model.js';
import type { HexPreview } from '../hex-viewer/hex-viewer.js';

const stages:Record<string,string> = {'client-request':'Original request','upstream-request':'Request sent to origin','upstream-response':'Origin response','client-response':'Response sent to client'};

export class MessageInspector extends WebUIElement {
  @attr side = 'response';
  @attr split = '35';
  @observable detail: SessionDetail | null = null;
  @observable mode = 'body';
  @observable boundaries: Array<{id:string;label:string}> = [];
  @observable boundary = '';
  @observable headerRows: Array<{id:string;name:string;value:string;bytes:number;size:string}> = [];
  @observable headersSummary = '';
  @observable largestFirst = false;
  @observable authorizationPresent = false;
  @observable proxyAuthorizationPresent = false;
  @observable sourceIp = '';
  @observable headSummary = 'Select an exchange';
  @observable bodyText = 'Select an exchange to preview its body.';
  @observable bodyFacts = '';
  @observable bodySummary = '';
  @observable viewerLabel = 'Auto';
  @observable imageUrl = '';
  @observable hexPreview: HexPreview | null = null;
  @observable detailsText = '';
  @observable loading = false;
  @observable viewer = 'auto';
  @observable decodeContent = true;
  @observable bodyMetadata:StoredBodyMetadata | null = null;
  @observable saving = false;
  @observable headerShare = '35%';
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
    this.bodyMetadata = this.detail?.storedBodies.find((body) => body.boundary === this.boundary) ?? null;
    this.headSummary = head ? this.side === 'request' ? (head.method ?? '')+' '+(head.target ?? '') : 'HTTP '+head.status+' · '+(stages[head.boundary] ?? head.boundary) : this.detail ? 'Waiting for '+this.side+' headers' : 'Select an exchange';
    this.headerRows = (head?.headers ?? []).map((header,index) => ({id:String(index),name:header.name,value:header.sensitive ? '[redacted]' : header.value,bytes:header.fieldBytes??0,size:header.fieldBytes===undefined?'Unavailable':formatBytes(header.fieldBytes)}));
    const measured = head?.headers.every(header=>header.fieldBytes!==undefined)??false;
    this.headersSummary=head?`${head.headers.length} fields · ${measured?formatBytes(this.headerRows.reduce((sum,row)=>sum+row.bytes,2))+' including final CRLF':'size unavailable'} · HTTP/1 equivalent`:'';
    this.authorizationPresent=head?.headers.some(header=>header.name.toLowerCase()==='authorization')??false;
    this.proxyAuthorizationPresent=head?.headers.some(header=>header.name.toLowerCase()==='proxy-authorization')??false;
    this.sourceIp=this.side==='request'?this.detail?.sourceIp??'':'';
    this.sortHeaders();
    this.detailsText = this.detail ? JSON.stringify({terminal:this.detail.terminal,route:this.detail.routeSelection,attempts:this.detail.routeAttempts,hookEffects:this.detail.hookEffects,diagnostics:this.detail.diagnostics,sequenceLoss:this.detail.sequenceLoss},null,2) : 'No exchange selected.';
    if (this.detail && (this.mode === 'body' || this.mode === 'split')) void this.inspectBody();
    else { this.generation++; this.hexPreview = null; this.bodyText = 'Select an exchange to preview its body.'; this.bodyFacts = ''; }
  }
  showMode(mode:string): void { this.mode = mode; if (mode === 'body' || mode === 'split') void this.inspectBody(); }
  changeBoundary(event:Event): void { this.boundary = (event.currentTarget as HTMLSelectElement).value; this.updateMessage(); }
  toggleHeaderOrder():void {this.largestFirst=!this.largestFirst;this.sortHeaders();}
  private sortHeaders():void {this.headerRows=[...this.headerRows].sort(this.largestFirst?(a,b)=>b.bytes-a.bytes:(a,b)=>Number(a.id)-Number(b.id));}
  changeViewer(event:Event): void { this.viewer = (event.currentTarget as HTMLSelectElement).value; void this.inspectBody(); }
  changeDecoding(event:Event): void { this.decodeContent = (event.currentTarget as HTMLInputElement).checked; void this.inspectBody(); }
  resizeHeaders(event:CustomEvent<{value:number;committed:boolean}>): void {
    this.headerShare = event.detail.value+'%';
    this.$emit('inspector-resize',{side:this.side,...event.detail});
  }
  async inspectBody(): Promise<void> {
    const generation = ++this.generation;
    const body = this.detail?.storedBodies.find((body) => body.boundary === this.boundary);
    const previewKey = [this.detail?.id, this.boundary, this.viewer, this.decodeContent].join(':');
    if (this.hexPreview?.key !== previewKey) this.hexPreview = null;
    this.imageUrl = ''; this.loading = false;
    if (!this.detail) { this.hexPreview = null; return; }
    const head = this.heads().find((head) => head.boundary === this.boundary);
    this.bodyFacts = body ? this.retentionFacts(body) : '';
    this.bodySummary=body?formatBytes(body.retainedBytes)+' · '+body.availability:'';
    this.viewerLabel = this.viewer === 'auto' ? 'Auto' : this.viewer;
    if (this.viewer === 'metadata') { this.bodyText = body ? JSON.stringify(body,null,2) : 'No body metadata was captured for this message stage.'; this.viewerLabel = 'Metadata'; return; }
    if (this.side === 'response' && head?.status === 304 && !body?.retainedBytes) {
      this.bodyText = '304 Not Modified — no response body is expected. The client uses its cached representation.';
      this.viewerLabel = 'Auto · no body'; return;
    }
    if (!body) { this.hexPreview = null; this.bodyText = this.side === 'request' ? 'No request body is available. Check retention settings or the source capture.' : 'No response body metadata was captured. The Details tab shows exchange failures and capture loss.'; return; }
    if (['disabled','evicted','quota-omitted'].includes(body.availability)) { this.hexPreview = null; this.bodyText = this.retentionReason(body); return; }
    if (!body.retainedBytes) { this.hexPreview = null; this.bodyText = body.availability === 'complete' && body.observedBytes === 0 ? 'This '+this.side+' has no body.' : this.retentionReason(body); return; }
    this.loading = true; this.bodyText = this.hexPreview ? '' : 'Loading preview…';
    try {
      const inspection = await invoke<BodyInspection>('inspect_body',{request:{sessionId:this.detail.id,boundary:this.boundary,representation:this.viewer,decodeContent:this.viewer !== 'bytes' && this.decodeContent,offset:0,maxBytes:Number(this.bodyLimit?.value ?? 262144)}});
      if (generation !== this.generation || !this.isConnected) return;
      this.bodyMetadata = inspection.metadata;
      const labels:Record<string,string> = {'formatted-json':'JSON','original-text':'Text','bytes':'Hex','image':'Image','metadata':'Metadata','unavailable':'Unavailable'};
      this.viewerLabel = (this.viewer === 'auto' ? 'Auto · ' : '')+(labels[inspection.representation] ?? inspection.representation);
      this.bodyFacts = this.retentionFacts(inspection.metadata)+' · '+formatBytes(inspection.displayBytes)+' shown'+(inspection.decoded ? ' · decoded' : '')+(inspection.truncated ? ' · preview truncated' : '');
      if (inspection.representation === 'bytes' && inspection.bytesBase64 !== null) {
        this.hexPreview = { key: previewKey, bytesBase64: inspection.bytesBase64, offset: inspection.byteOffset,
          truncated: inspection.truncated };
        this.bodyText = inspection.warning ?? '';
      } else {
        this.hexPreview = null;
        this.bodyText = inspection.display || inspection.warning || (inspection.representation === 'image' ? '' : 'No body content.');
        if (inspection.warning && inspection.display) this.bodyText = inspection.warning+'\n\n'+inspection.display;
      }
      if (inspection.metadata.availability !== 'complete') this.bodyText = this.retentionReason(inspection.metadata)+'\n\n'+this.bodyText;
      if (inspection.previewHandle) this.imageUrl = convertFileSrc('preview/'+inspection.previewHandle,'transmog-preview');
    } catch (error:unknown) {
      if (generation === this.generation) { this.hexPreview = null; this.bodyText = 'Preview unavailable: '+describeError(error)+'.\n\n'+this.retentionReason(body)+' Raw Hex shows the '+formatBytes(body.retainedBytes)+' retained bytes with decoding disabled. Metadata shows the capture status and reason.'; }
    } finally { if (generation === this.generation) this.loading = false; }
  }
  private retentionFacts(body:StoredBodyMetadata):string {
    const bytes = (value:number) => body.availability === 'complete' ? formatBytes(value) : value.toLocaleString()+' B';
    return (body.mediaType ?? 'Unknown content type')+' · '+bytes(body.retainedBytes)+' retained / '+bytes(body.observedBytes)+' observed · '+body.availability+(body.reason ? ' · '+body.reason : '');
  }
  private retentionReason(body:StoredBodyMetadata):string {
    const reasons:Record<string,string> = {
      complete:'The complete body was retained.', capturing:'The body is still being captured; its final size is not yet known.',
      truncated:'Only part of the body was retained because a capture limit was reached.', lost:'The capture is incomplete because delivery or storage was interrupted.',
      evicted:'The body was removed to make room in the circular response cache.', disabled:'Body retention was disabled for this exchange.',
      'quota-omitted':'The body was omitted because the response cache was full.',
    };
    return (reasons[body.availability] ?? 'Body capture status: '+body.availability+'.')+(body.reason ? ' Reason: '+body.reason+'.' : '');
  }
  async copyHeaders(): Promise<void> { await this.copy([...this.headerRows].sort((a,b)=>Number(a.id)-Number(b.id)).map((row) => row.name+': '+row.value).join('\r\n'),'Headers copied.'); }
  async saveBody():Promise<void> {
    if (this.saving || !this.detail || this.bodyMetadata?.availability !== 'complete' || !this.bodyMetadata.retainedBytes) return;
    this.saving = true;
    try {
      const result = await invoke<{fileName:string;bytes:number}|null>('save_response_body',{sessionId:this.detail.id,boundary:this.boundary});
      if (result) this.$emit('diagnostic','Saved '+result.fileName+' ('+formatBytes(result.bytes)+').');
    } catch (error:unknown) {
      this.$emit('notice',{title:'Response save failed',message:describeError(error),actionLabel:null,action:null});
    } finally { this.saving = false; }
  }
  private async copy(text:string,message:string): Promise<void> {
    try { await navigator.clipboard.writeText(text); this.$emit('diagnostic',message); }
    catch { this.$emit('diagnostic','Select the text and use Copy.'); }
  }
  disconnectedCallback(): void { this.generation++; super.disconnectedCallback(); }
}
MessageInspector.define('message-inspector');
