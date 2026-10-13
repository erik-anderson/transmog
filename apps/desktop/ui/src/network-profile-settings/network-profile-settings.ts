import initialState from '../initial-state.json';
import {observable} from '@microsoft/webui-framework';
import {invoke} from '@tauri-apps/api/core';
import {WorkspaceElement} from '../workspace-element.js';
import type {NetworkProfiles, NetworkProfileSnapshot, NetworkProfileSettings, ProductState} from '../models.js';
import {describeError} from '../utilities.js';

const chromium:NetworkProfileSnapshot=initialState.networkProfiles.activeProfile as NetworkProfileSnapshot;
type ChangeFlags=Record<keyof NetworkProfileSettings,boolean>;
const differenceLabels:Record<keyof NetworkProfileSettings,string>={
  protocols:'HTTP protocols',reuseConnections:'Connection reuse',tlsVersions:'TLS versions',
  compatibilityCiphers:'Compatibility ciphers',hybridKeyExchange:'Key exchange',grease:'GREASE',
  randomizeExtensions:'Extension order',echGrease:'ECH GREASE',certificateCompression:'Certificate compression'
};

/** Independent profile saves do not submit or overwrite ordinary preference drafts. */
export class NetworkProfileSettingsPanel extends WorkspaceElement {
  @observable networkProfiles:NetworkProfiles=structuredClone(initialState.networkProfiles) as NetworkProfiles;
  @observable networkOptions=initialState.networkOptions;
  @observable networkSelectedId=initialState.networkSelectedId;
  @observable networkActiveName=initialState.networkActiveName;
  @observable networkSummary=initialState.networkSummary;
  @observable networkDiffSummary=initialState.networkDiffSummary;
  @observable networkDifferences:string[]=[];
  @observable networkChanged:ChangeFlags=initialState.networkChanged;
  @observable networkBusy=true;
  @observable networkError='';
  @observable networkText='';
  @observable networkCustom=false;
  @observable networkSelectedActive=true;
  @observable networkCanUse=false;
  @observable networkDirty=false;
  @observable networkEditorOpen=false;
  @observable networkDraft:NetworkProfileSnapshot=structuredClone(chromium);
  @observable networkValidation='';
  @observable networkUndoAvailable=false;
  networkEditor!:HTMLDialogElement;
  networkForm!:HTMLFormElement;
  networkSelector!:HTMLSelectElement;
  networkMenu!:HTMLElement;
  @observable networkLoaded=initialState.networkLoaded;
  private generation=0;
  private editorSource:NetworkProfileSnapshot|null=null;
  private undo:NetworkProfileSnapshot|null=null;

  protected hydratedCallback():void {void this.load();}
  async load():Promise<void> {
    if(this.networkLoaded&&this.networkBusy)return;
    const generation=++this.generation;this.networkBusy=true;
    try {
      const state=await invoke<ProductState>('product_state');
      if(generation!==this.generation||!this.isConnected)return;
      if(!state.preferences.networkProfiles?.activeProfile)throw new Error('Network profiles could not be loaded. Reload profiles to try again.');
      this.networkProfiles=structuredClone(state.preferences.networkProfiles);
      this.networkLoaded=true;this.networkError='';
      if(!this.options().some(profile=>profile.id===this.networkSelectedId))this.networkSelectedId=this.networkProfiles.activeProfile.id;
      this.renderPreview();
    } catch(error:unknown) {if(generation===this.generation)this.networkError=describeError(error);}
    finally {if(generation===this.generation){this.networkBusy=false;this.renderPreview();}}
  }
  private options():NetworkProfileSnapshot[] {return [chromium,...this.networkProfiles.customProfiles];}
  private selected():NetworkProfileSnapshot {return this.options().find(profile=>profile.id===this.networkSelectedId)??chromium;}
  private differences(settings:NetworkProfileSettings):void {
    const changed={} as ChangeFlags;
    for(const key of Object.keys(differenceLabels) as Array<keyof NetworkProfileSettings>)changed[key]=JSON.stringify(settings[key])!==JSON.stringify(chromium.settings[key]);
    this.networkChanged=changed;
    this.networkDifferences=(Object.keys(changed) as Array<keyof NetworkProfileSettings>).filter(key=>changed[key]).map(key=>differenceLabels[key]);
    this.networkDiffSummary=this.networkDifferences.length?`${this.networkDifferences.length} changes from Chromium 154: ${this.networkDifferences.join(', ')}.`:'Matches the Chromium 154 recipe.';
  }
  private renderPreview():void {
    const selected=this.selected();
    this.networkOptions=this.options().map(profile=>({id:profile.id,name:profile.name}));
    this.networkActiveName=this.networkProfiles.activeProfile.name;
    this.networkCustom=selected.id!==chromium.id;
    this.networkSelectedActive=selected.id===this.networkProfiles.activeProfile.id;
    this.networkCanUse=this.networkLoaded&&!this.networkBusy&&JSON.stringify(selected)!==JSON.stringify(this.networkProfiles.activeProfile);
    const s=selected.settings;
    const protocols=[s.protocols.http1?'H1':'',s.protocols.http2?'H2':'',s.protocols.http3?'H3':''].filter(Boolean).join(' / ')||'All HTTP blocked';
    const tls=s.tlsVersions==='tls12-and13'?'TLS 1.2–1.3':s.tlsVersions==='tls12-only'?'TLS 1.2':'TLS 1.3';
    this.networkSummary=`${protocols} · ${tls} · ${s.reuseConnections?'Reuse on':'Reuse off'}`;
    if(!this.networkEditorOpen)this.differences(s);
  }
  choose(event:Event):void {
    event.stopPropagation();
    const id=(event.target as HTMLSelectElement).value;
    if(id==='create') {this.networkSelector.value=this.networkSelectedId;this.customize(true);return;}
    this.networkSelectedId=id;this.networkText='';this.renderPreview();
  }
  private closeMenu():void {this.networkMenu.hidePopover();}
  customize(copy=false):void {
    this.closeMenu();
    if(!this.networkLoaded||this.networkBusy)return;
    if(this.networkDirty) {this.openEditor();return;}
    const source=this.selected();
    const isNew=copy||source.id===chromium.id;
    if(isNew&&this.networkProfiles.customProfiles.length>=32) {this.networkError='You can save up to 32 custom profiles. Delete one before creating another.';return;}
    this.networkDraft=structuredClone(source);
    if(isNew) {
      this.networkDraft.id='custom-'+crypto.randomUUID();
      this.networkDraft.name=this.uniqueName(source.id===chromium.id?'My profile':source.name+' copy');
      this.networkDraft.revision=1;
    }
    this.editorSource=isNew?null:structuredClone(source);
    this.networkDirty=isNew;this.networkValidation='';
    this.openEditor();
  }
  private uniqueName(preferred:string):string {
    const stem=preferred.slice(0,70).replace(/[\uD800-\uDBFF]$/,'');
    let name=stem;let number=2;
    while(this.options().some(profile=>profile.name.toLowerCase()===name.toLowerCase()))name=stem+' '+number++;
    return name;
  }
  private openEditor():void {
    this.networkEditorOpen=true;this.differences(this.networkDraft.settings);this.$flushUpdates();
    this.populate();this.networkEditor.showModal();
    (this.networkForm.elements.namedItem('name') as HTMLInputElement).focus();
  }
  private populate():void {
    const fields=this.networkForm.elements;
    (fields.namedItem('name') as HTMLInputElement).value=this.networkDraft.name;
    const s=this.networkDraft.settings;
    for(const key of ['http1','http2','http3'] as const)(fields.namedItem(key) as HTMLInputElement).checked=s.protocols[key];
    for(const key of ['reuseConnections','compatibilityCiphers','hybridKeyExchange','randomizeExtensions','echGrease','certificateCompression'] as const)(fields.namedItem(key) as HTMLInputElement).checked=s[key];
    (fields.namedItem('tlsVersions') as HTMLSelectElement).value=s.tlsVersions;
    (fields.namedItem('grease') as HTMLSelectElement).value=s.grease;
  }
  edit(event:Event):void {
    event.stopPropagation();
    const data=new FormData(this.networkForm);
    const s=this.networkDraft.settings;
    const settings:NetworkProfileSettings={...s,protocols:{http1:data.has('http1'),http2:data.has('http2'),http3:data.has('http3')},
      reuseConnections:data.has('reuseConnections'),compatibilityCiphers:data.has('compatibilityCiphers'),hybridKeyExchange:data.has('hybridKeyExchange'),
      randomizeExtensions:data.has('randomizeExtensions'),echGrease:data.has('echGrease'),certificateCompression:data.has('certificateCompression'),
      tlsVersions:String(data.get('tlsVersions')) as NetworkProfileSettings['tlsVersions'],grease:String(data.get('grease')) as NetworkProfileSettings['grease']};
    this.networkDraft={...this.networkDraft,name:String(data.get('name')),settings};
    this.networkDirty=!this.editorSource||JSON.stringify(this.networkDraft)!==JSON.stringify(this.editorSource);
    this.networkValidation=settings.protocols.http3&&settings.tlsVersions==='tls12-only'?'HTTP/3 requires TLS 1.3. Enable TLS 1.3 or turn off HTTP/3.':'';
    this.differences(settings);
  }
  reset():void {
    this.networkDraft={...this.networkDraft,settings:structuredClone(chromium.settings)};
    this.networkValidation='';this.networkDirty=!this.editorSource||JSON.stringify(this.networkDraft)!==JSON.stringify(this.editorSource);
    this.differences(this.networkDraft.settings);this.populate();
  }
  closeEditor():void {if(!this.networkBusy)this.networkEditor.close();}
  editorCancelled(event:Event):void {if(this.networkBusy)event.preventDefault();}
  editorKeyboard(event:KeyboardEvent):void {
    event.stopPropagation();
    if((event.ctrlKey||event.metaKey)&&event.key.toLowerCase()==='s') {event.preventDefault();void this.saveEditor(event,false);}
  }
  editorClosed():void {this.networkEditorOpen=false;this.renderPreview();this.networkSelector.focus();}
  discard():void {this.networkDirty=false;this.editorSource=null;this.networkEditor.close();}
  async saveEditor(event:Event,use:boolean):Promise<void> {
    event.preventDefault();event.stopPropagation();
    if(this.networkBusy||!this.networkForm.reportValidity()||this.networkValidation)return;
    const profile={...structuredClone(this.networkDraft),name:this.networkDraft.name.trim()};
    const profiles=this.networkProfiles.customProfiles.filter(item=>item.id!==profile.id);
    profiles.push(profile);
    if(await this.save(profiles,use?profile.id:null)) {
      this.networkDirty=false;this.editorSource=null;this.networkSelectedId=profile.id;
      this.networkEditor.close();this.renderPreview();
    }
  }
  async useProfile():Promise<void> {await this.save(this.networkProfiles.customProfiles,this.networkSelectedId);}
  async remove():Promise<void> {
    this.closeMenu();const profile=this.selected();if(profile.id===chromium.id)return;
    if(await this.save(this.networkProfiles.customProfiles.filter(item=>item.id!==profile.id),this.networkSelectedActive?chromium.id:null)) {
      this.undo={...profile,id:'custom-'+crypto.randomUUID(),revision:1};this.networkUndoAvailable=true;this.networkSelectedId=this.networkProfiles.activeProfile.id;this.renderPreview();
    }
  }
  async undoRemove():Promise<void> {
    if(!this.undo)return;
    const restored={...this.undo,name:this.uniqueName(this.undo.name)};
    if(await this.save([...this.networkProfiles.customProfiles,restored],null)) {this.networkSelectedId=restored.id;this.undo=null;this.networkUndoAvailable=false;this.renderPreview();}
  }
  private async save(profiles:NetworkProfileSnapshot[],useProfile:string|null):Promise<boolean> {
    if(this.networkBusy)return false;
    this.networkBusy=true;this.networkError='';this.renderPreview();
    try {
      this.networkProfiles=await invoke<NetworkProfiles>('save_network_profiles',{expectedRevision:this.networkProfiles.revision,profiles,useProfile});
      this.networkText=useProfile?'Profile in use for new requests. Existing requests finish normally.':'Profile saved. The active recipe is unchanged.';
      this.$emit('network-profile-changed',structuredClone(this.networkProfiles));
      return true;
    } catch(error:unknown) {
      this.networkError=describeError(error);
      await this.showError('Network profile could not be saved',this.networkError);
      return false;
    } finally {this.networkBusy=false;this.renderPreview();}
  }
}
NetworkProfileSettingsPanel.define('network-profile-settings');
