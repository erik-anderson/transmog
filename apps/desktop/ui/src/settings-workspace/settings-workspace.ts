import initialState from '../initial-state.json';
import { attr, observable } from '@microsoft/webui-framework';
import { invoke } from '@tauri-apps/api/core';
import { WorkspaceElement } from '../workspace-element.js';
import type { AppStatus, CaIdentity, DesktopBootstrap, ProductState, WorkspacePreferences } from '../models.js';
import { describeError } from '../utilities.js';

export class SettingsWorkspace extends WorkspaceElement {
  @attr view = 'traffic';
  @observable bufferMode='automatic';
  @observable bufferSummary='Automatic uses half of installed RAM and keeps bodies in memory.';
  updateBufferMode():void {this.bufferMode=(this.settingsForm.elements.namedItem('bufferMode') as HTMLSelectElement).value;}
  private async refreshBufferStatus():Promise<void> {
    try {
      const status=await invoke<{installedRam:number|null;maxBytes:number|null;storage:string;retainedBytes:number}|null>('buffer_status');
      if(status) {
        const format=(bytes:number)=> (bytes/1073741824).toLocaleString(undefined,{maximumFractionDigits:2})+' GiB';
        this.bufferSummary='Current buffer: '+(status.maxBytes===null?'No maximum':format(status.maxBytes)+' maximum')+' · '+(status.storage==='memory'?'Memory storage':'Disk storage')+(status.installedRam===null?' · Installed RAM unavailable; automatic uses 1 GiB.':' · '+format(status.installedRam)+' installed RAM');
      }
    } catch {this.bufferSummary='Buffer status unavailable. Automatic uses half installed RAM; larger or unlimited buffers write to disk.';}
  }
  @observable settingsSection='preferences';
  @observable settingsDirty=false;
  @observable settingsBusy=false;
  @observable settingsText='';
  @observable settingsError='';
  @observable certificateReady=false;
  @observable certificateRecovery=false;
  @observable hostRecoveryPending=false;
  @observable supportBusy=false;
  @observable supportPathsAllowed=false;
  @observable supportDetails='';
  @observable supportFacts:Array<{label:string;value:string}>=[];
  showSettingsSection(section:string):void {this.settingsSection=section;}
  markSettingsDirty():void {this.settingsDirty=true;this.settingsText='Unsaved changes';}
  settingsKeyboard(event:KeyboardEvent):void {if((event.ctrlKey||event.metaKey)&&event.key.toLowerCase()==='s'){event.preventDefault();if(this.settingsDirty)this.settingsForm.requestSubmit();}}
  private updateCertificateState(bootstrap:DesktopBootstrap):void {this.certificateReady=bootstrap.caFilesPresent&&bootstrap.ownedCaTrusted;this.certificateRecovery=bootstrap.caFilesExist&&(!bootstrap.caFilesPresent||bootstrap.ownedCaSha256===null)||!bootstrap.caFilesPresent&&bootstrap.ownedCaSha256!==null;this.hostRecoveryPending=bootstrap.hostRestorePending;}
  private async runSupport(operation:()=>Promise<void>):Promise<void> {if(this.supportBusy)return;this.supportBusy=true;try {await operation();}finally{this.supportBusy=false;}}
  async chooseSupportPath():Promise<void> {await this.runSupport(async()=>{try {const path=await invoke<string|null>('pick_support_path');if(path!==null){const input=this.supportForm.elements.namedItem('destination') as HTMLInputElement;input.value=path;input.focus();}}catch(error:unknown){this.supportText='File selection failed: '+describeError(error);}});}
  @observable proxyReady = initialState.proxyReady;
  @observable proxyText = initialState.proxyText;
  @observable proxyKind = initialState.proxyKind;
  @observable supportText = initialState.supportText;
  @observable diagnosticsPathText = initialState.diagnosticsPathText;
  @observable certificatePath = initialState.certificatePath;
  @observable privateKeyPath = initialState.privateKeyPath;
  @observable caSha256 = initialState.caSha256;
  @observable proxyLifecycle = 'stopped';
  @observable proxyPending = '';
  proxyForm!: HTMLFormElement;
  settingsForm!: HTMLFormElement;
  supportForm!: HTMLFormElement;
  ready!: Promise<void>;
  private drainTimer: number | undefined;
  private statusRevision = 0;

  protected hydratedCallback(): void { this.ready = this.initializeShell(); }
  private renderAppStatus(status: AppStatus): void {
    this.statusRevision++;
    this.hostRecoveryPending = status.hostRestorePending;
    this.proxyLifecycle = status.lifecycle;
    this.$emit('status-changed', status);
    window.clearTimeout(this.drainTimer);
    if (status.lifecycle === 'draining' || status.lifecycle === 'stopping') {
      this.drainTimer = window.setTimeout(() => {
        if (this.isConnected) void this.refreshStatus();
      }, 750);
    }
  }
  disconnectedCallback(): void {
    window.clearTimeout(this.drainTimer);
    super.disconnectedCallback();
  }
  private applyTheme(theme: ProductState['preferences']['theme'], workspace?: WorkspacePreferences): void {
    this.$emit('preferences-changed', { theme, pageSize: Number((this.settingsForm.elements.namedItem('pageSize') as HTMLInputElement).value), workspace });
  }
  dismissNotice(): void { this.$emit('dismiss-notice'); }
  private async initializeShell(): Promise<void> {
    this.setProxyOutput('Loading durable proxy state…', 'progress');
    try {
      const [bootstrap, productState, status] = await Promise.all([
        invoke<DesktopBootstrap>('desktop_bootstrap'),
        invoke<ProductState>('product_state'),
        invoke<AppStatus>('app_status'),
      ]);
      this.certificatePath = bootstrap.caCertificatePath;
      this.privateKeyPath = bootstrap.caPrivateKeyPath;
      this.caSha256 = bootstrap.ownedCaSha256 ?? '';
      this.diagnosticsPathText = bootstrap.diagnosticsPath;
      this.populateSettings(productState);
      void this.refreshBufferStatus();
      this.applyTheme(productState.preferences.theme, productState.workspace);
      this.renderAppStatus(status);
      this.updateCertificateState(bootstrap);
      const recovery = this.caRecoveryRequired(bootstrap);
      const certificate = recovery
        ? 'The saved interception certificate needs to be reset. Reset certificate and set up again to create and trust a new one.'
        : bootstrap.ownedCaSha256 === null
          ? 'No app-owned trusted CA is recorded. Create and trust one before intercepting HTTPS.'
          : bootstrap.ownedCaTrusted
            ? 'HTTPS interception certificate is ready.'
            : 'The interception certificate needs current-user trust.';
      const ready = bootstrap.caFilesPresent && bootstrap.ownedCaTrusted;
      this.setProxyOutput(certificate, ready ? 'success' : recovery ? 'error' : 'progress');
      if (bootstrap.hostRestorePending) {
        this.showNotice(
          'Windows proxy recovery required',
          'Transmog could not restore the settings journaled by a previous run. Restore them before starting again.',
          'Restore settings',
          'recover-proxy',
        );
      } else if (recovery) {
        this.showCaRecovery(bootstrap);
      } else if (!ready) {
        this.showNotice(
          'HTTPS interception needs setup',
          'Before the proxy can start, create and trust Transmog’s local interception certificate. Windows will ask you to approve adding it to your current-user trusted roots.',
          'Set up now',
          'setup-ca',
        );
      }
    } catch (error: unknown) {
      const message = `Desktop initialization failed: ${describeError(error)}`;
      this.setProxyOutput(message, 'error');
      this.diagnostic = message;
      await this.reportFrontendIssue('desktop-initialization-failed', error);
    } finally {
      this.proxyReady = true;
      this.$emit('proxy-ready');
    }
  }

  async toggleProxy(): Promise<void> { if (this.proxyLifecycle === 'running') await this.stopProxy(); else await this.startProxy(); }
  async startProxy(event?: Event): Promise<void> {
    event?.preventDefault();
    await this.runProxyChange('starting', () => this.startProxyNow());
  }
  async stopProxy(): Promise<void> { await this.runProxyChange('stopping', () => this.stopProxyNow()); }
  private async runProxyChange(kind: string, operation: () => Promise<void>): Promise<void> {
    if (this.proxyPending) return;
    this.statusRevision++;
    window.clearTimeout(this.drainTimer);
    this.proxyPending = kind;
    this.$emit('proxy-operation',kind);
    try { await operation(); await this.refreshStatus(); }
    finally { this.proxyPending = ''; this.$emit('proxy-operation',''); }
  }

  private async startProxyNow(event?: Event): Promise<void> {
    event?.preventDefault();

    const data = new FormData(this.proxyForm);
    try {
      if (this.proxyLifecycle === 'draining') {
        const resumed = await invoke<AppStatus>('resume_application');
        this.renderAppStatus(resumed);
        if (resumed.lifecycle === 'running') {
          this.setProxyOutput('Proxy resumed. Existing requests and connections remain active.', 'success');
          return;
        }
      }
      const bootstrap = await invoke<DesktopBootstrap>('desktop_bootstrap');
      this.updateCertificateState(bootstrap);
      if (bootstrap.hostRestorePending) {
        this.showNotice(
          'Windows proxy recovery required',
          'Restore the exact settings journaled by the previous run before starting the proxy again.',
          'Restore settings',
          'recover-proxy',
        );
        this.setProxyOutput('Start blocked: Windows proxy recovery is pending.', 'error');
        return;
      }
      if (!bootstrap.caFilesPresent || bootstrap.ownedCaSha256 === null || !bootstrap.ownedCaTrusted) {
        if (this.caRecoveryRequired(bootstrap)) {
          this.showCaRecovery(bootstrap);
          this.activateView('settings');
          return;
        }
        const missing = !bootstrap.caFilesPresent || bootstrap.ownedCaSha256 === null
          ? 'Create and trust Transmog’s interception certificate before starting the proxy.'
          : 'Transmog’s interception certificate is not trusted for the current user. Trust it before starting the proxy.';
        this.showNotice('Proxy setup required', missing, 'Set up HTTPS interception', 'setup-ca');
        this.setProxyOutput(`Start blocked: ${missing}`, 'error');
        this.activateView('settings');
        return;
      }
      this.setProxyOutput('Starting capture, binding the proxy, and applying current-user Windows proxy settings…', 'progress');
      const status = await invoke<AppStatus>('start_proxy', {
        request: {
          caCertificatePath: String(data.get('certificate') ?? ''),
          caPrivateKeyPath: String(data.get('privateKey') ?? ''),
          listen: '127.0.0.1:0',
          route: 'auto',
          allowRemoteClients: false,
        },
      });
      this.renderAppStatus(status);
      const listener = status.listener ?? 'an unknown listener';
      this.dismissNotice();
      this.setProxyOutput(`Proxy listening at ${listener}. Windows proxy settings are active and traffic is being captured automatically.`, 'success');
    } catch (error: unknown) {
      const message = `Start failed: ${describeError(error)}`;
      this.setProxyOutput(message, 'error');
      this.diagnostic = message;
      this.showNotice('Proxy could not start', `${message} See Settings for recovery options.`, 'Open Settings', 'settings');
    }
  }

  async setupCa(): Promise<void> {
    await this.runProxyChange('certificate-setup', () => this.setupCaNow());
  }

  private caRecoveryRequired(bootstrap: DesktopBootstrap): boolean {
    return bootstrap.caFilesExist && (!bootstrap.caFilesPresent || bootstrap.ownedCaSha256 === null)
      || !bootstrap.caFilesPresent && bootstrap.ownedCaSha256 !== null;
  }

  private showCaRecovery(bootstrap: DesktopBootstrap): void {
    const reason = bootstrap.ownedCaSha256 === null
      ? 'Certificate files from a previous setup remain, but their Transmog identity is not recorded.'
      : 'The recorded interception certificate is missing one or both of its files.';
    const message = `${reason} Reset the certificate to remove the app-managed certificate and private key, then create and trust a new certificate.`;
    this.setProxyOutput(message, 'error');
    this.showNotice('Interception certificate needs a reset', message, 'Reset certificate and set up again', 'reset-ca');
  }

  async resetCa(): Promise<void> {
    await this.runProxyChange('certificate-setup', async () => {
      try {
        const bootstrap = await invoke<DesktopBootstrap>('desktop_bootstrap');
        const proceed = window.confirm(
          `Reset HTTPS interception? This permanently deletes Transmog’s app-managed certificate and private key:\n${bootstrap.caCertificatePath}\n${bootstrap.caPrivateKeyPath}\n\nAny exact certificate recorded by Transmog will also be removed from your current-user trusted roots. Transmog will create a new certificate and ask Windows to trust it. You must approve the Windows certificate dialog. Continue?`,
        );
        if (!proceed) return;
        this.setProxyOutput('Removing the previous interception certificate and private key…', 'progress');
        await invoke<void>('reset_ca');
        this.certificatePath = bootstrap.caCertificatePath;
        this.privateKeyPath = bootstrap.caPrivateKeyPath;
        this.caSha256 = '';
        this.$flushUpdates();
        const elements = this.proxyForm.elements;
        (elements.namedItem('certificate') as HTMLInputElement).value = this.certificatePath;
        (elements.namedItem('privateKey') as HTMLInputElement).value = this.privateKeyPath;
        (elements.namedItem('thumbprint') as HTMLInputElement).value = '';
        await this.setupCaNow(true);
      } catch (error: unknown) {
        const message = `Certificate reset failed: ${describeError(error)}`;
        this.setProxyOutput(message, 'error');
        this.showNotice('Certificate reset needs attention', message, 'Reset certificate and set up again', 'reset-ca');
      }
    });
  }

  private async setupCaNow(confirmed = false): Promise<void> {
    this.setProxyOutput('Preparing the interception certificate…', 'progress');
    try {
      let bootstrap = await invoke<DesktopBootstrap>('desktop_bootstrap');
      this.updateCertificateState(bootstrap);
      if (this.caRecoveryRequired(bootstrap)) {
        this.showCaRecovery(bootstrap);
        return;
      }
      if (!confirmed && !window.confirm(
        'Transmog will create a durable local interception certificate if needed, then ask Windows to trust its public certificate for your account. You must manually approve the Windows certificate dialog. Continue?',
      )) {
        this.setProxyOutput('Certificate setup canceled.', 'progress');
        return;
      }
      const data = new FormData(this.proxyForm);
      let sha256 = bootstrap.ownedCaSha256;
      if (!bootstrap.caFilesPresent) {
        const identity = await invoke<CaIdentity>('create_ca', {
          request: {
            certificatePath: String(data.get('certificate') ?? ''),
            privateKeyPath: String(data.get('privateKey') ?? ''),
            commonName: 'Transmog local interception CA',
            validityDays: 3650,
          },
        });
        sha256 = identity.sha256;
        this.caSha256 = identity.sha256;
      }
      if (sha256 === null) {
        throw new Error('The interception certificate identity is unavailable. Reset certificate and set up again.');
      }
      this.setProxyOutput('Approve the Windows certificate dialog to trust the Transmog interception CA.', 'progress');
      await invoke<void>('install_certificate', {
        path: String(data.get('certificate') ?? ''),
        sha256,
      });
      bootstrap = await invoke<DesktopBootstrap>('desktop_bootstrap');
      if (!bootstrap.ownedCaTrusted) throw new Error('Windows did not report the certificate as trusted.');
      this.caSha256 = sha256;
      const message = 'HTTPS interception is ready. Windows trusts the exact Transmog certificate for the current user.';
      this.setProxyOutput(message, 'success');
      this.showNotice('HTTPS interception is ready', 'You can start the proxy now.', 'Start proxy', 'start-proxy');
    } catch (error: unknown) {
      const message = `Certificate setup failed: ${describeError(error)}`;
      this.setProxyOutput(message, 'error');
      this.showNotice('Certificate setup needs attention', message, 'Try again', 'setup-ca');
    }
  }

  async createCa(): Promise<void> {
    const data = new FormData(this.proxyForm);
    try {
      const identity = await invoke<CaIdentity>('create_ca', {
        request: {
          certificatePath: String(data.get('certificate') ?? ''),
          privateKeyPath: String(data.get('privateKey') ?? ''),
          commonName: 'Transmog local interception CA',
          validityDays: 3650,
        },
      });
      this.caSha256 = identity.sha256;
      const message = `CA created with a current-user-only private-key ACL. SHA-256 ${identity.sha256}. Trust the public CA before intercepting HTTPS.`;
      this.setProxyOutput(message, 'success');
      this.diagnostic = message;
    } catch (error: unknown) {
      const message = `CA creation failed: ${describeError(error)}`;
      this.setProxyOutput(message, 'error');
      this.diagnostic = message;
    }
  }

  async installCa(): Promise<void> {
    const data = new FormData(this.proxyForm);
    try {
      await invoke<void>('install_certificate', {
        path: String(data.get('certificate') ?? ''),
        sha256: String(data.get('thumbprint') ?? ''),
      });
      const message = 'The exact public CA is trusted for the current user and its SHA-256 identity will be restored after restart.';
      this.setProxyOutput(message, 'success');
      this.diagnostic = message;
    } catch (error: unknown) {
      const message = `CA installation failed: ${describeError(error)}`;
      this.setProxyOutput(message, 'error');
      this.diagnostic = message;
    }
  }

  async removeCa(): Promise<void> {
    await this.runProxyChange('certificate-removal', () => this.removeCaNow());
  }

  private async removeCaNow(): Promise<void> {
    const data = new FormData(this.proxyForm);
    try {
      await invoke<void>('remove_certificate', { sha256: String(data.get('thumbprint') ?? '') });
      this.updateCertificateState(await invoke<DesktopBootstrap>('desktop_bootstrap'));
      const message = 'The exact public CA was removed from current-user trust. The durable CA files and identity were retained so setup can trust it again.';
      this.setProxyOutput(message, 'success');
      this.diagnostic = message;
    } catch (error: unknown) {
      const message = `CA removal failed: ${describeError(error)}`;
      this.setProxyOutput(message, 'error');
      this.diagnostic = message;
    }
  }

  private async stopProxyNow(): Promise<void> {
    this.setProxyOutput('Stopping and restoring host settings…', 'progress');
    try {
      const status = await invoke<AppStatus>('stop_application');
      this.renderAppStatus(status);
      this.setProxyOutput(status.lifecycle==='draining'?'Proxy routing is off. Existing requests and active connections will finish; Start proxy resumes the same listener.':'Proxy stopped and any current-user Windows proxy changes were restored.', status.lifecycle==='draining'?'progress':'success');
    } catch (error: unknown) {
      const message = `Stop failed: ${describeError(error)}`;
      this.setProxyOutput(message, 'error');
      this.diagnostic = message;
    }
  }

  async recoverProxy(): Promise<void> {
    try {
      const restored = await invoke<boolean>('recover_windows_proxy');
      const message = restored
        ? 'The exact journaled Windows proxy settings were restored.'
        : 'No Windows proxy recovery journal was present.';
      this.setProxyOutput(message, 'success');
      this.diagnostic = message;
    } catch (error: unknown) {
      const message = `Recovery failed: ${describeError(error)}`;
      this.setProxyOutput(message, 'error');
      this.diagnostic = message;
    }
  }

  async loadSettings():Promise<void> {if(this.settingsBusy)return;this.settingsBusy=true;this.settingsError='';try {const state=await invoke<ProductState>('product_state');this.populateSettings(state);this.applyTheme(state.preferences.theme,state.workspace);this.settingsText='Changes reverted.';}catch(error:unknown){this.settingsError='Settings could not be loaded: '+describeError(error);}finally{this.settingsBusy=false;}}
  async saveSettings(event:Event):Promise<void> {event.preventDefault();if(this.settingsBusy)return;const data=new FormData(this.settingsForm);this.settingsBusy=true;this.settingsError='';try {const state=await invoke<ProductState>('product_state');state.preferences.theme=String(data.get('theme')) as ProductState['preferences']['theme'];state.preferences.sessionPageSize=Number(data.get('pageSize'));state.preferences.configureSystemProxy=true;state.privacy.bufferLimit=this.bufferMode==='custom'?{mode:'custom',bytes:Math.round(Number(data.get('bufferSize'))*1073741824)}:{mode:this.bufferMode==='unlimited'?'unlimited':'automatic'};state.privacy.retainRequestBodies=data.get('requestBodies')==='on';state.privacy.requestBodyLimit=data.get('requestBodyLimit')==='unlimited'?null:25000000;state.privacy.redactSensitiveHeaders=data.get('redactHeaders')==='on';state.privacy.retainResponseBodies=data.get('defaultBodies')==='on';state.privacy.retainBodySamples=data.get('captureBodies')==='on';state.privacy.rememberRecentArtifacts=data.get('rememberArtifacts')==='on';state.privacy.includePathsInSupportBundles=data.get('supportPaths')==='on';const saved=await invoke<ProductState>('save_product_state',{productState:state});this.populateSettings(saved);this.applyTheme(saved.preferences.theme,saved.workspace);await this.refreshBufferStatus();this.settingsText='Settings saved.';}catch(error:unknown){this.settingsError='Settings save failed: '+describeError(error);}finally{this.settingsBusy=false;}}

  async refreshDiagnostics():Promise<void> {await this.runSupport(async()=>{try {const report=await invoke<{applicationVersion:string;runtime:{operatingSystem:string;architecture:string;webviewVersion:string|null};events:Array<{level:string}>;privacyNotice:string}>('diagnostics_report');this.supportFacts=[{label:'Transmog version',value:report.applicationVersion},{label:'Operating system',value:report.runtime.operatingSystem},{label:'Architecture',value:report.runtime.architecture},{label:'WebView2',value:report.runtime.webviewVersion??'Unavailable'},{label:'Recent events',value:String(report.events.length)},{label:'Warnings / errors',value:String(report.events.filter(event=>event.level!=='info').length)},{label:'Privacy',value:report.privacyNotice}];this.supportDetails=JSON.stringify(report,null,2);this.supportText='Diagnostics refreshed.';}catch(error:unknown){this.supportText='Diagnostics unavailable: '+describeError(error);}});}
  async createSupportBundle(event:Event):Promise<void> {event.preventDefault();const data=new FormData(this.supportForm);await this.runSupport(async()=>{try {const result=await invoke<{destination:string;bytes:number;includedRecentPaths:boolean}>('create_support_bundle',{destination:String(data.get('destination')??''),includeRecentPaths:data.get('includePaths')==='on'});this.supportFacts=[{label:'Saved to',value:result.destination},{label:'Size',value:result.bytes.toLocaleString()+' bytes'},{label:'Recent paths',value:result.includedRecentPaths?'Included by your saved privacy preference':'Excluded'}];this.supportDetails=JSON.stringify(result,null,2);this.supportText='Support bundle saved.';}catch(error:unknown){this.supportText='Support bundle failed: '+describeError(error);}});}
  async prepareUpdate():Promise<void> {await this.runSupport(async()=>{try {await invoke<AppStatus>('prepare_update_handoff');this.renderAppStatus(await invoke<AppStatus>('app_status'));this.supportText='Proxy, recording, breakpoints and Windows host changes are stopped. Close Transmog before running the installer.';}catch(error:unknown){this.supportText='Update handoff failed: '+describeError(error);}});}

  async refreshStatus(): Promise<string | null> {
    const revision = this.statusRevision;
    try {
      const status = await invoke<AppStatus>('app_status');
      if (revision === this.statusRevision) this.renderAppStatus(status);
      return null;
    } catch (error: unknown) {
      const message = `Status unavailable: ${describeError(error)}`;
      this.diagnostic = message;
      return message;
    }
  }

  private populateSettings(state: ProductState): void {
    this.settingsDirty=false;this.supportPathsAllowed=state.privacy.includePathsInSupportBundles;
    const elements = this.settingsForm.elements;
    this.bufferMode=state.privacy.bufferLimit?.mode??'automatic';
    (elements.namedItem('bufferMode') as HTMLSelectElement).value=this.bufferMode;
    (elements.namedItem('bufferSize') as HTMLInputElement).value=String(state.privacy.bufferLimit?.mode==='custom'?state.privacy.bufferLimit.bytes/1073741824:1);
    (elements.namedItem('theme') as HTMLSelectElement).value = state.preferences.theme;
    (elements.namedItem('pageSize') as HTMLInputElement).value = String(state.preferences.sessionPageSize);
    (elements.namedItem('requestBodies') as HTMLInputElement).checked = state.privacy.retainRequestBodies ?? true;
    (elements.namedItem('requestBodyLimit') as HTMLSelectElement).value=state.privacy.requestBodyLimit===null?'unlimited':'25000000';
    (elements.namedItem('redactHeaders') as HTMLInputElement).checked = state.privacy.redactSensitiveHeaders ?? false;
    (elements.namedItem('defaultBodies') as HTMLInputElement).checked = state.privacy.retainResponseBodies;
    (elements.namedItem('captureBodies') as HTMLInputElement).checked = state.privacy.retainBodySamples;
    (elements.namedItem('rememberArtifacts') as HTMLInputElement).checked = state.privacy.rememberRecentArtifacts;
    (elements.namedItem('supportPaths') as HTMLInputElement).checked = state.privacy.includePathsInSupportBundles;
  }

  private setProxyOutput(message: string, kind: 'progress' | 'success' | 'error'): void {
    this.proxyText = message;
    this.proxyKind = kind;
  }

}

SettingsWorkspace.define('settings-workspace');
