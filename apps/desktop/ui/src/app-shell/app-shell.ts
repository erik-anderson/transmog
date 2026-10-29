import { automationActivity } from "../automation-state.js";
import initialState from "../initial-state.json";
import "../comparison-workspace/comparison-workspace.js";
import type { ComparisonWorkspace } from "../comparison-workspace/comparison-workspace.js";
import type { ComparisonOpen, CaptureOpen } from "../comparison-models.js";
import "../autoresponse-switch/autoresponse-switch.js";
import "../app-updates/app-updates.js";
import "../client-certificate-requests/client-certificate-requests.js";
import "../mcp-indicator/mcp-indicator.js";
import "../session-review/session-review.js";
import "../captured-preview/captured-preview.js";
import type { CapturedPreview } from "../captured-preview/captured-preview.js";
import "../response-set-page-picker/response-set-page-picker.js";
import type { ResponseSetPagePicker } from "../response-set-page-picker/response-set-page-picker.js";
import "../response-set-intake/response-set-intake.js";
import type { ResponseSetIntake } from "../response-set-intake/response-set-intake.js";
import type { ResponseSetIntakeRequest } from "../models.js";
import type { SavedDemoPreviewIntent } from "../saved-demo-models.js";
import { WebUIElement, attr, observable } from "@microsoft/webui-framework";
import { invoke } from "@tauri-apps/api/core";
import type {
  TrafficRuleRequest,
  AppStatus,
  AutomationStatus,
  Notice,
  NoticeAction,
  ProductState,
  ResponsePreviewIntent,
  SelectedResponse,
  SessionDetail,
  ViewName,
  WorkspacePreferences,
  TracePasswordPrompt,
  OperationError,
} from "../models.js";
import { defaultWorkspace, normalizeWorkspace } from "../table-model.js";
import { isTextEditing } from "../list-selection.js";
import type { TrafficWorkspace } from "../traffic-workspace/traffic-workspace.js";
import type { SettingsWorkspace } from "../settings-workspace/settings-workspace.js";
import type { AutomationWorkspace } from "../automation-workspace/automation-workspace.js";
import type { ComposerWorkspace } from "../composer-workspace/composer-workspace.js";
import type { AppUpdates, UpdateStatus } from "../app-updates/app-updates.js";
import { describeError, lifecycleLabel } from "../utilities.js";

const loaders = {
  breakpoints: () => import("../breakpoint-workspace/breakpoint-workspace.js"),
  automation: () => import("../automation-workspace/automation-workspace.js"),
  composer: () => import("../composer-workspace/composer-workspace.js"),
  captures: () => import("../capture-workspace/capture-workspace.js"),
};

/** Composition, shared status, and local workspace selection. */
export class AppShell extends WebUIElement {
  comparison!: ComparisonWorkspace;
  onComparison(event: CustomEvent<ComparisonOpen>): void {
    event.stopPropagation();
    void this.comparison.open(event.detail);
  }
  onCaptureComparison(event: CustomEvent<CaptureOpen>): void {
    event.stopPropagation();
    void this.comparison.openCaptures(event.detail);
  }
  async onSaveComparisonCase(
    event: CustomEvent<{ id: string; returnFocus: HTMLElement }>,
  ): Promise<void> {
    event.stopPropagation();
    await this.traffic.showPortableCase("export", event.detail.id, event.detail.returnFocus);
  }
  onCaseComparison(
    event: CustomEvent<{
      id: string;
      kind: string;
      left: ComparisonOpen["left"];
      right: ComparisonOpen["right"];
      options: NonNullable<ComparisonOpen["options"]>;
    }>,
  ): void {
    event.stopPropagation();
    if (event.detail.kind === "captures") {
      void this.comparison.openCaseCaptures(event.detail.id);
    } else {
      void this.comparison.open({
        left: event.detail.left,
        right: event.detail.right,
        options: event.detail.options,
      });
    }
  }
  async onComparisonSource(event: CustomEvent<string>): Promise<void> {
    event.stopPropagation();
    try {
      if (!(await this.activateView("traffic"))) {
        this.comparison.comparisonStatus =
          "Navigation canceled; comparison and editor draft preserved.";
        return;
      }
      const focus = this.traffic.trackNavigationFocus();
      try {
        if (!(await this.traffic.revealSession(event.detail, focus.unchanged))) {
          this.comparison.comparisonStatus =
            "The source is no longer retained. Frozen comparison evidence remains available.";
          return;
        }
        const restore = focus.unchanged();
        this.comparison.sourceOpened();
        if (this.traffic.previewReportDialog.open) {
          this.traffic.closePreviewReport();
        }
        if (restore) {
          this.traffic.focusTraffic();
        }
      } finally {
        focus.dispose();
      }
    } catch (error: unknown) {
      this.comparison.comparisonStatus = "Source could not be opened: " + describeError(error);
    }
  }

  async openClientCertificateSettings(): Promise<void> {
    await this.activateView("settings");
    this.settings.showSettingsSection("client-certificates");
  }
  certificatePanelClosed(): void {
    this.workspaceContent.focus();
  }
  @observable automationEnabledCount = 0;
  @observable automationEnabledLabel = "";
  @observable enabledScriptCount = 0;
  private scriptGeneration = 0;
  private updateAutomationBadge(): void {
    const activity = automationActivity(this.autoresponseState, this.enabledScriptCount);
    this.automationEnabledCount = activity.total;
    this.automationEnabledLabel = activity.label;
  }
  onScriptActivity(event: CustomEvent<{ generation: number; activeCount: number }>): void {
    if (event.detail.generation < this.scriptGeneration) {
      return;
    }
    this.scriptGeneration = event.detail.generation;
    this.enabledScriptCount = event.detail.activeCount;
    this.updateAutomationBadge();
  }
  @observable errorTitle = initialState.errorTitle;
  @observable errorMessage = initialState.errorMessage;
  errorDialog!: HTMLDialogElement;
  workspaceContent!: HTMLElement;
  private operationErrors: OperationError[] = [];
  onOperationError(event: CustomEvent<OperationError>): void {
    event.stopPropagation();
    this.operationErrors.push(event.detail);
    if (this.operationErrors.length === 1) {
      this.showOperationError();
    }
  }
  private showOperationError(): void {
    const error = this.operationErrors[0];
    if (!error || !this.isConnected) {
      return;
    }
    this.errorTitle = error.title;
    this.errorMessage = error.message;
    this.$flushUpdates();
    this.errorDialog.showModal();
  }
  closeOperationError(): void {
    this.errorDialog.close();
  }
  operationErrorClosed(): void {
    if (this.errorDialog.open) {
      return;
    }
    this.operationErrors.shift()?.resolve();
    this.errorTitle = "";
    this.errorMessage = "";
    this.showOperationError();
  }
  dismissDiagnostic(): void {
    this.diagnosticText = "";
    this.workspaceContent.focus();
  }
  @observable passwordTitle = "Capture password";
  @observable passwordConfirm = false;
  @observable passwordVisible = false;
  @observable passwordError = "";
  passwordDialog!: HTMLDialogElement;
  passwordForm!: HTMLFormElement;
  private passwordRequest: TracePasswordPrompt | null = null;
  onTracePasswordRequest(event: CustomEvent<TracePasswordPrompt>): void {
    event.stopPropagation();
    if (this.passwordRequest) {
      event.detail.resolve(null);
      return;
    }
    this.passwordRequest = event.detail;
    this.passwordTitle = event.detail.title;
    this.passwordConfirm = event.detail.confirm;
    this.passwordError = event.detail.message;
    this.passwordVisible = false;
    this.passwordForm.reset();
    this.setPasswordInputType(false);
    this.passwordDialog.showModal();
    (this.passwordForm.elements.namedItem("password") as HTMLInputElement).focus();
  }
  submitTracePassword(event: Event): void {
    event.preventDefault();
    const password = (this.passwordForm.elements.namedItem("password") as HTMLInputElement).value;
    if (
      this.passwordConfirm &&
      password !==
        (this.passwordForm.elements.namedItem("confirmPassword") as HTMLInputElement).value
    ) {
      this.passwordError = "The passwords do not match.";
      return;
    }
    const request = this.passwordRequest;
    this.passwordRequest = null;
    this.passwordForm.reset();
    this.passwordDialog.close();
    request?.resolve(password);
  }
  cancelTracePassword(): void {
    this.passwordDialog.close();
  }
  tracePasswordClosed(): void {
    if (this.passwordDialog.open) {
      return;
    }
    const request = this.passwordRequest;
    this.passwordRequest = null;
    this.passwordForm.reset();
    this.passwordError = "";
    this.passwordVisible = false;
    request?.resolve(null);
  }
  setPasswordVisible(event: Event): void {
    this.passwordVisible = (event.target as HTMLInputElement).checked;
    this.setPasswordInputType(this.passwordVisible);
  }
  private setPasswordInputType(visible: boolean): void {
    for (const name of ["password", "confirmPassword"]) {
      (this.passwordForm.elements.namedItem(name) as HTMLInputElement).type = visible
        ? "text"
        : "password";
    }
  }

  @observable viewerMode = false;
  @attr({ attribute: "data-theme" }) theme: ProductState["preferences"]["theme"] = "system";
  @observable activeView: ViewName = initialState.activeView as ViewName;
  @observable currentNavigation: Record<string, string> = initialState.currentNavigation;
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
  @observable autoresponseState: AutomationStatus | null = null;
  @observable autoresponsePending = false;
  @observable workspace = defaultWorkspace();
  @observable proxyPending = "";
  @observable pausedCount = 0;
  onBreakpointState(event: CustomEvent<{ count: number }>): void {
    this.pausedCount = event.detail.count;
  }
  @observable navigationExpanded = "true";
  traffic!: TrafficWorkspace;
  @observable capturedPreviewController: TrafficWorkspace | null = null;
  capturedPreviewSurface!: CapturedPreview;
  onPreviewStateChange(): void {
    // WebUI observes each component's own fields. Re-evaluate direct controller
    // bindings explicitly rather than copying dialog state between components.
    this.capturedPreviewSurface?.$update();
    this.$update();
  }
  settings!: SettingsWorkspace;
  automation!: AutomationWorkspace;
  composer!: ComposerWorkspace;
  get updates(): AppUpdates {
    return this.settings.aboutUpdates;
  }
  @observable updateToolbarVisible = initialState.updateToolbarVisible;
  @observable updateToolbarLabel = initialState.updateToolbarLabel;
  onUpdateState(event: CustomEvent<UpdateStatus>): void {
    const status = event.detail;
    const track = status.channel === "Canary" ? "Canary" : "Stable";
    this.updateToolbarVisible = Boolean(
      (status.version && !status.suppressed) ||
      status.scheduled ||
      ["downloading", "ready", "installing"].includes(status.phase),
    );
    this.updateToolbarLabel = status.scheduled
      ? `${track} update on exit`
      : status.phase === "downloading"
        ? `Downloading ${track} update…`
        : status.phase === "ready"
          ? `${track} update ready`
          : status.phase === "installing"
            ? `Installing ${track} update…`
            : `${track} update available`;
  }
  openAbout(): void {
    this.settings.showSettingsSection("about");
    this.activateView("settings");
  }
  updateDraftDialog!: HTMLDialogElement;
  @observable updateDraftSummary = initialState.updateDraftSummary;
  private installPending = false;
  private resolveUpdateDrafts: ((proceed: boolean) => void) | undefined;
  async onInstallUpdate(event: CustomEvent<{ reopen: boolean }>): Promise<void> {
    if (this.installPending) {
      return;
    }
    this.installPending = true;
    try {
      const dirty = [
        this.composer?.composerDirty ? "Composer" : "",
        this.settings?.settingsDirty ? "Settings" : "",
        this.settings?.clientCertificates?.certificateSettingsDirty ? "Client certificates" : "",
        this.automation?.draftDirty ? "Saved responses" : "",
        this.automation?.headerRulesEditor?.hrDirty ? "Header changes" : "",
        this.automation?.responseTextEditor?.hrDirty ? "Response text replacements" : "",
        this.automation?.userAgentOverrides?.uaDirty ? "UA headers" : "",
        this.automation?.scriptEditor?.editorLoaded && this.automation.scriptEditor.scriptDirty
          ? "Scripts"
          : "",
      ].filter(Boolean);
      if (dirty.length) {
        this.updateDraftSummary = `There are unsaved changes in ${dirty.join(", ")}. Installing will close Transmog and discard those drafts. Keep working to save them first.`;
        const proceed = await new Promise<boolean>((resolve) => {
          this.resolveUpdateDrafts = resolve;
          this.updateDraftDialog.showModal();
        });
        if (!proceed) {
          return;
        }
      }
      this.settings.showSettingsSection("about");
      await this.activateView("settings");
      await this.updates.install(event.detail.reopen);
      await this.settings.refreshStatus();
    } finally {
      this.installPending = false;
    }
  }
  resolveUpdateDraftsChoice(proceed: boolean): void {
    this.updateDraftDialog.close();
    this.resolveUpdateDrafts?.(proceed);
    this.resolveUpdateDrafts = undefined;
  }
  cancelUpdateDrafts(event: Event): void {
    event.preventDefault();
    this.resolveUpdateDraftsChoice(false);
  }
  checkUpdates(): void {
    this.openAbout();
    void this.updates.check(true);
  }
  private noticeAction: NoticeAction = null;
  private navigationGeneration = 0;
  private workspaceTouched = false;
  @observable workspaceLoaded = false;
  private saveTimer: number | undefined;
  private saving = false;
  private saveAgain = false;
  private readonly inspectorShortcut = (event: KeyboardEvent): void => {
    // Reserve the layout command before list/button Enter handlers; editors get first refusal.
    if (this.activeView === "traffic" && !isTextEditing(event)) {
      this.traffic.inspectorKeyboard(event);
    }
  };
  private readonly shortcutListener = (event: KeyboardEvent): void => {
    if (
      event.defaultPrevented ||
      event.isComposing ||
      !(event.ctrlKey || event.metaKey) ||
      event.altKey
    ) {
      return;
    }
    // Let dialogs and editors own their input before handling application commands.
    if (event.composedPath().some((node) => node instanceof HTMLDialogElement && node.open)) {
      return;
    }
    if (this.activeView === "traffic" && this.traffic.inspectorKeyboard(event)) {
      return;
    }
    const key = event.key.toLowerCase();
    if (event.shiftKey && key !== "f") {
      return;
    }
    const traffic = this.activeView === "traffic";
    if (
      !["n", "o"].includes(key) &&
      !(key === "w" && this.viewerMode) &&
      !(key === "," && !this.viewerMode) &&
      !(traffic && ["f", "s"].includes(key))
    ) {
      return;
    }
    event.preventDefault();
    if (event.repeat) {
      return;
    }
    if (key === "n") {
      void this.traffic.openViewer();
    } else if (key === "w") {
      void this.closeViewer();
    } else if (key === "o") {
      void this.importTraffic();
    } else if (key === ",") {
      void this.openSettings();
    } else if (key === "s") {
      this.traffic.showSaveTrace();
    } else if (key === "f") {
      void this.traffic.focusSearch(event.shiftKey);
    }
  };
  private async importTraffic(): Promise<void> {
    const switching = this.activeView !== "traffic";
    if (await this.activateView("traffic")) {
      if (switching) {
        this.traffic.filesMenuButton.focus();
      }
      await this.traffic.openSavedTrace(false);
    }
  }
  private async openSettings(): Promise<void> {
    if (await this.activateView("settings")) {
      this.workspaceContent.focus();
    }
  }
  private async closeViewer(): Promise<void> {
    try {
      await invoke("close_trace_window");
    } catch (error: unknown) {
      this.diagnosticText = "Viewer could not be closed: " + describeError(error);
    }
  }
  workspaceChanged(): void {
    this.navigationExpanded = this.workspace.sidebarCollapsed ? "false" : "true";
  }
  connectedCallback(): void {
    super.connectedCallback();
    this.addEventListener("keydown", this.inspectorShortcut, true);
  }
  protected hydratedCallback(): void {
    this.capturedPreviewController = this.traffic;
    document.addEventListener("keydown", this.shortcutListener);
    if (this.viewerMode) {
      void invoke<ProductState>("product_state")
        .then((state) =>
          this.onPreferences(
            new CustomEvent("preferences-changed", {
              detail: { theme: state.preferences.theme, workspace: state.workspace },
            }),
          ),
        )
        .catch((error) => {
          this.diagnosticText = describeError(error);
        });
    } else {
      void this.refreshAutoresponses();
    }
  }
  async openMainWindow(): Promise<void> {
    try {
      await invoke("open_main_window");
    } catch (error: unknown) {
      this.diagnosticText = "Main window could not be opened: " + describeError(error);
    }
  }
  onAutomationState(event: CustomEvent<AutomationStatus>): void {
    if (!this.autoresponseState || event.detail.generation >= this.autoresponseState.generation) {
      this.autoresponseState = event.detail;
    }
    const status = event.detail;
    if (
      status.scriptGeneration !== undefined &&
      status.activeScripts !== undefined &&
      status.scriptGeneration >= this.scriptGeneration
    ) {
      this.scriptGeneration = status.scriptGeneration;
      this.enabledScriptCount = status.activeScripts;
    }
    this.updateAutomationBadge();
  }
  private async refreshAutoresponses(): Promise<void> {
    try {
      this.onAutomationState(
        new CustomEvent("automation-state-change", {
          detail: await invoke<AutomationStatus>("automation_status"),
        }),
      );
    } catch (error: unknown) {
      this.diagnosticText = "Autoresponse state could not be loaded: " + describeError(error);
    }
  }
  async toggleAutoresponses(): Promise<void> {
    if (this.autoresponsePending) {
      return;
    }
    this.autoresponsePending = true;
    try {
      const current =
        this.autoresponseState ?? (await invoke<AutomationStatus>("automation_status"));
      const status = await invoke<AutomationStatus>("set_autoresponses_enabled", {
        enabled: !(current.autoresponsesEnabled ?? true),
        generation: current.generation,
      });
      this.onAutomationState(new CustomEvent("automation-state-change", { detail: status }));
      if (status.generation === this.autoresponseState?.generation) {
        this.diagnosticText = status.autoresponsesEnabled
          ? "Saved responses resumed for new requests."
          : "Saved responses paused. Individual rule states are preserved.";
      }
    } catch (error: unknown) {
      this.diagnosticText = "Saved response switch failed: " + describeError(error);
      await this.refreshAutoresponses();
    } finally {
      this.autoresponsePending = false;
    }
  }

  activeViewChanged(): void {
    this.diagnosticText = "";
    this.currentNavigation = Object.fromEntries(
      Object.keys(initialState.currentNavigation).map((view) => [
        view,
        view === this.activeView ? "page" : "false",
      ]),
    );
  }

  onDiagnostic(event: CustomEvent<string>): void {
    const views: Record<string, string> = {
      "traffic-workspace": "traffic",
      "automation-workspace": "automation",
      "composer-workspace": "composer",
      "capture-workspace": "captures",
      "breakpoint-workspace": "breakpoints",
      "settings-workspace": "settings",
    };
    const source = event
      .composedPath()
      .map((node) => (node instanceof HTMLElement ? views[node.localName] : undefined))
      .find(Boolean);
    if (source === "traffic" && event.detail === this.traffic.sessionText) {
      return;
    }
    if (!source || source === this.activeView) {
      this.diagnosticText = event.detail;
    }
  }
  onNotice(event: CustomEvent<Notice>): void {
    this.noticeTitleText = event.detail.title;
    this.noticeMessageText = event.detail.message;
    this.noticeActionLabel = event.detail.actionLabel ?? "";
    this.noticeAction = event.detail.action;
    this.noticeVisible = true;
  }
  onStatus(event: CustomEvent<AppStatus>): void {
    this.applyStatus(event.detail);
  }
  onCaptureStatus(event: CustomEvent<AppStatus>): void {
    event.stopPropagation();
    this.settings.acceptAppStatus(event.detail);
  }
  private applyStatus(status: AppStatus): void {
    this.lifecycleLabel =
      status.lifecycle === "draining" ? status.summary : lifecycleLabel(status.lifecycle);
    this.lifecycleKind = status.lifecycle;
    this.listener = status.listener ?? "Not listening";
    if (status.hostRestorePending) {
      this.diagnosticText = "Host restoration is pending and must be retried before restart.";
    } else if (
      this.diagnosticText === "Host restoration is pending and must be retried before restart."
    ) {
      this.diagnosticText = "";
    }
  }
  onPreferences(
    event: CustomEvent<{
      theme: ProductState["preferences"]["theme"];
      workspace?: WorkspacePreferences;
    }>,
  ): void {
    this.theme = event.detail.theme;
    if (!this.workspaceLoaded && !this.workspaceTouched) {
      this.workspace = normalizeWorkspace(event.detail.workspace);
    } else if (!this.workspaceLoaded && event.detail.workspace) {
      this.workspace = {
        ...this.workspace,
        responseSetGuide: event.detail.workspace.responseSetGuide ?? "not-offered",
      };
    }
    this.workspaceLoaded = true;
  }
  onProxyOperation(event: CustomEvent<string>): void {
    this.proxyPending = event.detail;
  }
  async toggleProxy(): Promise<void> {
    await this.settings.ready;
    if (this.lifecycleKind === "running") {
      await this.settings.stopProxy();
    } else {
      await this.settings.startProxy();
    }
  }
  toggleNavigation(): void {
    this.changeWorkspace({ sidebarCollapsed: !this.workspace.sidebarCollapsed });
  }
  onWorkspaceChange(
    event: CustomEvent<{ patch: Partial<WorkspacePreferences>; committed?: boolean }>,
  ): void {
    this.changeWorkspace(event.detail.patch, event.detail.committed !== false);
  }
  private changeWorkspace(patch: Partial<WorkspacePreferences>, committed = true): void {
    this.workspaceTouched = true;
    this.workspace = { ...this.workspace, ...patch };
    if (!committed) {
      return;
    }
    window.clearTimeout(this.saveTimer);
    this.saveTimer = window.setTimeout(() => {
      void this.saveWorkspace();
    }, 250);
  }
  resetWorkspace(): void {
    this.changeWorkspace({
      ...defaultWorkspace(),
      responseSetGuide: this.workspace.responseSetGuide,
    });
  }
  private async saveWorkspace(): Promise<void> {
    this.saveAgain = true;
    if (this.saving) {
      return;
    }
    this.saving = true;
    try {
      while (this.saveAgain && this.isConnected) {
        this.saveAgain = false;
        await invoke<WorkspacePreferences>("save_workspace_preferences", {
          preferences: this.workspace,
        });
      }
    } catch (error: unknown) {
      this.diagnosticText = "Layout could not be saved: " + describeError(error);
    } finally {
      this.saving = false;
    }
  }
  async copyListener(): Promise<void> {
    if (this.listener === "Not listening") {
      return;
    }
    try {
      await navigator.clipboard.writeText(this.listener);
      this.diagnosticText = "Proxy address copied.";
    } catch {
      this.diagnosticText = "Select and copy the proxy address shown in the header.";
    }
  }
  onProxyReady(): void {
    this.proxyReady = true;
  }
  onSelection(event: CustomEvent<SelectedResponse | null>): void {
    this.selection = event.detail;
  }
  onMcpSelection(event: CustomEvent<{ provide(ids: string[]): void }>): void {
    event.detail.provide(this.traffic.selectedTrafficIds());
  }
  async openMcpSettings(): Promise<void> {
    await this.activateView("settings");
    this.settings.showSettingsSection("integrations");
  }
  onNavigate(event: CustomEvent<ViewName>): void {
    if (event.detail === "settings") {
      void this.openConnectionSettings();
    } else {
      void this.activateView(event.detail);
    }
  }
  async openTrafficFiles(): Promise<void> {
    if (await this.activateView("traffic")) {
      this.traffic.filesMenuButton.focus();
      this.traffic.filesMenuButton.click();
    }
  }
  async onTraceImport(event: CustomEvent<{ paths: string[] }>): Promise<void> {
    event.stopPropagation();
    if (await this.activateView("traffic")) {
      await this.traffic.importPaths(event.detail.paths);
    }
  }
  showView(event: Event): void {
    event.preventDefault();
    const view = (event.currentTarget as HTMLAnchorElement).dataset.view;
    if (
      view &&
      ["traffic", "breakpoints", "automation", "composer", "captures", "settings"].includes(view)
    ) {
      void this.activateView(view as ViewName);
    }
  }

  private async activateView(view: ViewName): Promise<boolean> {
    if (this.viewerMode && view !== "traffic" && view !== "composer") {
      return false;
    }
    const generation = ++this.navigationGeneration;
    try {
      if (view in loaders) {
        await loaders[view as keyof typeof loaders]();
      }
      if (generation !== this.navigationGeneration || !this.isConnected) {
        return false;
      }
      this.activeView = view;
      this.$flushUpdates();
      return true;
    } catch (error: unknown) {
      this.diagnosticText = "Workspace could not be loaded: " + describeError(error);
      return false;
    }
  }

  async startProxy(event?: Event): Promise<void> {
    event?.preventDefault();
    await this.settings.ready;
    await this.settings.startProxy();
  }
  async stopProxy(): Promise<void> {
    await this.settings.ready;
    await this.settings.stopProxy();
  }
  async refreshStatus(): Promise<void> {
    await this.settings.ready;
    const error = await this.settings.refreshStatus();
    if (error) {
      this.diagnosticText = error;
    } else if (
      this.diagnosticText !== "Host restoration is pending and must be retried before restart."
    ) {
      this.diagnosticText = "Proxy " + this.lifecycleKind;
    }
  }
  runNoticeAction(): void {
    const action = this.noticeAction;
    this.dismissNotice();
    if (action === "settings") {
      void this.openConnectionSettings();
    } else if (action === "setup-ca") {
      void this.settings.setupCa();
    } else if (action === "reset-ca") {
      void this.settings.resetCa();
    } else if (action === "start-proxy") {
      void this.startProxy();
    } else if (action === "recover-proxy") {
      void this.settings.recoverProxy();
    }
  }
  async openConnectionSettings(): Promise<void> {
    if (await this.activateView("settings")) {
      this.settings.showSettingsSection("connection");
    }
  }
  dismissNotice(): void {
    this.noticeAction = null;
    this.noticeVisible = false;
  }
  async onTrafficRule(event: CustomEvent<TrafficRuleRequest>): Promise<void> {
    event.stopPropagation();
    if (!this.viewerMode && (await this.activateView("automation"))) {
      await this.automation.beginRule(event.detail);
    }
  }
  async onResponseText(event: CustomEvent<SelectedResponse>): Promise<void> {
    if (!this.viewerMode && (await this.activateView("automation"))) {
      await this.automation.beginTextEdit(event.detail);
    }
  }
  async onAutoResponse(event: CustomEvent<SelectedResponse>): Promise<void> {
    if (await this.activateView("automation")) {
      await this.automation.populateCapturedAutoResponse(
        event.detail.sessionId,
        event.detail.detail,
      );
    }
  }
  async onAutoResponseBatch(event: CustomEvent<string[]>): Promise<void> {
    if (await this.activateView("automation")) {
      await this.automation.beginBatch(event.detail);
    }
  }
  responseSetIntake!: ResponseSetIntake;
  async onResponseSetIntake(event: CustomEvent<ResponseSetIntakeRequest>): Promise<void> {
    event.stopPropagation();
    if (this.viewerMode) {
      return;
    }
    await loaders.automation();
    await this.responseSetIntake.request(event.detail, {
      prepare: (id, current, movingRuleIds) =>
        this.automation.prepareResponseSetIntake(id, current, movingRuleIds),
      complete: (status) =>
        this.automation.onVariantStatus(new CustomEvent("set-status", { detail: status })),
    });
  }
  async onResponseSetOpen(event: CustomEvent<{ id: string }>): Promise<void> {
    event.stopPropagation();
    if (await this.activateView("automation")) {
      await this.automation.openResponseSet(new CustomEvent("set-open", { detail: event.detail }));
    }
  }
  async onSourceTraffic(event: CustomEvent<string>): Promise<void> {
    if (await this.activateView("traffic")) {
      if (this.traffic.previewReportDialog.open) {
        this.traffic.closePreviewReport();
      }
      if (!(await this.traffic.revealSession(event.detail))) {
        await this.automation?.refreshSourceAvailability?.();
      }
    }
  }
  onTrafficRefreshed(): void {
    if (this.activeView === "automation") {
      void this.automation.refreshSourceAvailability();
      this.automation.refreshUsageSoon();
    }
  }
  onPreviewOptions(): void {
    this.traffic.previewDraftDirty = Boolean(this.automation?.draftDirty);
  }
  previewPrerequisiteDialog!: HTMLDialogElement;
  @observable previewLookupFailed = initialState.previewLookupFailed;
  private previewRequest = 0;
  private previewReturnFocus: HTMLElement | null = null;
  private previewOrigin: ViewName = "automation";
  responseSetPagePicker!: ResponseSetPagePicker;
  onPreviewPageStatus(event: CustomEvent<AutomationStatus>): void {
    event.stopPropagation();
    this.automation.onVariantStatus(event);
  }
  async onPageAssociation(
    event: CustomEvent<{ id: string; isCurrent: () => boolean }>,
  ): Promise<void> {
    event.stopPropagation();
    const navigation = this.navigationGeneration,
      origin = this.activeView;
    await this.responseSetPagePicker.request(
      event.detail.id,
      this.selection?.sessionId ?? "",
      false,
      () =>
        this.isConnected &&
        navigation === this.navigationGeneration &&
        origin === this.activeView &&
        event.detail.isCurrent(),
    );
  }
  private async previewNamedSet(intent: ResponsePreviewIntent): Promise<void> {
    const request = ++this.previewRequest,
      navigation = this.navigationGeneration,
      origin = this.activeView;
    const current = () =>
      this.isConnected &&
      request === this.previewRequest &&
      navigation === this.navigationGeneration &&
      origin === this.activeView &&
      (intent.isCurrent?.() ?? true);
    const pageTarget = await this.responseSetPagePicker.request(
      intent.responseSetId!,
      this.selection?.sessionId ?? "",
      true,
      current,
    );
    if (!pageTarget || !current()) {
      return;
    }
    const prepared = await this.traffic.prepareRulePreview({ ...intent, pageTarget }, current);
    if (prepared && current()) {
      this.traffic.rememberPreviewFocus();
      prepared();
    }
  }
  showPreviewDiagnostics(): void {
    void this.traffic.showPreviewReport();
  }
  onSavedDemoPreview(event: CustomEvent<SavedDemoPreviewIntent>): void {
    event.stopPropagation();
    if (this.viewerMode || this.activeView !== "automation" || !event.detail.isCurrent()) {
      return;
    }
    this.traffic.showSavedDemoPreview(event.detail.demo);
  }
  async onPreviewSource(
    event: CustomEvent<{ id: string; isCurrent: () => boolean }>,
  ): Promise<void> {
    event.stopPropagation();
    const origin = this.activeView;
    if (!event.detail.isCurrent() || !(await this.activateView("traffic"))) {
      return;
    }
    const focus = this.traffic.trackNavigationFocus();
    const navigation = this.navigationGeneration,
      current = () =>
        event.detail.isCurrent() &&
        navigation === this.navigationGeneration &&
        this.activeView === "traffic" &&
        focus.unchanged();
    try {
      const found = await this.traffic.revealSession(
        event.detail.id,
        () => current() && focus.unchanged(),
        current,
      );
      if (!current()) {
        return;
      }
      if (found) {
        const restore = focus.unchanged();
        this.traffic.closePreviewReport();
        if (restore) {
          this.traffic.focusTraffic();
        }
      } else {
        this.traffic.previewReportError =
          "The source entry is no longer retained. Its resource decision remains in this report.";
        await this.activateView(origin);
      }
    } finally {
      focus.dispose();
    }
  }
  async onPagePreview(event: CustomEvent<ResponsePreviewIntent>): Promise<void> {
    event.stopPropagation();
    if (event.detail.responseSetId) {
      await this.previewNamedSet(event.detail);
      return;
    }
    const intent = event.detail,
      request = ++this.previewRequest,
      navigation = this.navigationGeneration,
      origin = this.activeView;
    const root = this.shadowRoot ?? this.getRootNode(),
      active = root instanceof ShadowRoot ? root.activeElement : document.activeElement;
    const focus = this.traffic.trackNavigationFocus();
    let canceled = false;
    const cancel = (e: Event) => {
      if (e instanceof KeyboardEvent && e.key !== "Escape") {
        return;
      }
      canceled = true;
    };
    root.addEventListener("keydown", cancel, true);
    root.addEventListener("input", cancel, true);
    root.addEventListener("change", cancel, true);
    const current = () =>
      this.isConnected &&
      request === this.previewRequest &&
      navigation === this.navigationGeneration &&
      origin === this.activeView &&
      !canceled &&
      focus.unchanged() &&
      (intent.isCurrent?.() ?? true);
    try {
      let prepared: (() => void) | null | undefined,
        failed = false;
      try {
        prepared = await this.traffic.prepareRulePreview(intent, current);
      } catch {
        failed = true;
      }
      if (!current()) {
        return;
      }
      if (prepared === undefined && !failed) {
        return;
      }
      if (!prepared) {
        this.previewReturnFocus = active instanceof HTMLElement ? active : null;
        this.previewOrigin = origin;
        this.previewLookupFailed = failed;
        this.$flushUpdates();
        this.previewPrerequisiteDialog.showModal();
        return;
      }
      // Preparation is read-only. Navigate only when a warning can be presented.
      if ((await this.activateView("traffic")) && request === this.previewRequest) {
        this.traffic.rememberPreviewFocus();
        prepared();
      }
    } finally {
      focus.dispose();
      root.removeEventListener("keydown", cancel, true);
      root.removeEventListener("input", cancel, true);
      root.removeEventListener("change", cancel, true);
    }
  }
  closePreviewPrerequisite(): void {
    this.previewPrerequisiteDialog.close();
  }
  previewPrerequisiteClosed(): void {
    if (this.previewPrerequisiteDialog.open) {
      return;
    }
    const target = this.previewReturnFocus;
    this.previewReturnFocus = null;
    if (
      this.activeView === this.previewOrigin &&
      target?.isConnected &&
      target.getClientRects().length
    ) {
      target.focus({ preventScroll: true });
    }
  }
  async previewGoToTraffic(): Promise<void> {
    this.previewReturnFocus = null;
    this.previewPrerequisiteDialog.close();
    if (await this.activateView("traffic")) {
      this.traffic.focusTraffic();
    }
  }
  async onMatchedRule(
    event: CustomEvent<string | { id: string; isCurrent: () => boolean }>,
  ): Promise<void> {
    if (typeof event.detail === "string") {
      if (await this.activateView("automation")) {
        await this.automation.refreshAutomation();
        this.automation.showMatchedRule(event.detail);
      }
      return;
    }
    const request = event.detail;
    if (!request.isCurrent() || !(await this.activateView("automation"))) {
      return;
    }
    const navigation = this.navigationGeneration;
    let focus = this.traffic.trackNavigationFocus();
    const current = () =>
      request.isCurrent() &&
      navigation === this.navigationGeneration &&
      this.activeView === "automation" &&
      focus.unchanged();
    const confirmLeave = async () => {
      if (!current()) {
        return false;
      }
      // The destination is known before asking to leave a draft. Its owned
      // confirmation may move focus; resume watching for later work afterward.
      focus.dispose();
      const allowed = await this.automation.canLeaveEditor();
      focus = this.traffic.trackNavigationFocus();
      return allowed && current();
    };
    try {
      await this.automation.refreshAutomation();
      if (current()) {
        await this.automation.showMatchedRule(request.id, current, confirmLeave);
      }
    } finally {
      focus.dispose();
    }
  }
  async onReplay(event: CustomEvent<SessionDetail>): Promise<void> {
    if (await this.activateView("composer")) {
      await this.composer.populateRequest(event.detail);
    }
  }
  disconnectedCallback(): void {
    this.removeEventListener("keydown", this.inspectorShortcut, true);
    document.removeEventListener("keydown", this.shortcutListener);
    window.clearTimeout(this.saveTimer);
    this.resolveUpdateDrafts?.(false);
    for (const error of this.operationErrors) {
      error.resolve();
    }
    this.operationErrors = [];
    super.disconnectedCallback();
  }
}

AppShell.define("app-shell");
