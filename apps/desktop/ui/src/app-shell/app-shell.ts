import initialState from '../initial-state.json';
import { WebUIElement, attr, observable } from '@microsoft/webui-framework';
import { invoke } from '@tauri-apps/api/core';
import type { AppStatus, Notice, NoticeAction, ProductState, SelectedResponse, ViewName } from '../models.js';
import type { TrafficWorkspace } from '../traffic-workspace/traffic-workspace.js';
import type { SettingsWorkspace } from '../settings-workspace/settings-workspace.js';
import type { AutomationWorkspace } from '../automation-workspace/automation-workspace.js';
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
  traffic!: TrafficWorkspace;
  settings!: SettingsWorkspace;
  automation!: AutomationWorkspace;
  private noticeAction: NoticeAction = null;
  private navigationGeneration = 0;

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
  onPreferences(event: CustomEvent<{theme: ProductState['preferences']['theme']; pageSize: number}>): void {
    this.theme = event.detail.theme;
    this.pageSize = String(event.detail.pageSize);
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
    try { this.applyStatus(await invoke<AppStatus>('app_status')); }
    catch (error: unknown) { this.diagnosticText = 'Status unavailable: ' + describeError(error); }
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
  async onMatchedRule(event: CustomEvent<string>): Promise<void> {
    if (await this.activateView('automation')) { await this.automation.refreshAutomation(); this.automation.showMatchedRule(event.detail); }
  }
  allowSessionDrop(event: DragEvent): void {
    if (event.dataTransfer?.types.includes('application/x-transmog-session')) event.preventDefault();
  }
  async dropSession(event: DragEvent): Promise<void> {
    event.preventDefault();
    const id = event.dataTransfer?.getData('application/x-transmog-session');
    if (id && await this.activateView('automation')) await this.automation.beginAutoResponseFromSessionId(id);
  }
}

AppShell.define('app-shell');
