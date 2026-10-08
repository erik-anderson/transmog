import initialState from '../initial-state.json';
import '../autoresponse-switch/autoresponse-switch.js';
import { WebUIElement, attr, observable } from '@microsoft/webui-framework';
import { invoke } from '@tauri-apps/api/core';
import type { AppStatus, AutomationStatus, Notice, NoticeAction, ProductState, SelectedResponse, SessionDetail, ViewName, WorkspacePreferences } from '../models.js';
import { defaultWorkspace, normalizeWorkspace } from '../table-model.js';
import type { TrafficWorkspace } from '../traffic-workspace/traffic-workspace.js';
import type { SettingsWorkspace } from '../settings-workspace/settings-workspace.js';
import type { AutomationWorkspace } from '../automation-workspace/automation-workspace.js';
import type { ComposerWorkspace } from '../composer-workspace/composer-workspace.js';
import { describeError, lifecycleLabel } from '../utilities.js';

const loaders = {
  breakpoints: () => import('../breakpoint-workspace/breakpoint-workspace.js'),
  automation: () => import('../automation-workspace/automation-workspace.js'),
  composer: () => import('../composer-workspace/composer-workspace.js'),
  captures: () => import('../capture-workspace/capture-workspace.js'),
};

/** Composition, shared status, and local workspace selection. */
export class AppShell extends WebUIElement {
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
  @observable navigationExpanded = 'true';
  traffic!: TrafficWorkspace;
  settings!: SettingsWorkspace;
  automation!: AutomationWorkspace;
  composer!: ComposerWorkspace;
  private noticeAction: NoticeAction = null;
  private navigationGeneration = 0;
  private workspaceTouched = false;
  private workspaceLoaded = false;
  private saveTimer: number | undefined;
  private saving = false;
  private saveAgain = false;
  workspaceChanged():void { this.navigationExpanded = this.workspace.sidebarCollapsed ? 'false' : 'true'; }
  protected hydratedCallback():void {void this.refreshAutoresponses();}
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
    this.currentNavigation = Object.fromEntries(Object.keys(initialState.currentNavigation).map((view) => [view, view === this.activeView ? 'page' : 'false']));
  }

  onDiagnostic(event: CustomEvent<string>): void { this.diagnosticText = event.detail; }
  onNotice(event: CustomEvent<Notice>): void {
    this.noticeTitleText = event.detail.title;
    this.noticeMessageText = event.detail.message;
    this.noticeActionLabel = event.detail.actionLabel ?? '';
    this.noticeAction = event.detail.action;
    this.noticeVisible = true;
  }
  onStatus(event: CustomEvent<AppStatus>): void { this.applyStatus(event.detail); }
  private applyStatus(status: AppStatus): void {
    this.lifecycleLabel = lifecycleLabel(status.lifecycle);
    this.lifecycleKind = status.lifecycle;
    this.listener = status.listener ?? 'Not listening';
    this.diagnosticText = status.hostRestorePending ? 'Host restoration is pending and must be retried before restart.' : status.summary;
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
  onNavigate(event: CustomEvent<ViewName>): void { void this.activateView(event.detail); }
  showView(event: Event): void {
    event.preventDefault();
    const view = (event.currentTarget as HTMLAnchorElement).dataset.view;
    if (view && ['traffic', 'breakpoints', 'automation', 'composer', 'captures', 'settings'].includes(view)) void this.activateView(view as ViewName);
  }

  private async activateView(view: ViewName): Promise<boolean> {
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
    await this.settings.refreshStatus();
  }
  runNoticeAction(): void {
    const action = this.noticeAction;
    this.dismissNotice();
    if (action === 'settings') void this.activateView('settings');
    else if (action === 'setup-ca') void this.settings.setupCa();
    else if (action === 'start-proxy') void this.startProxy();
    else if (action === 'recover-proxy') void this.settings.recoverProxy();
  }
  dismissNotice(): void { this.noticeAction = null; this.noticeVisible = false; }
  async onAutoResponse(event: CustomEvent<SelectedResponse>): Promise<void> {
    if (await this.activateView('automation')) await this.automation.populateCapturedAutoResponse(event.detail.sessionId, event.detail.detail);
  }
  async onAutoResponseBatch(event:CustomEvent<string[]>):Promise<void> {if(await this.activateView('automation'))await this.automation.beginBatch(event.detail);}
  async onSourceTraffic(event: CustomEvent<string>): Promise<void> {
    if (await this.activateView('traffic')) {
      if (!await this.traffic.revealSession(event.detail)) await this.automation.refreshSourceAvailability();
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
  disconnectedCallback(): void { window.clearTimeout(this.saveTimer); super.disconnectedCallback(); }
  allowSessionDrop(event: DragEvent): void {
    if (event.dataTransfer?.types.some(type=>type==='application/x-transmog-session' || type==='application/x-transmog-sessions')) event.preventDefault();
  }
  async dropSession(event: DragEvent): Promise<void> {
    event.preventDefault();
    const ids=event.dataTransfer?.getData('application/x-transmog-sessions');
    const id = event.dataTransfer?.getData('application/x-transmog-session');
    if(await this.activateView('automation')) {
      if(ids) {try {await this.automation.beginBatch(JSON.parse(ids));}catch(error:unknown){this.diagnosticText='Responses could not be prepared: '+describeError(error);}}
      else if(id)await this.automation.beginAutoResponseFromSessionId(id);
    }
  }
}

AppShell.define('app-shell');
