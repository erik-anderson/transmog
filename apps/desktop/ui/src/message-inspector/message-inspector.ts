import { WebUIElement, attr, observable } from '@microsoft/webui-framework';
import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import type { BodyBytePage, BodyInspection, HeaderPage, HeaderSummary, HeadView, SessionDetail, StoredBodyMetadata } from '../models.js';
import { describeError } from '../utilities.js';
import { formatBytes } from '../table-model.js';
import { opaqueConnectTransport } from '../connection-evidence.js';
import {copyGesture, requestCopy, copyScalar, CopyError} from '../inspection-copy.js';
import { selectedConverterText } from '../converter-selection.js';
import type { HexPreview } from '../hex-viewer/hex-viewer.js';
import {listen,type UnlistenFn} from '@tauri-apps/api/event';
import type {VisualizerChoice,VisualizerSelection} from '../extension-visualizer/models.js';
import type {AuthSelection,AuthSourceNavigation} from '../auth-inspector/models.js';

const stages:Record<string,string> = {'client-request':'Original request','upstream-request':'Request sent to origin','upstream-response':'Origin response','client-response':'Response sent to client'};

const exactHeaderBytes=(value:number|null|undefined):string=>value==null?'Size unavailable':value.toLocaleString()+(value===1?' byte':' bytes');
export class MessageInspector extends WebUIElement {
  @observable authVisible=false;
  @observable authSelection:AuthSelection|null=null;
  @observable authBadge='';
  @observable authCount=-1;
  @observable authSourceNotice='';
  private authRequestedKey='';
  onAuthCount(event:CustomEvent<{count:number|null;complete?:boolean}>):void {
    event.stopPropagation();this.authCount=event.detail.count??-1;this.authBadge=this.authCount<0?'':'('+this.authCount+')';
    if(this.authCount===0&&this.mode==='auth'&&this.authRequestedKey!==this.authSelection?.key)this.showMode(this.side==='request'?'headers':'body');
  }
  async showAuthSource(event:CustomEvent<AuthSourceNavigation>):Promise<void> {
    event.stopPropagation();const source=event.detail;
    if(source.sessionId!==this.detail?.id||source.boundary!==this.boundary)return;
    const body=source.source.kind==='body';
    this.authSourceNotice='Original retained source may contain secrets. '+source.source.label+(source.source.transforms.length?' · '+source.source.transforms.join(' → '):'');
    if(body)this.viewer='original-text';else {this.headerViewer='raw';this.largestFirst=false;}
    this.setChoices();this.sortHeaders();this.showMode(body?'body':'headers');
    if(body)return;
    if(source.source.kind==='query')return;
    const index=source.source.headerIndex??source.source.occurrence;
    const offset=Math.floor(index/512)*512;
    let moved=false;const interaction=():void=>{moved=true;};document.addEventListener('pointerdown',interaction,true);document.addEventListener('keydown',interaction,true);
    try {
      if((this.heads().find(head=>head.boundary===this.boundary)?.summary?.totalFields??0)>512)await this.loadHeaderPage(offset);
      if(moved||!this.isConnected||source.sessionId!==this.detail?.id||source.boundary!==this.boundary)return;
      this.$flushUpdates();const root=this.getRootNode();if(root instanceof ShadowRoot)root.getElementById(this.pane+'-header-'+index)?.focus({preventScroll:false});
    } finally {document.removeEventListener('pointerdown',interaction,true);document.removeEventListener('keydown',interaction,true);}
  }
  showAuthBoundary(boundary:string):void {if(!this.heads().some(head=>head.boundary===boundary))return;this.boundary=boundary;this.updateMessage();this.showMode('auth');}
  headerContext(header:{id:string;sensitive:boolean},event:MouseEvent|KeyboardEvent):void {
    if (!copyGesture(event) || selectedConverterText(event)) return;
    const source=event.currentTarget as HTMLElement,id=this.detail?.id,boundary=this.boundary,headers=this.headerGeneration;
    if(!id)return;
    const current=():boolean=>this.isConnected&&headers===this.headerGeneration&&this.detail?.id===id&&this.boundary===boundary;
    const read=()=>invoke<{name:string;value:string|null;binary:boolean}>('copy_header_field',{id,boundary,index:Number(header.id)});
    requestCopy(event,{source,current,note:header.sensitive?'The captured value was redacted.':undefined,actions:[
      {id:'name',label:'Copy name',read:async()=>copyScalar((await read()).name)},
      {id:'value',label:'Copy value',disabled:header.sensitive,read:async()=>{const field=await read();if(field.value===null)throw new CopyError('The captured value was redacted.');return copyScalar(field.value);}},
      {id:'pair',label:'Copy name and value',disabled:header.sensitive,read:async()=>{const field=await read();if(field.value===null)throw new CopyError('The captured value was redacted.');return copyScalar(field.name)+': '+copyScalar(field.value);}},
      {id:'converter',label:'Open in Format converter',disabled:header.sensitive,convert:true,read:()=>invoke<string>('converter_header_value',{id,boundary,index:Number(header.id)})},
    ]});
  }
  @attr side = 'response';
  @attr split = '35';
  @attr pane = 'message';
  @observable headerViewer='raw';
  @observable headerChoices:VisualizerChoice[]=[];
  @observable bodyChoices:VisualizerChoice[]=[];
  @observable headerChoice:VisualizerChoice|null=null;
  @observable bodyChoice:VisualizerChoice|null=null;
  @observable headerSelection:VisualizerSelection|null=null;
  @observable bodySelection:VisualizerSelection|null=null;
  @observable bodyExtension=false;
  @observable headerSlot='message-headers';
  @observable bodySlot='message-body';
  private choiceKey='';
  private choiceGeneration=0;
  private choiceListener:UnlistenFn|undefined;
  private choiceListenerGeneration=0;
  private choiceVisible=false;
  private choiceObserver:IntersectionObserver|null=null;
  private preferredMode = '';
  private preferredBodyViewer = 'auto';
  private preferredHeaderViewer = 'raw';
  private preferredProtocol: string | null = null;
  private rejectedAutomatic = new Set<string>();
  @observable detail: SessionDetail | null = null;
  @observable mode = 'body';
  @observable boundaries: Array<{id:string;label:string}> = [];
  @observable boundary = '';
  @observable headerRows: Array<{id:string;name:string;value:string;sensitive:boolean;bytes:number;size:string;valueSize:string;sizeTitle:string;valueSizeTitle:string}> = [];
  @observable headersSummaryTitle='';
  @observable headersSummary = '';
  @observable showHeaderSizes=false;
  toggleHeaderSizes(event:Event):void {this.showHeaderSizes=(event.currentTarget as HTMLInputElement).checked;}
  @observable largestFirst = false;
  @observable authorizationPresent = false;
  @observable proxyAuthorizationPresent = false;
  @observable authorizationState='unknown';
  @observable proxyAuthorizationState='unknown';
  @observable headerPageText='';
  @observable headerOffset=0;
  @observable headerNextOffset:number|null=null;
  @observable headersBusy=false;
  private headerGeneration=0;
  private headerContextGeneration=0;
  @observable sourceIp = '';
  @observable headSummary = 'Select an exchange';
  @observable bodyText = 'Select an exchange to preview its body.';
  @observable bodyFacts = '';
  @observable bodySummary = '';
  @observable viewerLabel = 'Auto';
  @observable imageUrl = '';
  @observable hexPreview: HexPreview | null = null;
  @observable pagedHex = false;
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
  private inspectedBodyKey: string | null = null;

  sideChanged(): void {
    this.preferredMode = this.side === 'request' ? 'headers' : 'body';
    this.preferredProtocol = null;
    this.mode = this.preferredMode;
    this.updateMessage();
  }
  paneChanged():void {this.headerSlot=this.pane+'-headers';this.bodySlot=this.pane+'-body';}
  protected hydratedCallback():void {
    this.choiceObserver=new IntersectionObserver(entries=>{const visible=entries.some(entry=>entry.isIntersecting);this.authVisible=visible;if(this.choiceVisible===visible)return;this.choiceVisible=visible;if(visible){this.watchChoices();void this.refreshChoices();}else {this.choiceGeneration++;this.choiceKey='';this.unwatchChoices();}});this.choiceObserver.observe(this);
  }
  private watchChoices():void {
    const generation=++this.choiceListenerGeneration;
    void listen('extensions-changed',()=>{if(this.choiceVisible)void this.refreshChoices(true);}).then(unlisten=>{if(this.isConnected&&this.choiceVisible&&generation===this.choiceListenerGeneration)this.choiceListener=unlisten;else void Promise.resolve(unlisten()).catch(()=>undefined);}).catch(()=>undefined);
  }
  private unwatchChoices():void {this.choiceListenerGeneration++;const unlisten=this.choiceListener;this.choiceListener=undefined;if(unlisten)void Promise.resolve(unlisten()).catch(()=>undefined);}
  private setChoices(): void {
    this.headerChoice = this.headerChoices.find(choice => choice.key === this.headerViewer) ?? null;
    const candidates = this.bodyChoices.filter(choice => choice.automatic)
      .sort((left, right) => left.key.localeCompare(right.key)).slice(0, 4);
    this.bodyChoice = this.viewer === 'auto'
      ? candidates.find(choice => !this.rejectedAutomatic.has(choice.key)) ?? null
      : this.bodyChoices.find(choice => choice.key === this.viewer) ?? null;
    this.bodyExtension = !!this.bodyChoice;
  }
  private rememberView(): void {
    this.preferredMode = this.mode;
    this.preferredBodyViewer = this.viewer;
    this.preferredHeaderViewer = this.headerViewer;
    this.preferredProtocol = this.mode === 'headers' ? this.headerChoice?.key ?? null
      : this.mode === 'body' || this.mode === 'split' ? this.bodyChoice?.key ?? null : null;
  }
  private restoreView(): void {
    this.headerViewer = this.headerChoices.some(choice => choice.key === this.preferredHeaderViewer)
      ? this.preferredHeaderViewer : 'raw';
    this.viewer = !this.preferredBodyViewer.startsWith('extension:')
      || this.bodyChoices.some(choice => choice.key === this.preferredBodyViewer)
      ? this.preferredBodyViewer : 'auto';
    const available = !this.preferredProtocol
      || [...this.headerChoices, ...this.bodyChoices].some(choice => choice.key === this.preferredProtocol);
    this.mode = available ? this.preferredMode || (this.side === 'request' ? 'headers' : 'body')
      : this.side === 'request' ? 'headers' : 'body';
    this.setChoices();
    if (this.mode === 'body' || this.mode === 'split') void this.inspectBody(false);
  }
  automaticRejected(event: CustomEvent<{selection: VisualizerSelection; key: string}>): void {
    const {selection, key} = event.detail;
    if (this.viewer !== 'auto' || this.bodyChoice?.key !== key
      || this.detail?.id !== selection.sessionId || this.boundary !== selection.boundary) return;
    this.rejectedAutomatic.add(key);
    this.setChoices();
    if (!this.bodyChoice) void this.inspectBody();
  }
  private async refreshChoices(force=false):Promise<void> {
    if(!this.isConnected||!this.choiceVisible||!this.detail)return;
    const key=JSON.stringify([this.detail.id,this.boundary,...this.detail.requests.map(head=>[head.boundary,head.method,head.summary?.totalFields,head.summary?.serializedBytes]),...this.detail.responses.map(head=>[head.boundary,head.status,head.summary?.totalFields,head.summary?.serializedBytes]),...this.detail.storedBodies.map(body=>[body.boundary,body.availability,body.availability==='complete'?body.retainedBytes:null,body.sha256])]);
    if(!force&&this.choiceKey===key)return;this.choiceKey=key;
    this.rejectedAutomatic.clear();
    const generation=++this.choiceGeneration,sessionId=this.detail.id,boundary=this.boundary;
    try {
      const results=await Promise.all((['headers','body'] as const).map(surface=>invoke<Omit<VisualizerChoice,'key'>[]>('visualizer_choices',{selection:{sessionId,boundary,surface}})));
      if(generation!==this.choiceGeneration||!this.choiceVisible||!this.isConnected)return;
      const map=(choices:Omit<VisualizerChoice,'key'>[]):VisualizerChoice[]=>choices.map(choice=>({...choice,key:'extension:'+choice.extensionId+':'+choice.visualizerId}));
      this.headerChoices=map(results[0]!);this.bodyChoices=map(results[1]!);
      this.restoreView();
    } catch {if(generation===this.choiceGeneration)this.choiceKey='';}
  }
  changeHeaderViewer(event:Event):void {this.headerViewer=(event.currentTarget as HTMLSelectElement).value;this.setChoices();this.rememberView();}
  splitChanged(): void { this.headerShare = this.split+'%'; }
  detailChanged(): void {
    if (this.detail?.id !== this.previousId) { this.authSourceNotice=''; this.previousId = this.detail?.id ?? null; this.headerOffset=0;this.headersBusy=false;this.headerGeneration++; this.resetViewers(); this.boundary = ''; this.imageUrl = ''; }
    this.updateMessage();
  }
  private heads(): HeadView[] { return this.side === 'request' ? this.detail?.requests ?? [] : this.detail?.responses ?? []; }
  private updateMessage(): void {
    this.headerGeneration++;this.headersBusy=false;
    const heads = this.heads();
    const expected = this.side === 'request' ? 'client-request' : 'client-response';
    this.boundaries = heads.map((head) => ({id:head.boundary,label:stages[head.boundary] ?? head.boundary}));
    if (!heads.some((head) => head.boundary === this.boundary)) this.boundary = heads.find((head) => head.boundary === expected)?.boundary ?? heads[0]?.boundary ?? expected;
    const head = heads.find((head) => head.boundary === this.boundary);
    this.bodyMetadata = this.detail?.storedBodies.find((body) => body.boundary === this.boundary) ?? null;
    this.headSummary = head ? this.side === 'request' ? (head.method ?? '')+' '+(head.target ?? '') : 'HTTP '+head.status+' · '+(stages[head.boundary] ?? head.boundary) : this.detail ? 'Waiting for '+this.side+' headers' : 'Select an exchange';
    if(this.headerOffset>0)void this.loadHeaderPage(this.headerOffset);
    else {this.renderHeaders(head?.headers??[],head?.summary,0);if(this.largestFirst && (head?.summary?.totalFields??0)>512)void this.loadHeaderPage(0);}
    this.sourceIp=this.side==='request'?this.detail?.sourceIp??'':'';
    this.sortHeaders();
    this.detailsText = this.detail ? JSON.stringify({terminal:this.detail.terminal,route:this.detail.routeSelection,attempts:this.detail.routeAttempts,hookEffects:this.detail.hookEffects,diagnostics:this.detail.diagnostics,sequenceLoss:this.detail.sequenceLoss,...(this.detail.composerOrigin?{composerSource:this.detail.composerOrigin}:{})},null,2) : 'No exchange selected.';
    this.headerSelection=this.detail?{sessionId:this.detail.id,boundary:this.boundary,surface:'headers'}:null;
    this.bodySelection=this.detail?{sessionId:this.detail.id,boundary:this.boundary,surface:'body'}:null;
    const authKey=JSON.stringify([this.detail?.id,this.boundary,this.detail?.authRevisions?.[this.boundary]??head,this.bodyMetadata]);
    this.authSelection=this.detail?{sessionId:this.detail.id,boundary:this.boundary,key:authKey}:null;
    void this.refreshChoices();
    if (this.detail && (this.mode === 'body' || this.mode === 'split')) void this.inspectBody(false);
    else if (!this.detail || this.bodyInspectionKey() !== this.inspectedBodyKey) {
      this.generation++; this.inspectedBodyKey=null; this.loading=false; this.hexPreview=null; this.imageUrl='';
      this.bodyText='Select an exchange to preview its body.'; this.bodyFacts=''; this.bodySummary='';
    }
  }
  showMode(mode:string): void { if(mode==='auth'){this.authSourceNotice='';this.authRequestedKey=this.authSelection?.key??'';}this.mode = mode; this.rememberView(); if (mode === 'body' || mode === 'split') void this.inspectBody(false); }
  private resetViewers():void {
    this.viewer='auto';this.headerViewer='raw';this.headerChoice=null;this.bodyChoice=null;this.bodyExtension=false;
    this.headerChoices=[];this.bodyChoices=[];this.choiceKey='';this.choiceGeneration++;this.rejectedAutomatic.clear();
    this.mode=this.preferredProtocol?(this.side==='request'?'headers':'body'):this.preferredMode||this.mode;
  }
  changeBoundary(event:Event): void { this.authSourceNotice=''; this.headerOffset=0; this.resetViewers();this.boundary = (event.currentTarget as HTMLSelectElement).value; this.updateMessage(); }
  toggleHeaderOrder():void {this.largestFirst=!this.largestFirst;this.headerOffset=0;const head=this.heads().find(head=>head.boundary===this.boundary);if((head?.summary?.totalFields??0)>512)void this.loadHeaderPage(0);else this.sortHeaders();}
  previousHeaders():void {void this.loadHeaderPage(Math.max(0,this.headerOffset-512));}
  nextHeaders():void {if(this.headerNextOffset!==null)void this.loadHeaderPage(this.headerNextOffset);}
  private renderHeaders(fields:HeadView['headers'],summary?:HeaderSummary,offset=0):void {
    this.headerRows=fields.map((header,index)=>({id:String(header.index??offset+index),sensitive:header.sensitive,name:header.name,value:header.sensitive?'[redacted]':header.value,bytes:header.fieldBytes??0,size:header.fieldBytes==null?'Unavailable':formatBytes(header.fieldBytes),sizeTitle:exactHeaderBytes(header.fieldBytes)+' · Field name, colon-space, original value and CRLF',valueSizeTitle:exactHeaderBytes(header.valueBytes)+' · Original field value before redaction',valueSize:header.valueBytes==null?'Unavailable':formatBytes(header.valueBytes)}));
    const total=summary?.totalFields??fields.length, measured=fields.every(header=>header.fieldBytes!=null);
    const bytes=summary?summary.serializedBytes:measured?this.headerRows.reduce((sum,row)=>sum+row.bytes,2):null;
    this.headersSummaryTitle=exactHeaderBytes(bytes)+' · Sum of original serialized fields and final CRLF, before redaction; HTTP/1 equivalent';
    this.headersSummary=fields.length||summary?total+' headers · '+(bytes===null?'size unavailable':formatBytes(bytes)):'';
    this.authorizationState=summary?.authorization??(fields.length?fields.some(header=>header.name.toLowerCase()==='authorization')?'present':'absent':'unknown');
    this.proxyAuthorizationState=summary?.proxyAuthorization??(fields.length?fields.some(header=>header.name.toLowerCase()==='proxy-authorization')?'present':'absent':'unknown');
    this.authorizationPresent=this.authorizationState==='present';this.proxyAuthorizationPresent=this.proxyAuthorizationState==='present';
    this.headerNextOffset=offset+fields.length<total?offset+fields.length:null;this.headerPageText=total>512?'Showing '+(offset+1)+'–'+(offset+fields.length)+' of '+total+' headers':'';
  }
  private async loadHeaderPage(offset:number):Promise<void> {
    if(!this.detail)return;const id=this.detail.id,boundary=this.boundary,generation=++this.headerGeneration;this.headersBusy=true;
    try {const page=await invoke<HeaderPage>('inspect_headers',{id,boundary,offset,largestFirst:this.largestFirst});if(!this.isConnected||this.detail?.id!==id||this.boundary!==boundary||generation!==this.headerGeneration)return;this.headerOffset=page.offset;this.renderHeaders(page.headers,page.summary,page.offset);}
    catch(error:unknown){if(generation===this.headerGeneration)this.$emit('diagnostic','Headers could not be loaded: '+describeError(error));}
    finally {if(generation===this.headerGeneration)this.headersBusy=false;}
  }
  private sortHeaders():void {this.headerRows=[...this.headerRows].sort(this.largestFirst?(a,b)=>b.bytes-a.bytes:(a,b)=>Number(a.id)-Number(b.id));}
  changeViewer(event:Event): void {
    this.viewer = (event.currentTarget as HTMLSelectElement).value;
    if (this.viewer === 'auto') this.rejectedAutomatic.clear();
    this.setChoices();this.rememberView();void this.inspectBody();
  }
  changeDecoding(event:Event): void { this.decodeContent = (event.currentTarget as HTMLInputElement).checked; void this.inspectBody(); }
  resizeHeaders(event:CustomEvent<{value:number;committed:boolean}>): void {
    this.headerShare = event.detail.value+'%';
    this.$emit('inspector-resize',{side:this.side,...event.detail});
  }
  private bodyInspectionKey():string {
    const body=this.detail?.storedBodies.find(body=>body.boundary===this.boundary);
    const status=this.heads().find(head=>head.boundary===this.boundary)?.status;
    // Session detail objects also change for unrelated traffic and timing updates.
    // Only the selected body evidence and its interpretation invalidate this view.
    return JSON.stringify([this.detail?.id,this.side,this.boundary,this.viewer,this.decodeContent,Number(this.bodyLimit?.value??262144),
      status,this.detail?opaqueConnectTransport(this.detail)?.tunnelDestination:null,
      body?[body.exchangeId,body.boundary,body.observedBytes,body.wireBodyBytes,body.retainedBytes,body.availability,body.mediaType,body.charset,body.contentCodings,body.sha256,body.reason,body.lengthKnown]:null]);
  }
  async inspectBody(force=true): Promise<void> {
    if(this.bodyExtension){this.generation++;this.loading=false;this.hexPreview=null;this.imageUrl='';this.bodyText='';this.inspectedBodyKey=null;return;}
    const inspectionKey=this.bodyInspectionKey();
    if(!force && inspectionKey===this.inspectedBodyKey)return;
    this.inspectedBodyKey=inspectionKey;
    const generation = ++this.generation;
    const body = this.detail?.storedBodies.find((body) => body.boundary === this.boundary);
    const previewKey = [this.detail?.id, this.boundary, this.viewer, this.decodeContent].join(':');
    if (this.hexPreview?.key !== previewKey) this.hexPreview = null;
    this.pagedHex = false;
    this.imageUrl = ''; this.loading = false;
    if (!this.detail) { this.hexPreview = null; return; }
    const head = this.heads().find((head) => head.boundary === this.boundary);
    this.bodyFacts = body ? this.retentionFacts(body) : '';
    this.bodySummary=body?formatBytes(body.retainedBytes)+' · '+body.availability:'';
    this.viewerLabel = this.viewer === 'auto' ? 'Auto' : this.viewer;
    const tunnel=opaqueConnectTransport(this.detail);
    if(tunnel) {this.hexPreview=null;this.bodyText='Opaque CONNECT tunnel to '+tunnel.tunnelDestination+'. HTTPS decryption was disabled. Tunnel contents and the tunneled protocol were not inspected. Open Timings for connection outcomes, endpoints and byte counts.';this.viewerLabel='Opaque tunnel';return;}
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
      if(this.viewer==='bytes') {
        await this.inspectRawHex(previewKey,generation);return;
      }
      const inspection = await invoke<BodyInspection>('inspect_body',{request:{sessionId:this.detail.id,boundary:this.boundary,representation:this.viewer,decodeContent:this.viewer !== 'bytes' && this.decodeContent,offset:0,maxBytes:Number(this.bodyLimit?.value ?? 262144)}});
      if (generation !== this.generation || !this.isConnected) return;
      this.bodyMetadata = inspection.metadata;
      const labels:Record<string,string> = {'formatted-json':'JSON','original-text':'Text','bytes':'Hex','image':'Image','metadata':'Metadata','unavailable':'Unavailable'};
      this.viewerLabel = (this.viewer === 'auto' ? 'Auto · ' : '')+(labels[inspection.representation] ?? inspection.representation);
      this.bodyFacts = this.retentionFacts(inspection.metadata)+' · '+formatBytes(inspection.displayBytes)+' shown'+(inspection.decoded ? ' · decoded' : '')+(inspection.truncated ? ' · preview truncated' : '');
      if (inspection.representation === 'bytes' && inspection.bytesBase64 !== null) {
        if(!inspection.decoded) await this.inspectRawHex(previewKey,generation,inspection.warning??'');
        else {
          this.hexPreview = { key: previewKey, bytesBase64: inspection.bytesBase64, offset: inspection.byteOffset,truncated: inspection.truncated };
          this.bodyText = inspection.warning ?? '';
        }
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
  private async inspectRawHex(key:string,generation:number,warning=''):Promise<void> {
    if(!this.detail)return;
    const sessionId=this.detail.id,boundary=this.boundary;
    const page=await invoke<BodyBytePage>('read_body_range',{request:{sessionId,boundary,offset:'0',maxBytes:65536}});
    if(generation!==this.generation||!this.isConnected)return;
    this.bodyMetadata=page.metadata;this.pagedHex=true;
    this.viewerLabel=(this.viewer==='auto'?'Auto · ':'')+'Hex';
    this.bodySummary=formatBytes(page.metadata.retainedBytes)+' · '+page.metadata.availability;
    this.bodyFacts=this.retentionFacts(page.metadata)+' · All retained bytes available, loaded in pages · raw bytes';
    this.hexPreview={key,bytesBase64:page.bytesBase64,offset:'0',truncated:false,source:{sessionId,boundary,totalBytes:page.totalBytes,readLimit:page.readLimit}};
    this.bodyText=warning;
    if(page.metadata.availability!=='complete')this.bodyText=this.retentionReason(page.metadata)+'\n\n'+this.bodyText;
  }
  private retentionFacts(body:StoredBodyMetadata):string {
    const bytes = (value:number) => body.availability === 'complete' ? formatBytes(value) : value.toLocaleString()+' B';
    return (body.mediaType ?? 'Unknown content type')+' · '+bytes(body.retainedBytes)+' retained / '+bytes(body.observedBytes)+' observed'+(body.contentCodings.length&&body.wireBodyBytes!=null?' · '+body.wireBodyBytes.toLocaleString()+' wire body bytes ('+body.contentCodings.join(', ')+', excludes framing/TLS)':'')+' · '+body.availability+(body.reason ? ' · '+body.reason : '');
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
  async copyHeaders():Promise<void> {if(!this.detail)return;try {const text=await invoke<string>('copy_message_headers',{id:this.detail.id,boundary:this.boundary});await this.copy(text,'Complete message headers copied.');}catch(error:unknown){this.$emit('diagnostic','Headers could not be copied: '+describeError(error));}}
  async copyUrl():Promise<void> {if(!this.detail)return;try {const text=await invoke<string>('copy_message_url',{id:this.detail.id,boundary:this.boundary});await this.copy(text,'Complete request URL copied.');}catch(error:unknown){this.$emit('diagnostic','URL could not be copied: '+describeError(error));}}
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
  disconnectedCallback(): void { this.choiceGeneration++;this.choiceVisible=false;this.choiceObserver?.disconnect();this.choiceObserver=null;this.unwatchChoices();this.headerGeneration++;this.generation++;this.inspectedBodyKey=null; super.disconnectedCallback(); }
}
MessageInspector.define('message-inspector');
