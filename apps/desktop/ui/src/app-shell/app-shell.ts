import initialState from '../initial-state.json';
import '../autoresponse-switch/autoresponse-switch.js';
import '../app-updates/app-updates.js';
import { WebUIElement, attr, observable } from '@microsoft/webui-framework';
import { invoke } from '@tauri-apps/api/core';
import type { AppStatus, AutomationStatus, Notice, NoticeAction, ProductState, SelectedResponse, SessionDetail, ViewName, WorkspacePreferences, TracePasswordPrompt, OperationError } from '../models.js';
import { defaultWorkspace, normalizeWorkspace } from '../table-model.js';
import type { TrafficWorkspace } from '../traffic-workspace/traffic-workspace.js';
import type { SettingsWorkspace } from '../settings-workspace/settings-workspace.js';
import type { AutomationWorkspace } from '../automation-workspace/automation-workspace.js';
import type { ComposerWorkspace } from '../composer-workspace/composer-workspace.js';
import type { AppUpdates } from '../app-updates/app-updates.js';
import { describeError, lifecycleLabel } from '../utilities.js';

const loaders = {
  breakpoints: () => import('../breakpoint-workspace/breakpoint-workspace.js'),
  automation: () => import('../automation-workspace/automation-workspace.js'),
  composer: () => import('../composer-workspace/composer-workspace.js'),
  captures: () => import('../capture-workspace/capture-workspace.js'),
};

/** Composition, shared status, and local workspace selection. */
export class AppShell extends WebUIElement {
  @observable errorTitle=initialState.errorTitle;
  @observable errorMessage=initialState.errorMessage;
  errorDialog!:HTMLDialogElement;
  workspaceContent!:HTMLElement;
  private operationErrors:OperationError[]=[];
  onOperationError(event:CustomEvent<OperationError>):void {
    event.stopPropagation();this.operationErrors.push(event.detail);
    if(this.operationErrors.length===1)this.showOperationError();
  }
  private showOperationError():void {
    const error=this.operationErrors[0];if(!error||!this.isConnected)return;
    this.errorTitle=error.title;this.errorMessage=error.message;this.$flushUpdates();this.errorDialog.showModal();
  }
  closeOperationError():void {this.errorDialog.close();}
  operationErrorClosed():void {
    if(this.errorDialog.open)return;
    this.operationErrors.shift()?.resolve();this.errorTitle='';this.errorMessage='';this.showOperationError();
  }
  dismissDiagnostic():void {this.diagnosticText='';this.workspaceContent.focus();}
  @observable passwordTitle='Capture password';
  @observable passwordConfirm=false;
  @observable passwordVisible=false;
  @observable passwordError='';
  passwordDialog!:HTMLDialogElement;
  passwordForm!:HTMLFormElement;
  private passwordRequest:TracePasswordPrompt|null=null;
  onTracePasswordRequest(event:CustomEvent<TracePasswordPrompt>):void {
    event.stopPropagation();if(this.passwordRequest){event.detail.resolve(null);return;}
    this.passwordRequest=event.detail;this.passwordTitle=event.detail.title;this.passwordConfirm=event.detail.confirm;this.passwordError=event.detail.message;this.passwordVisible=false;
    this.passwordForm.reset();this.setPasswordInputType(false);this.passwordDialog.showModal();
    (this.passwordForm.elements.namedItem('password') as HTMLInputElement).focus();
  }
  submitTracePassword(event:Event):void {
    event.preventDefault();const password=(this.passwordForm.elements.namedItem('password') as HTMLInputElement).value;
    if(this.passwordConfirm&&password!==(this.passwordForm.elements.namedItem('confirmPassword') as HTMLInputElement).value){this.passwordError='The passwords do not match.';return;}
    const request=this.passwordRequest;this.passwordRequest=null;this.passwordForm.reset();this.passwordDialog.close();request?.resolve(password);
  }
  cancelTracePassword():void {this.passwordDialog.close();}
  tracePasswordClosed():void {if(this.passwordDialog.open)return;const request=this.passwordRequest;this.passwordRequest=null;this.passwordForm.reset();this.passwordError='';this.passwordVisible=false;request?.resolve(null);}
  setPasswordVisible(event:Event):void {this.passwordVisible=(event.target as HTMLInputElement).checked;this.setPasswordInputType(this.passwordVisible);}
  private setPasswordInputType(visible:boolean):void {for(const name of ['password','confirmPassword'])(this.passwordForm.elements.namedItem(name) as HTMLInputElement).type=visible?'text':'password';}

  @observable viewerMode = false;
  @attr({ attribute: 'data-theme' }) theme: ProductState['preferences']['theme'] = 'system';
  @observable activeView: ViewName = initialState.activeView as ViewName;
  @observable currentNavigation: Record<string, string> = initialState.currentNavigation;
  @observable pageSize = initialState.pageSize;
  @observable lifecycleLabel = initialState.lifecycleLabel;
  @observable lifecycleKind = initialState.lifecycleKind;
  @observable listener = initialState.listener;
  @observable diagnosticText = initialState.diagnosticText;
  @observable proxyReady = initialState.proxyReady;
  @observable noticeVisible = initialState.noticeVisible;
  @observable noticeTitleText = initialState.noticeTitleText;
  @observable noticeMessageText = initialState.noticeMessageText;
  @observable noticeActionLabel = initialState.noticeActionLabel;
  @observable selection: SelectedResponse | null = initialState.selection;
  @observable autoresponseState:AutomationStatus|null=null;
  @observable autoresponsePending=false;
  @observable workspace = defaultWorkspace();
  @observable proxyPending = '';
  @observable pausedCount=0;
  onBreakpointState(event:CustomEvent<{count:number}>):void {this.pausedCount=event.detail.count;}
  @observable navigationExpanded = 'true';
  traffic!: TrafficWorkspace;
  settings!: SettingsWorkspace;
  automation!: AutomationWorkspace;
  composer!: ComposerWorkspace;
  updates!: AppUpdates;
  updateDraftDialog!: HTMLDialogElement;
  @observable updateDraftSummary = initialState.updateDraftSummary;
  private installPending = false;
  private resolveUpdateDrafts: ((proceed: boolean) => void) | undefined;
  async onInstallUpdate(event: CustomEvent<{reopen: boolean}>): Promise<void> {
    if (this.installPending) return;
    this.installPending = true;
    try {
      const dirty = [this.composer?.composerDirty ? 'Composer' : '', this.settings?.settingsDirty ? 'Settings' : '', this.automation?.draftDirty ? 'Autoresponses' : '', this.automation?.scriptEditor?.editorLoaded && this.automation.scriptEditor.scriptDirty ? 'Scripts' : ''].filter(Boolean);
      if (dirty.length) {
        this.updateDraftSummary = `There are unsaved changes in ${dirty.join(', ')}. Installing will close Transmog and discard those drafts. Keep working to save them first.`;
        const proceed = await new Promise<boolean>(resolve => { this.resolveUpdateDrafts = resolve; this.updateDraftDialog.showModal(); });
        if (!proceed) return;
      }
      await this.updates.install(event.detail.reopen);
      await this.settings.refreshStatus();
    } finally { this.installPending = false; }
  }
  resolveUpdateDraftsChoice(proceed: boolean): void { this.updateDraftDialog.close(); this.resolveUpdateDrafts?.(proceed); this.resolveUpdateDrafts = undefined; }
  cancelUpdateDrafts(event: Event): void { event.preventDefault(); this.resolveUpdateDraftsChoice(false); }
  checkUpdates(): void { void this.updates.check(true); }
  private noticeAction: NoticeAction = null;
  private navigationGeneration = 0;
  private workspaceTouched = false;
  private workspaceLoaded = false;
  private saveTimer: number | undefined;
  private saving = false;
  private saveAgain = false;
  workspaceChanged():void { this.navigationExpanded = this.workspace.sidebarCollapsed ? 'false' : 'true'; }
  protected hydratedCallback():void {
    if(this.viewerMode)void invoke<ProductState>('product_state').then(state=>this.onPreferences(new CustomEvent('preferences-changed',{detail:{theme:state.preferences.theme,pageSize:state.preferences.sessionPageSize,workspace:state.workspace}}))).catch(error=>{this.diagnosticText=describeError(error);});
    else void this.refreshAutoresponses();
  }
  async openMainWindow():Promise<void> {try{await invoke('open_main_window');}catch(error:unknown){this.diagnosticText='Main window could not be opened: '+describeError(error);}}
  onAutomationState(event:CustomEvent<AutomationStatus>):void {
    if(!this.autoresponseState || event.detail.generation>=this.autoresponseState.generation)this.autoresponseState=event.detail;
  }
  private async refreshAutoresponses():Promise<void> {
    try {this.onAutomationState(new CustomEvent('automation-state-change',{detail:await invoke<AutomationStatus>('automation_status')}));}
    catch(error:unknown){this.diagnosticText='Autoresponse state could not be loaded: '+describeError(error);}
  }
  async toggleAutoresponses():Promise<void> {
    if(this.autoresponsePending)return;this.autoresponsePending=true;
    try {
      const current=this.autoresponseState??await invoke<AutomationStatus>('automation_status');
      const status=await invoke<AutomationStatus>('set_autoresponses_enabled',{enabled:!(current.autoresponsesEnabled??true),generation:current.generation});
      this.autoresponseState=status;this.diagnosticText=status.autoresponsesEnabled?'Autoresponses resumed for new requests.':'Autoresponses paused. Individual rule states are preserved.';
    } catch(error:unknown){this.diagnosticText='Autoresponse switch failed: '+describeError(error);await this.refreshAutoresponses();}
    finally{this.autoresponsePending=false;}
  }

  activeViewChanged(): void {
    this.diagnosticText='';
    this.currentNavigation = Object.fromEntries(Object.keys(initialState.currentNavigation).map((view) => [view, view === this.activeView ? 'page' : 'false']));
  }

  onDiagnostic(event:CustomEvent<string>):void {const views:Record<string,string>={'traffic-workspace':'traffic','automation-workspace':'automation','composer-workspace':'composer','capture-workspace':'captures','breakpoint-workspace':'breakpoints','settings-workspace':'settings'};const source=event.composedPath().map(node=>node instanceof HTMLElement?views[node.localName]:undefined).find(Boolean);if(source==='traffic'&&event.detail===this.traffic.sessionText)return;if(!source||source===this.activeView)this.diagnosticText=event.detail;}
  onNotice(event: CustomEvent<Notice>): void {
    this.noticeTitleText = event.detail.title;
    this.noticeMessageText = event.detail.message;
    this.noticeActionLabel = event.detail.actionLabel ?? '';
    this.noticeAction = event.detail.action;
    this.noticeVisible = true;
  }
  onStatus(event: CustomEvent<AppStatus>): void { this.applyStatus(event.detail); }
  private applyStatus(status: AppStatus): void {
    this.lifecycleLabel = status.lifecycle==='draining'?status.summary:lifecycleLabel(status.lifecycle);
    this.lifecycleKind = status.lifecycle;
    this.listener = status.listener ?? 'Not listening';
    if(status.hostRestorePending)this.diagnosticText='Host restoration is pending and must be retried before restart.';
    else if(this.diagnosticText==='Host restoration is pending and must be retried before restart.')this.diagnosticText='';
  }
  onPreferences(event: CustomEvent<{theme: ProductState['preferences']['theme']; pageSize: number; workspace?: WorkspacePreferences}>): void {
    this.theme = event.detail.theme;
    this.pageSize = String(event.detail.pageSize);
    if (!this.workspaceLoaded && !this.workspaceTouched) this.workspace = normalizeWorkspace(event.detail.workspace);
    this.workspaceLoaded = true;
  }
  onProxyOperation(event: CustomEvent<string>): void { this.proxyPending = event.detail; }
  async toggleProxy(): Promise<void> {
    await this.settings.ready;
    if (this.lifecycleKind === 'running') await this.settings.stopProxy();
    else await this.settings.startProxy();
  }
  toggleNavigation(): void { this.changeWorkspace({sidebarCollapsed:!this.workspace.sidebarCollapsed}); }
  onWorkspaceChange(event: CustomEvent<{patch:Partial<WorkspacePreferences>; committed?: boolean}>): void { this.changeWorkspace(event.detail.patch,event.detail.committed !== false); }
  private changeWorkspace(patch: Partial<WorkspacePreferences>, committed = true): void {
    this.workspaceTouched = true;
    this.workspace = {...this.workspace,...patch};
    if (!committed) return;
    window.clearTimeout(this.saveTimer);
    this.saveTimer = window.setTimeout(() => { void this.saveWorkspace(); },250);
  }
  resetWorkspace(): void { this.changeWorkspace(defaultWorkspace()); }
  private async saveWorkspace(): Promise<void> {
    this.saveAgain = true;
    if (this.saving) return;
    this.saving = true;
    try {
      while (this.saveAgain && this.isConnected) {
        this.saveAgain = false;
        await invoke<WorkspacePreferences>('save_workspace_preferences',{preferences:this.workspace});
      }
    } catch (error: unknown) { this.diagnosticText = 'Layout could not be saved: '+describeError(error); }
    finally { this.saving = false; }
  }
  async copyListener(): Promise<void> {
    if (this.listener === 'Not listening') return;
    try { await navigator.clipboard.writeText(this.listener); this.diagnosticText = 'Proxy address copied.'; }
    catch { this.diagnosticText = 'Select and copy the proxy address shown in the header.'; }
  }
  onProxyReady(): void { this.proxyReady = true; }
  onSelection(event: CustomEvent<SelectedResponse | null>): void { this.selection = event.detail; }
  onNavigate(event:CustomEvent<ViewName>):void {if(event.detail==='settings')void this.openConnectionSettings();else void this.activateView(event.detail);}
  showView(event: Event): void {
    event.preventDefault();
    const view = (event.currentTarget as HTMLAnchorElement).dataset.view;
    if (view && ['traffic', 'breakpoints', 'automation', 'composer', 'captures', 'settings'].includes(view)) void this.activateView(view as ViewName);
  }

  private async activateView(view: ViewName): Promise<boolean> {
    if(this.viewerMode&&view!=='traffic'&&view!=='composer')return false;
    const generation = ++this.navigationGeneration;
    try {
      if (view in loaders) await loaders[view as keyof typeof loaders]();
      if (generation !== this.navigationGeneration || !this.isConnected) return false;
      this.activeView = view;
      this.$flushUpdates();
      return true;
    } catch (error: unknown) {
      this.diagnosticText = 'Workspace could not be loaded: ' + describeError(error);
      return false;
    }
  }

  async startProxy(event?: Event): Promise<void> { event?.preventDefault(); await this.settings.ready; await this.settings.startProxy(); }
  async stopProxy(): Promise<void> { await this.settings.ready; await this.settings.stopProxy(); }
  async refreshStatus(): Promise<void> {
    await this.settings.ready;
    const error=await this.settings.refreshStatus();
    if(error)this.diagnosticText=error;
    else if(this.diagnosticText!=='Host restoration is pending and must be retried before restart.')this.diagnosticText='Proxy '+this.lifecycleKind;
  }
  runNoticeAction(): void {
    const action = this.noticeAction;
    this.dismissNotice();
    if (action === 'settings') void this.openConnectionSettings();
    else if (action === 'setup-ca') void this.settings.setupCa();
    else if (action === 'reset-ca') void this.settings.resetCa();
    else if (action === 'start-proxy') void this.startProxy();
    else if (action === 'recover-proxy') void this.settings.recoverProxy();
  }
  async openConnectionSettings():Promise<void> {if(await this.activateView('settings'))this.settings.showSettingsSection('connection');}
  dismissNotice(): void { this.noticeAction = null; this.noticeVisible = false; }
  async onAutoResponse(event: CustomEvent<SelectedResponse>): Promise<void> {
    if (await this.activateView('automation')) await this.automation.populateCapturedAutoResponse(event.detail.sessionId, event.detail.detail);
  }
  async onAutoResponseBatch(event:CustomEvent<string[]>):Promise<void> {if(await this.activateView('automation'))await this.automation.beginBatch(event.detail);}
  async onSourceTraffic(event: CustomEvent<string>): Promise<void> {
    if (await this.activateView('traffic')) {
      if (!await this.traffic.revealSession(event.detail)) await this.automation.refreshSourceAvailability?.();
    }
  }
  onTrafficRefreshed(): void {
    if (this.activeView === 'automation') {void this.automation.refreshSourceAvailability();this.automation.refreshUsageSoon();}
  }
  async onMatchedRule(event: CustomEvent<string>): Promise<void> {
    if (await this.activateView('automation')) { await this.automation.refreshAutomation(); this.automation.showMatchedRule(event.detail); }
  }
  async onReplay(event: CustomEvent<SessionDetail>): Promise<void> {
    if (await this.activateView('composer')) await this.composer.populateRequest(event.detail);
  }
  disconnectedCallback(): void { window.clearTimeout(this.saveTimer); this.resolveUpdateDrafts?.(false); for(const error of this.operationErrors)error.resolve();this.operationErrors=[];super.disconnectedCallback(); }

}

AppShell.define('app-shell');
