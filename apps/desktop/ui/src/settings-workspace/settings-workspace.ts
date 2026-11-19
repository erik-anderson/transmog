import { capturePresentationIntent, captureRenderScope, renderPresentation } from "../rendering.js";
import initialState from "../initial-state.json";
import "../network-profile-settings/network-profile-settings.js";
import "../process-inspection-settings/process-inspection-settings.js";
import type { ProcessInspectionSettings } from "../process-inspection-settings/process-inspection-settings.js";
import "../extension-settings/extension-settings.js";
import "../mcp-settings/mcp-settings.js";
import type { ExtensionSettings } from "../extension-settings/extension-settings.js";
import "../client-certificate-settings/client-certificate-settings.js";
import "../loopback-exemptions/loopback-exemptions.js";
import type { LoopbackExemptions } from "../loopback-exemptions/loopback-exemptions.js";
import type { AppUpdates } from "../app-updates/app-updates.js";
import type { ClientCertificateSettings } from "../client-certificate-settings/client-certificate-settings.js";
import { attr, observable } from "@microsoft/webui-framework";
import { invoke } from "@tauri-apps/api/core";
import { WorkspaceElement } from "../workspace-element.js";
import type {
  AppStatus,
  CaIdentity,
  DesktopBootstrap,
  Notice,
  ProductState,
  WorkspacePreferences,
} from "../models.js";
import { describeError, listensOnAllInterfaces, proxyClientAddress } from "../utilities.js";

type SettingsFormSection = "preferences" | "connection";
type SettingsSection = keyof typeof initialState.currentSettings | "client-certificates";
type ConnectionField = "listener" | "https" | "processes" | "isolation";

export class SettingsWorkspace extends WorkspaceElement {
  extensionsEditor!: ExtensionSettings;
  clientCertificates!: ClientCertificateSettings;
  aboutUpdates!: AppUpdates;
  loopbackManager!: LoopbackExemptions;
  processInspectionEditor!: ProcessInspectionSettings;
  openLoopback(event: Event): void {
    void this.loopbackManager.open(event);
  }
  async openWebsite(event: Event): Promise<void> {
    event.preventDefault();
    try {
      await invoke("open_product_website");
    } catch (error: unknown) {
      await this.showError("Website could not be opened", describeError(error));
    }
  }
  @observable credits: Array<{
    id: string;
    name: string;
    version: string;
    license: string;
    text: string;
  }> = [];
  @observable creditsSummary = "";
  @observable creditsStatus = "";
  creditsDialog!: HTMLDialogElement;
  async showCredits(): Promise<void> {
    this.creditsDialog.showModal();
    if (this.credits.length) {
      return;
    }
    this.creditsStatus = "Loading third-party notices…";
    try {
      const response = await fetch("/credits.json");
      if (!response.ok) {
        throw new Error("The bundled notices are unavailable.");
      }
      const notices = (await response.json()) as {
        summary: string;
        entries: Array<{
          id: string;
          name: string;
          version: string;
          license: string;
          text: string;
        }>;
      };
      this.credits = notices.entries;
      this.creditsSummary = notices.summary;
      this.creditsStatus = "";
    } catch (error: unknown) {
      this.creditsStatus = describeError(error);
    }
  }
  closeCredits(): void {
    this.creditsDialog.close();
  }
  resetWorkspace(): void {
    this.$emit("reset-workspace");
    this.settingsText = "Layout and columns reset.";
  }
  checkUpdates(): void {
    this.$emit("check-updates");
  }
  @attr view = "traffic";
  @observable customPortEnabled = initialState.customPortEnabled;
  updateProxyPortMode(): void {
    this.customPortEnabled = (
      this.connectionForm.elements.namedItem("customProxyPort") as HTMLInputElement
    ).checked;
  }
  showInvalidPort(): void {
    this.listenerOptions.open = true;
  }
  @observable requestBodyMode = initialState.requestBodyMode;
  updateRequestBodyLimit(): void {
    this.requestBodyMode = (
      this.settingsForm.elements.namedItem("requestBodyLimit") as HTMLSelectElement
    ).value;
  }
  @observable bufferMode = initialState.bufferMode;
  @observable entryLimitEnabled = false;
  updateEntryLimit(): void {
    this.entryLimitEnabled = (
      this.settingsForm.elements.namedItem("limitEntries") as HTMLInputElement
    ).checked;
  }
  @observable bufferSummary = "Automatic uses half of installed RAM and keeps bodies in memory.";
  updateBufferMode(): void {
    this.bufferMode = (
      this.settingsForm.elements.namedItem("bufferMode") as HTMLSelectElement
    ).value;
  }
  private async refreshBufferStatus(): Promise<void> {
    try {
      const status = await invoke<{
        installedRam: number | null;
        maxBytes: number | null;
        storage: string;
        retainedBytes: number;
      } | null>("buffer_status");
      if (status) {
        const format = (bytes: number) =>
          (bytes / 1073741824).toLocaleString(undefined, { maximumFractionDigits: 2 }) + " GiB";
        this.bufferSummary =
          "Current buffer: " +
          (status.maxBytes === null ? "No maximum" : format(status.maxBytes) + " maximum") +
          " · " +
          (status.storage === "memory" ? "Memory storage" : "Disk storage") +
          (status.installedRam === null
            ? " · Installed RAM unavailable; automatic uses 1 GiB."
            : " · " + format(status.installedRam) + " installed RAM");
      }
    } catch {
      this.bufferSummary =
        "Buffer status unavailable. Automatic uses half installed RAM; larger or unlimited buffers write to disk.";
    }
  }
  @observable settingsSection: SettingsSection = "preferences";
  @observable currentSettings = initialState.currentSettings;
  @observable settingsDirty = false;
  @observable settingsBusy = false;
  @observable settingsLoaded = initialState.settingsLoaded;
  @observable settingsLoadError = initialState.settingsLoadError;
  @observable settingsText = "";
  @observable settingsError = "";
  @observable connectionDirty = false;
  @observable connectionText = "";
  @observable connectionError = "";
  private editedConnectionFields = new Set<ConnectionField>();
  @observable certificateReady = false;
  @observable certificateRecovery = false;
  @observable interceptionCertificateUnavailable = true;
  @observable certificateDetailsOpen = initialState.certificateDetailsOpen;
  @observable httpsEnableError = initialState.httpsEnableError;
  decryptInput!: HTMLInputElement;
  interceptionSummary!: HTMLElement;
  clientCertificateSettingsButton!: HTMLButtonElement;
  clientCertificateBackButton!: HTMLButtonElement;
  @observable hostRecoveryPending = false;
  @observable supportBusy = false;
  @observable supportPathsAllowed = false;
  @observable supportDetails = "";
  @observable supportFacts: Array<{ label: string; value: string }> = [];
  showSettingsSection(section: SettingsSection): void {
    this.settingsSection = section;
    const current = section === "client-certificates" ? "connection" : section;
    this.currentSettings = {
      preferences: current === "preferences" ? "page" : "false",
      connection: current === "connection" ? "page" : "false",
      integrations: current === "integrations" ? "page" : "false",
      extensions: current === "extensions" ? "page" : "false",
      support: current === "support" ? "page" : "false",
      about: current === "about" ? "page" : "false",
    };
  }
  async openInterceptionCertificate(): Promise<void> {
    this.certificateDetailsOpen = true;
    await this.focusSettingsControl("connection", this.interceptionSummary, "center");
  }
  certificateDisclosureChanged(event: Event): void {
    this.certificateDetailsOpen = (event.target as HTMLDetailsElement).open;
  }
  async openHttpsSettings(): Promise<void> {
    await this.focusSettingsControl("connection", this.decryptInput);
  }
  async openClientCertificateSettings(): Promise<void> {
    await this.focusSettingsControl("client-certificates", this.clientCertificateBackButton);
  }
  async backToConnection(): Promise<void> {
    await this.focusSettingsControl("connection", this.clientCertificateSettingsButton);
  }
  private async focusSettingsControl(
    section: SettingsSection,
    control: HTMLElement,
    block: ScrollLogicalPosition = "nearest",
  ): Promise<void> {
    this.showSettingsSection(section);
    const current = await renderPresentation(this, () => [this.view, this.settingsSection]);
    if (current() && control.getClientRects().length && !control.matches(":disabled")) {
      control.focus({ preventScroll: true });
      control.scrollIntoView({ block });
    }
  }
  markSettingsDirty(): void {
    this.settingsDirty = true;
    this.settingsText = "Unsaved changes";
  }
  markConnectionDirty(field: ConnectionField): void {
    if (field === "https" && this.decryptInput.checked === this.savedDecryptHttps) {
      this.editedConnectionFields.delete(field);
      this.clearHttpsEnableError();
    } else {
      this.editedConnectionFields.add(field);
    }
    this.updateConnectionDraftStatus();
  }
  private updateConnectionDraftStatus(): void {
    this.connectionDirty = this.editedConnectionFields.size > 0;
    this.connectionText = this.connectionDirty ? "Unsaved changes" : "";
    if (!this.connectionDirty) {
      this.connectionError = "";
    }
  }
  private clearHttpsEnableError(): void {
    if (this.httpsEnableError) {
      this.connectionError = "";
      this.httpsEnableError = false;
    }
  }
  settingsKeyboard(event: KeyboardEvent, section: SettingsFormSection): void {
    if (
      event.defaultPrevented ||
      !(event.ctrlKey || event.metaKey) ||
      event.key.toLowerCase() !== "s" ||
      event
        .composedPath()
        .some(
          (target) =>
            target instanceof HTMLElement && target.localName === "network-profile-settings",
        )
    ) {
      return;
    }
    event.preventDefault();
    if (this.settingsBusy || this.proxyPending || !this.proxyReady || !this.settingsLoaded) {
      return;
    }
    if (section === "connection" && this.connectionDirty) {
      this.connectionForm.requestSubmit();
    } else if (section === "preferences" && this.settingsDirty) {
      this.settingsForm.requestSubmit();
    }
  }
  private certificateBootstrap: DesktopBootstrap | undefined;
  private certificateRevision = 0;
  private certificateSetupNeeded = true;
  private savedDecryptHttps = initialState.decryptHttps;
  @observable routingSavePending = initialState.routingSavePending;
  private routingRevision = 0;
  @observable configureSystemProxy = initialState.configureSystemProxy;
  private activeRoutingMode: AppStatus["routingMode"] = initialState.activeRoutingMode;
  private activeHttpsDecryption: AppStatus["httpsDecryption"] = null;
  @observable connectionUsesSystemProxy = initialState.connectionUsesSystemProxy;
  @observable publicCertificateAvailable = initialState.publicCertificateAvailable;
  @observable windowsCertificateTrusted = initialState.windowsCertificateTrusted;
  @observable interceptionCertificateStatus = initialState.interceptionCertificateStatus;
  @observable listenerOnAllInterfaces = initialState.listenerOnAllInterfaces;
  async saveProxyRouting(configureSystemProxy: boolean): Promise<void> {
    this.routingSavePending = true;
    try {
      await invoke<void>("save_proxy_routing", { configureSystemProxy });
      this.routingRevision++;
      this.applyProxyRouting(configureSystemProxy);
    } finally {
      this.routingSavePending = false;
    }
  }
  private applyProxyRouting(configureSystemProxy: boolean): void {
    this.configureSystemProxy = configureSystemProxy;
    this.$emit("routing-changed", configureSystemProxy);
    this.presentCertificateState();
  }
  private updateCertificateState(bootstrap: DesktopBootstrap, updateRecovery = true): void {
    this.certificateRevision++;
    this.certificateBootstrap = bootstrap;
    if (updateRecovery) {
      this.hostRecoveryPending = bootstrap.hostRestorePending;
    }
    this.presentCertificateState();
  }
  private presentCertificateState(showStartBlock = false): void {
    const system = this.activeRoutingMode
      ? this.activeRoutingMode === "system"
      : this.configureSystemProxy;
    this.connectionUsesSystemProxy = system;
    const bootstrap = this.certificateBootstrap;
    if (!bootstrap) {
      return;
    }
    const decrypting = this.activeHttpsDecryption ?? this.decryptHttps;
    const recovery = this.caRecoveryRequired(bootstrap);
    const available = bootstrap.caFilesPresent && bootstrap.ownedCaSha256 !== null && !recovery;
    const trusted = available && bootstrap.ownedCaTrusted;
    this.publicCertificateAvailable = available;
    this.windowsCertificateTrusted = trusted;
    this.certificateSetupNeeded = !available || (system && !trusted);
    this.interceptionCertificateUnavailable = decrypting && this.certificateSetupNeeded;
    this.certificateRecovery = recovery;
    this.certificateReady = !this.interceptionCertificateUnavailable;
    if (this.httpsEnableError && !this.certificateSetupNeeded && !bootstrap.caStatusError) {
      this.clearHttpsEnableError();
      this.connectionText = "Certificate ready. Save settings to enable HTTPS decryption.";
    }
    this.interceptionCertificateStatus = recovery
      ? this.caRecoveryNotice(bootstrap).message
      : !available
        ? "Create a local interception certificate to decrypt HTTPS."
        : trusted
          ? "Apps with their own trust store may also need the public certificate."
          : "To decrypt HTTPS, the app must trust this public certificate. Use Windows trust or the app’s own trust store.";
    let notice: Notice | null = null;
    if (
      (!this.activeRoutingMode && this.interceptionCertificateUnavailable) ||
      (showStartBlock && recovery)
    ) {
      notice = recovery
        ? this.caRecoveryNotice(bootstrap)
        : {
            title: "HTTPS interception needs setup",
            message: system
              ? "Create or trust Transmog’s interception certificate for your Windows account to decrypt HTTPS."
              : "Create an interception certificate, then trust it through Windows or in the app you want to inspect.",
            actionLabel: system ? "Set up HTTPS interception" : "Create interception certificate",
            action: "setup-ca",
          };
    }
    this.$emit("certificate-state", {
      unavailable: this.interceptionCertificateUnavailable,
      notice,
      showStartBlock,
    });
    if (showStartBlock || this.proxyOutputFromCertificateBlock) {
      this.setProxyOutput(notice?.message ?? "", notice ? "error" : "success");
      this.proxyOutputFromCertificateBlock = notice !== null;
    }
  }
  private async runSupport(operation: () => Promise<void>): Promise<void> {
    if (this.supportBusy) {
      return;
    }
    this.supportBusy = true;
    try {
      await operation();
    } finally {
      this.supportBusy = false;
    }
  }
  @observable decryptHttps = initialState.decryptHttps;
  @observable proxyReady = initialState.proxyReady;
  @observable proxyText = initialState.proxyText;
  @observable proxyKind = initialState.proxyKind;
  private proxyOutputFromCertificateBlock = false;
  @observable supportText = initialState.supportText;
  @observable diagnosticsPathText = initialState.diagnosticsPathText;
  @observable certificatePath = initialState.certificatePath;
  @observable privateKeyPath = initialState.privateKeyPath;
  @observable caSha256 = initialState.caSha256;
  @observable proxyLifecycle = "stopped";
  @observable proxyPending = "";
  proxyForm!: HTMLFormElement;
  connectionForm!: HTMLFormElement;
  listenerOptions!: HTMLDetailsElement;
  settingsForm!: HTMLFormElement;
  supportForm!: HTMLFormElement;
  ready!: Promise<void>;
  private drainTimer: number | undefined;
  private statusRevision = 0;

  protected hydratedCallback(): void {
    this.ready = this.initializeShell();
  }
  acceptAppStatus(status: AppStatus): void {
    this.statusRevision++;
    this.hostRecoveryPending = status.hostRestorePending;
    this.proxyLifecycle = status.lifecycle;
    this.listenerOnAllInterfaces = listensOnAllInterfaces(status.listener);
    this.activeRoutingMode = status.routingMode ?? null;
    this.activeHttpsDecryption = this.activeRoutingMode ? (status.httpsDecryption ?? null) : null;
    this.$emit("status-changed", status);
    this.presentCertificateState();
    window.clearTimeout(this.drainTimer);
    if (status.lifecycle === "draining" || status.lifecycle === "stopping") {
      this.drainTimer = window.setTimeout(() => {
        if (this.isConnected) {
          void this.refreshStatus();
        }
      }, 750);
    }
  }
  disconnectedCallback(): void {
    window.clearTimeout(this.drainTimer);
    super.disconnectedCallback();
  }
  private applyTheme(
    theme: ProductState["preferences"]["theme"],
    workspace?: WorkspacePreferences,
  ): void {
    this.$emit("preferences-changed", { theme, workspace });
  }
  dismissNotice(): void {
    this.$emit("dismiss-notice");
  }
  private async initializeShell(): Promise<void> {
    const routingRevision = this.routingRevision;
    this.setProxyOutput("Loading durable proxy state…", "progress");
    try {
      const [bootstrap, productState, status] = await Promise.allSettled([
        invoke<DesktopBootstrap>("desktop_bootstrap"),
        invoke<ProductState>("product_state"),
        invoke<AppStatus>("app_status"),
      ]);
      if (bootstrap.status === "fulfilled") {
        this.certificatePath = bootstrap.value.caCertificatePath;
        this.privateKeyPath = bootstrap.value.caPrivateKeyPath;
        this.caSha256 = bootstrap.value.ownedCaSha256 ?? "";
        this.diagnosticsPathText = bootstrap.value.diagnosticsPath;
      }
      if (productState.status === "fulfilled") {
        this.populateProductSettings(productState.value, routingRevision);
      } else {
        this.settingsLoadError =
          "Settings could not be loaded: " + describeError(productState.reason);
      }
      if (status.status === "fulfilled") {
        this.acceptAppStatus(status.value);
        if (status.value.hostRestorePending) {
          this.showNotice(
            "Windows proxy recovery required",
            "Restore the Windows proxy settings saved by a previous run. Manual proxy is available while recovery is pending.",
            "Restore settings",
            "recover-proxy",
          );
        }
      }
      this.setProxyOutput("", "success");
      if (bootstrap.status === "fulfilled") {
        this.updateCertificateState(bootstrap.value);
      }
      const failure = [bootstrap, productState, status].find(
        (result) => result.status === "rejected",
      );
      if (failure) {
        throw failure.reason;
      }
    } catch (error: unknown) {
      const message = `Desktop initialization failed: ${describeError(error)}`;
      this.setProxyOutput(message, "error");
      this.diagnostic = message;
      await this.reportFrontendIssue("desktop-initialization-failed", error);
    } finally {
      this.proxyReady = true;
      this.$emit("proxy-ready");
    }
  }

  async toggleProxy(): Promise<void> {
    if (this.proxyLifecycle === "running") {
      await this.stopProxy();
    } else {
      await this.startProxy();
    }
  }
  async startProxy(event?: Event): Promise<void> {
    event?.preventDefault();
    const intent = capturePresentationIntent(this);
    const scope = captureRenderScope(this, () => [this.view, this.settingsSection]);
    try {
      await this.runProxyChange("starting", () =>
        this.startProxyNow(() => scope() && intent.current()),
      );
    } finally {
      intent.dispose();
    }
  }
  async startHttpsTunnels(): Promise<void> {
    await this.runProxyChange("starting", async () => {
      try {
        const data = new FormData(this.proxyForm);
        const status = await invoke<AppStatus>("start_https_tunnels", {
          request: {
            caCertificatePath: String(data.get("certificate") ?? ""),
            caPrivateKeyPath: String(data.get("privateKey") ?? ""),
            listen: "127.0.0.1:0",
            route: "auto",
            allowRemoteClients: false,
          },
        });
        this.updateSavedHttpsMode(false, true);
        this.acceptAppStatus(status);
        if (!status.hostRestorePending) {
          this.dismissNotice();
        }
        this.setProxyOutput(
          status.routingMode === "manual"
            ? `Proxy listening at ${proxyClientAddress(status.listener ?? "")}. Configure your application’s HTTP and HTTPS proxy to use this address. HTTPS uses tunnels.`
            : "Proxy started with HTTPS tunnels. HTTP is inspected; HTTPS contents are not decrypted. This choice is saved.",
          "success",
        );
      } catch (error: unknown) {
        this.setProxyOutput("Start failed: " + describeError(error), "error");
        // Binding can fail after the tunnel choice is saved. Preserve the draft
        // until the saved mode confirms that the explicit choice committed.
        try {
          const saved = await invoke<ProductState>("product_state");
          const decrypt = saved.preferences.decryptHttps;
          this.updateSavedHttpsMode(decrypt, !decrypt && this.savedDecryptHttps);
        } catch (error: unknown) {
          this.diagnostic = "Saved HTTPS choice could not be refreshed: " + describeError(error);
        }
      } finally {
        try {
          this.updateCertificateState(await invoke<DesktopBootstrap>("desktop_bootstrap"));
        } catch (error: unknown) {
          this.diagnostic = "Certificate status could not be refreshed: " + describeError(error);
        }
      }
    });
  }
  private updateSavedHttpsMode(decrypt: boolean, replaceDraft = false): void {
    const hasDraft = this.decryptInput.checked !== this.savedDecryptHttps;
    this.decryptHttps = decrypt;
    this.savedDecryptHttps = decrypt;
    if (replaceDraft || !hasDraft) {
      this.decryptInput.checked = decrypt;
    }
    if (this.decryptInput.checked === decrypt) {
      this.clearHttpsEnableError();
      if (this.editedConnectionFields.delete("https")) {
        this.updateConnectionDraftStatus();
      }
    }
  }
  async stopProxy(): Promise<void> {
    await this.runProxyChange("stopping", () => this.stopProxyNow());
  }
  private async runProxyChange(kind: string, operation: () => Promise<void>): Promise<void> {
    if (this.proxyPending || this.routingSavePending || this.settingsBusy) {
      return;
    }
    this.statusRevision++;
    window.clearTimeout(this.drainTimer);
    this.proxyPending = kind;
    this.$emit("proxy-operation", kind);
    try {
      await operation();
      await this.refreshStatus();
    } finally {
      this.proxyPending = "";
      this.$emit("proxy-operation", "");
    }
  }

  private async startProxyNow(canOpenSettings: () => boolean): Promise<void> {
    const data = new FormData(this.proxyForm);
    try {
      if (this.proxyLifecycle === "draining" || this.proxyLifecycle === "stopping") {
        const resumed = await invoke<AppStatus>("resume_application");
        this.acceptAppStatus(resumed);
        if (resumed.lifecycle === "running") {
          this.setProxyOutput(
            "Proxy resumed. Existing requests and connections remain active.",
            "success",
          );
          return;
        }
      }
      const [bootstrap, productState] = await Promise.all([
        invoke<DesktopBootstrap>("desktop_bootstrap"),
        invoke<ProductState>("product_state"),
      ]);
      this.updateSavedHttpsMode(productState.preferences.decryptHttps);
      this.updateCertificateState(bootstrap);
      const system = productState.preferences.configureSystemProxy;
      if (
        productState.preferences.decryptHttps &&
        (this.caRecoveryRequired(bootstrap) ||
          !bootstrap.caFilesPresent ||
          bootstrap.ownedCaSha256 === null ||
          (system && !bootstrap.ownedCaTrusted))
      ) {
        this.presentCertificateState(true);
        if (canOpenSettings()) {
          this.showSettingsSection("connection");
          this.activateView("settings");
        }
        return;
      }
      this.setProxyOutput(
        system ? "Applying current-user Windows proxy settings…" : "Binding the proxy listener…",
        "progress",
      );
      const status = await invoke<AppStatus>("start_proxy", {
        request: {
          caCertificatePath: String(data.get("certificate") ?? ""),
          caPrivateKeyPath: String(data.get("privateKey") ?? ""),
          listen: "127.0.0.1:0",
          route: "auto",
          allowRemoteClients: false,
        },
      });
      this.acceptAppStatus(status);
      const listener = status.listener ? proxyClientAddress(status.listener) : "an unknown address";
      if (!status.hostRestorePending) {
        this.dismissNotice();
      }
      this.setProxyOutput(
        status.routingMode === "manual" || !system
          ? `Proxy listening at ${listener}. Configure your application’s HTTP and HTTPS proxy to use this address.`
          : `Proxy listening at ${listener}. Windows proxy settings are active and traffic is being captured automatically.`,
        "success",
      );
    } catch (error: unknown) {
      const message = `Start failed: ${describeError(error)}`;
      this.setProxyOutput(message, "error");
      this.diagnostic = message;
      try {
        const bootstrap = await invoke<DesktopBootstrap>("desktop_bootstrap");
        this.updateCertificateState(bootstrap);
        if (bootstrap.hostRestorePending && this.configureSystemProxy) {
          this.showNotice(
            "Windows proxy recovery required",
            message,
            "Restore settings",
            "recover-proxy",
          );
          return;
        }
        if (this.caRecoveryRequired(bootstrap)) {
          this.presentCertificateState(true);
          return;
        }
      } catch {
        /* Preserve the original start failure if inspection also fails. */
      }
      this.showNotice(
        "Proxy could not start",
        `${message} See Settings for recovery options.`,
        "Open Settings",
        "settings",
      );
    }
  }

  async setupCa(): Promise<void> {
    await this.runProxyChange("certificate-setup", () => this.setupCaNow());
  }

  private caRecoveryRequired(bootstrap: DesktopBootstrap): boolean {
    return (
      Boolean(bootstrap.caKeyError) ||
      (bootstrap.caFilesExist && (!bootstrap.caFilesPresent || bootstrap.ownedCaSha256 === null)) ||
      (!bootstrap.caFilesPresent && bootstrap.ownedCaSha256 !== null)
    );
  }

  private caRecoveryNotice(bootstrap: DesktopBootstrap): Notice {
    const reason =
      bootstrap.caKeyError ??
      (bootstrap.ownedCaSha256 === null
        ? "Certificate files from a previous setup remain, but their Transmog identity is not recorded."
        : "The recorded interception certificate is missing one or both of its files.");
    const next = this.connectionUsesSystemProxy
      ? "Reset the certificate to remove the app-managed files, then create and trust a new certificate."
      : "Reset the certificate, then trust the replacement through Windows or in the app you want to inspect.";
    return {
      title: "Interception certificate needs a reset",
      message: `${reason} ${next}`,
      actionLabel: "Reset certificate and set up again",
      action: "reset-ca",
    };
  }

  async resetCa(): Promise<void> {
    await this.runProxyChange("certificate-setup", async () => {
      try {
        const bootstrap = await invoke<DesktopBootstrap>("desktop_bootstrap");
        const trust = this.configureSystemProxy
          ? "Transmog will create a new certificate and ask Windows to trust it. You must approve the Windows certificate dialog."
          : "Transmog will create a new certificate. Trust the public certificate in the application you are debugging.";
        const proceed = window.confirm(
          `Reset HTTPS interception? This permanently deletes Transmog’s app-managed certificate and private key:\n${bootstrap.caCertificatePath}\n${bootstrap.caPrivateKeyPath}\n\nAny exact certificate recorded by Transmog will also be removed from your current-user trusted roots. ${trust} Continue?`,
        );
        if (!proceed) {
          return;
        }
        this.setProxyOutput(
          "Removing the previous interception certificate and private key…",
          "progress",
        );
        await invoke<void>("reset_ca");
        this.certificatePath = bootstrap.caCertificatePath;
        this.privateKeyPath = bootstrap.caPrivateKeyPath;
        this.caSha256 = "";
        const elements = this.proxyForm.elements;
        (elements.namedItem("certificate") as HTMLInputElement).value = this.certificatePath;
        (elements.namedItem("privateKey") as HTMLInputElement).value = this.privateKeyPath;
        (elements.namedItem("thumbprint") as HTMLInputElement).value = "";
        await this.setupCaNow(true);
      } catch (error: unknown) {
        const message = `Certificate reset failed: ${describeError(error)}`;
        this.setProxyOutput(message, "error");
        this.showNotice(
          "Certificate reset needs attention",
          message,
          "Reset certificate and set up again",
          "reset-ca",
        );
      }
    });
  }

  private async setupCaNow(confirmed = false): Promise<void> {
    const systemRouting = this.connectionUsesSystemProxy;
    this.setProxyOutput("Preparing the interception certificate…", "progress");
    try {
      let bootstrap = await invoke<DesktopBootstrap>("desktop_bootstrap", { inspectCaKey: true });
      this.updateCertificateState(bootstrap);
      if (this.caRecoveryRequired(bootstrap)) {
        this.presentCertificateState(true);
        return;
      }
      if (
        systemRouting &&
        !confirmed &&
        !window.confirm(
          "Transmog will create a durable local interception certificate if needed, then ask Windows to trust its public certificate for your account. You must manually approve the Windows certificate dialog. Continue?",
        )
      ) {
        this.setProxyOutput("Certificate setup canceled.", "progress");
        return;
      }
      const data = new FormData(this.proxyForm);
      let sha256 = bootstrap.ownedCaSha256;
      if (!bootstrap.caFilesPresent) {
        const identity = await invoke<CaIdentity>("create_ca", {
          request: {
            certificatePath: String(data.get("certificate") ?? ""),
            privateKeyPath: String(data.get("privateKey") ?? ""),
            commonName: "Transmog local interception CA",
            validityDays: 3650,
          },
        });
        sha256 = identity.sha256;
        this.caSha256 = identity.sha256;
      }
      if (sha256 === null) {
        throw new Error(
          "The interception certificate identity is unavailable. Reset certificate and set up again.",
        );
      }
      if (systemRouting && !bootstrap.ownedCaTrusted) {
        this.setProxyOutput(
          "Approve the Windows certificate dialog to trust the Transmog interception CA.",
          "progress",
        );
        await invoke<void>("install_certificate", {
          path: bootstrap.caCertificatePath,
          sha256,
        });
      }
      bootstrap = await invoke<DesktopBootstrap>("desktop_bootstrap", { inspectCaKey: true });
      this.updateCertificateState(bootstrap);
      if (systemRouting && !bootstrap.ownedCaTrusted) {
        throw new Error("Windows did not report the certificate as trusted.");
      }
      this.caSha256 = sha256;
      const trustClient =
        "For HTTPS decryption, trust the public certificate through Windows or in the app you want to inspect.";
      const message = systemRouting
        ? "HTTPS interception is ready. Windows trusts the exact Transmog certificate for the current user."
        : "Interception certificate ready.";
      this.setProxyOutput(message, "success");
      if (this.proxyLifecycle !== "stopped" || !this.decryptHttps) {
        const next = this.decryptHttps
          ? "Review HTTPS settings in Connection."
          : this.decryptInput.checked
            ? "Save Connection settings to enable HTTPS decryption."
            : "Turn on Decrypt HTTPS traffic in Connection, then save.";
        this.showNotice(
          "Interception certificate is ready",
          systemRouting ? next : `${trustClient} ${next}`,
          "HTTPS settings",
          "https-settings",
        );
      } else {
        this.showNotice(
          systemRouting ? "HTTPS interception is ready" : "Interception certificate is ready",
          systemRouting ? "You can start the proxy now." : trustClient,
          systemRouting ? "Start proxy" : "Certificate settings",
          systemRouting ? "start-proxy" : "certificate-settings",
        );
      }
    } catch (error: unknown) {
      const message = `Certificate setup failed: ${describeError(error)}`;
      await this.refreshCertificateState();
      this.setProxyOutput(message, "error");
      this.showNotice("Certificate setup needs attention", message, "Try again", "setup-ca");
    }
  }

  async createCa(): Promise<void> {
    const data = new FormData(this.proxyForm);
    try {
      const identity = await invoke<CaIdentity>("create_ca", {
        request: {
          certificatePath: String(data.get("certificate") ?? ""),
          privateKeyPath: String(data.get("privateKey") ?? ""),
          commonName: "Transmog local interception CA",
          validityDays: 3650,
        },
      });
      this.caSha256 = identity.sha256;
      const message = `CA created with a current-user-only private-key ACL. SHA-256 ${identity.sha256}. Trust the public CA before intercepting HTTPS.`;
      this.setProxyOutput(message, "success");
      this.diagnostic = message;
    } catch (error: unknown) {
      const message = `CA creation failed: ${describeError(error)}`;
      this.setProxyOutput(message, "error");
      this.diagnostic = message;
    }
  }

  async installCa(): Promise<void> {
    const certificate = this.certificateBootstrap;
    if (
      !certificate?.ownedCaSha256 ||
      !this.publicCertificateAvailable ||
      this.windowsCertificateTrusted
    ) {
      return;
    }
    const sha256 = certificate.ownedCaSha256;
    await this.runProxyChange("certificate-setup", async () => {
      this.setProxyOutput(
        "Approve the Windows certificate dialog to trust the interception certificate.",
        "progress",
      );
      try {
        await invoke<void>("install_certificate", { path: certificate.caCertificatePath, sha256 });
        if (!(await this.refreshCertificateState())?.ownedCaTrusted) {
          throw new Error("Windows could not verify the certificate’s trust.");
        }
        this.setProxyOutput(
          "Interception certificate trusted for this Windows account.",
          "success",
        );
      } catch (error: unknown) {
        await this.refreshCertificateState();
        this.setProxyOutput(
          `Windows certificate trust could not be set up. ${describeError(error)}`,
          "error",
        );
      }
    });
  }

  private async refreshCertificateState(): Promise<DesktopBootstrap | null> {
    const statusRevision = this.statusRevision;
    const certificateRevision = ++this.certificateRevision;
    try {
      const bootstrap = await invoke<DesktopBootstrap>("desktop_bootstrap", { inspectCaKey: true });
      if (this.isConnected && this.certificateRevision === certificateRevision) {
        this.updateCertificateState(bootstrap, this.statusRevision === statusRevision);
      }
      return bootstrap;
    } catch (error: unknown) {
      if (this.isConnected && this.certificateRevision === certificateRevision) {
        this.diagnostic = `Certificate status could not be refreshed. ${describeError(error)}`;
      }
      return null;
    }
  }

  async copyPublicCertificatePath(): Promise<void> {
    if (!this.publicCertificateAvailable || !this.certificateBootstrap) {
      return;
    }
    try {
      await navigator.clipboard.writeText(this.certificateBootstrap.caCertificatePath);
      this.setProxyOutput("Public certificate path copied.", "success");
    } catch (error: unknown) {
      this.setProxyOutput(
        `Copy the path in Public CA certificate. ${describeError(error)}`,
        "error",
      );
    }
  }

  async removeCa(): Promise<void> {
    await this.runProxyChange("certificate-removal", () => this.removeCaNow());
  }

  private async removeCaNow(): Promise<void> {
    const data = new FormData(this.proxyForm);
    try {
      await invoke<void>("remove_certificate", { sha256: String(data.get("thumbprint") ?? "") });
      this.updateCertificateState(await invoke<DesktopBootstrap>("desktop_bootstrap"));
      const message =
        "The exact public CA was removed from current-user trust. The durable CA files and identity were retained so setup can trust it again.";
      this.setProxyOutput(message, "success");
      this.diagnostic = message;
    } catch (error: unknown) {
      const message = `CA removal failed: ${describeError(error)}`;
      this.setProxyOutput(message, "error");
      this.diagnostic = message;
    }
  }

  private async stopProxyNow(): Promise<void> {
    const manual = this.activeRoutingMode === "manual";
    this.setProxyOutput(
      manual ? "Finishing requests…" : "Stopping and restoring host settings…",
      "progress",
    );
    try {
      const status = await invoke<AppStatus>("stop_application");
      this.acceptAppStatus(status);
      this.setProxyOutput(
        status.lifecycle === "draining"
          ? "Existing requests and active connections will finish. Start proxy resumes the same listener."
          : manual
            ? "Proxy stopped."
            : "Proxy stopped and any current-user Windows proxy changes were restored.",
        status.lifecycle === "draining" ? "progress" : "success",
      );
    } catch (error: unknown) {
      const message = `Stop failed: ${describeError(error)}`;
      this.setProxyOutput(message, "error");
      this.diagnostic = message;
    }
  }

  async recoverProxy(): Promise<void> {
    await this.runProxyChange("recovering", () => this.recoverProxyNow());
  }

  private async recoverProxyNow(): Promise<void> {
    this.setProxyOutput("Recovering Windows proxy settings…", "progress");
    try {
      const restored = await invoke<boolean>("recover_windows_proxy");
      const message = restored
        ? "Windows proxy recovery completed."
        : "No Windows proxy recovery journal was present.";
      this.setProxyOutput(message, "success");
      this.diagnostic = message;
    } catch (error: unknown) {
      const message = `Recovery failed: ${describeError(error)}`;
      this.setProxyOutput(message, "error");
      this.diagnostic = message;
    }
  }

  private setSettingsBusy(busy: boolean): void {
    this.settingsBusy = busy;
    this.$emit("settings-busy", busy);
  }

  private populateProductSettings(state: ProductState, routingRevision: number): void {
    this.populatePreferences(state);
    this.populateConnection(state, routingRevision);
    this.applyTheme(state.preferences.theme, state.workspace);
    this.settingsLoadError = "";
    this.settingsLoaded = true;
    void this.refreshBufferStatus();
  }

  async retryProductSettings(section: SettingsFormSection): Promise<void> {
    if (this.settingsLoaded) {
      return;
    }
    const intent = capturePresentationIntent(this);
    const current = captureRenderScope(this, () => [this.view, this.settingsSection]);
    try {
      await this.loadSettings(section);
      if (this.settingsLoaded && current() && intent.current()) {
        const control =
          section === "connection"
            ? this.decryptInput
            : (this.settingsForm.elements.namedItem("theme") as HTMLSelectElement);
        await this.focusSettingsControl(section, control);
      }
    } finally {
      intent.dispose();
    }
  }

  async loadSettings(section: SettingsFormSection = "preferences"): Promise<void> {
    if (this.settingsBusy || this.proxyPending || !this.proxyReady) {
      return;
    }
    const routingRevision = this.routingRevision;
    this.setSettingsBusy(true);
    try {
      const state = await invoke<ProductState>("product_state");
      if (!this.settingsLoaded) {
        this.populateProductSettings(state, routingRevision);
      } else if (section === "connection") {
        this.populateConnection(state, routingRevision);
        this.connectionText = "Changes reverted.";
      } else {
        this.populatePreferences(state);
        this.applyTheme(state.preferences.theme, state.workspace);
        this.settingsText = "Changes reverted.";
      }
    } catch (error: unknown) {
      const message = "Settings could not be loaded: " + describeError(error);
      if (!this.settingsLoaded) {
        this.settingsLoadError = message;
      } else if (section === "connection") {
        this.connectionError = message;
      } else {
        this.settingsError = message;
      }
    } finally {
      this.setSettingsBusy(false);
    }
  }

  async saveSettings(event: Event, section: SettingsFormSection = "preferences"): Promise<void> {
    event.preventDefault();
    if (this.settingsBusy || this.proxyPending || !this.proxyReady || !this.settingsLoaded) {
      return;
    }
    const connection = section === "connection";
    const data = new FormData(connection ? this.connectionForm : this.settingsForm);
    const requestedDecrypt = this.decryptInput.checked;
    const expectedDecryptHttps = this.savedDecryptHttps;
    const routingRevision = this.routingRevision;
    let processInspection: ProductState["preferences"]["processInspection"] | undefined;
    if (connection) {
      this.connectionError = "";
      this.httpsEnableError = false;
      try {
        processInspection = this.processInspectionEditor.policy();
      } catch (error: unknown) {
        this.connectionError = describeError(error);
        return;
      }
    } else {
      this.settingsError = "";
    }
    const expectedProcessInspection = connection
      ? structuredClone(this.processInspectionEditor.baseline)
      : undefined;
    this.setSettingsBusy(true);
    try {
      const state = await invoke<ProductState>("product_state");
      if (processInspection) {
        this.updateConnectionSettings(state, data, requestedDecrypt, processInspection);
      } else {
        this.updatePreferences(state, data);
      }
      const saved = await invoke<ProductState>("save_product_state", {
        productState: state,
        section,
        expectedProcessInspection,
        expectedDecryptHttps: connection ? expectedDecryptHttps : undefined,
      });
      if (connection) {
        this.populateConnection(saved, routingRevision);
        this.connectionText = "Connection settings saved.";
        await this.refreshStatus();
      } else {
        this.populatePreferences(saved);
        this.applyTheme(saved.preferences.theme, saved.workspace);
        this.settingsText = "Settings saved.";
        await this.refreshBufferStatus();
      }
    } catch (error: unknown) {
      const message = "Settings save failed: " + describeError(error);
      if (connection) {
        this.connectionError = message;
        if (requestedDecrypt && !expectedDecryptHttps) {
          const certificate = await this.refreshCertificateState();
          this.httpsEnableError = Boolean(certificate && this.certificateSetupNeeded);
        }
      } else {
        this.settingsError = message;
      }
    } finally {
      this.setSettingsBusy(false);
    }
  }

  private updateConnectionSettings(
    state: ProductState,
    data: FormData,
    decryptHttps: boolean,
    processInspection: ProductState["preferences"]["processInspection"],
  ): void {
    const port = data.get("customProxyPort") === "on" ? Number(data.get("proxyPort")) : null;
    if (port !== null && (!Number.isSafeInteger(port) || port < 1 || port > 65535)) {
      throw new Error("Choose a proxy port between 1 and 65535.");
    }
    state.preferences.processInspection = processInspection;
    state.preferences.decryptHttps = decryptHttps;
    state.preferences.allowRemoteClients = data.get("allowRemoteClients") === "on";
    state.preferences.proxyPort = port;
    state.preferences.isolateOutboundConnections = data.get("isolateOutbound") === "on";
  }

  private updatePreferences(state: ProductState, data: FormData): void {
    state.preferences.theme = String(data.get("theme")) as ProductState["preferences"]["theme"];
    const bufferMode = String(data.get("bufferMode"));
    state.privacy.bufferLimit =
      bufferMode === "custom"
        ? { mode: "custom", bytes: Math.round(Number(data.get("bufferSize")) * 1073741824) }
        : { mode: bufferMode === "unlimited" ? "unlimited" : "automatic" };
    const entryLimit = data.get("limitEntries") === "on" ? Number(data.get("maxEntries")) : null;
    if (entryLimit !== null && (!Number.isSafeInteger(entryLimit) || entryLimit < 1)) {
      throw new Error("Choose a positive whole number of live entries.");
    }
    state.privacy.maxLiveEntries = entryLimit;
    state.privacy.terminateWebsocketsOnClear = data.get("terminateWebsocketsOnClear") === "on";
    state.privacy.retainRequestBodies = data.get("requestBodies") === "on";
    const bodyMode = String(data.get("requestBodyLimit"));
    const customMb = Number(data.get("customRequestBodyLimit"));
    if (
      bodyMode === "custom" &&
      (!Number.isFinite(customMb) || customMb < 1 || customMb > 1000000)
    ) {
      throw new Error("Enter a body limit from 1 to 1,000,000 MB (1 TB).");
    }
    state.privacy.requestBodyLimit =
      bodyMode === "unlimited"
        ? null
        : bodyMode === "custom"
          ? Math.round(customMb * 1000000)
          : 25000000;
    state.privacy.customRequestBodyLimit = bodyMode === "custom";
    state.privacy.redactSensitiveHeaders = data.get("redactHeaders") === "on";
    state.privacy.retainResponseBodies = data.get("defaultBodies") === "on";
    state.privacy.retainBodySamples = data.get("captureBodies") === "on";
    state.privacy.rememberRecentArtifacts = data.get("rememberArtifacts") === "on";
    state.privacy.includePathsInSupportBundles = data.get("supportPaths") === "on";
  }

  async refreshDiagnostics(): Promise<void> {
    await this.runSupport(async () => {
      try {
        const report = await invoke<{
          applicationVersion: string;
          runtime: { operatingSystem: string; architecture: string; webviewVersion: string | null };
          events: Array<{ level: string }>;
          privacyNotice: string;
          unresolvedProcessConnections: number;
        }>("diagnostics_report");
        this.supportFacts = [
          { label: "Transmog version", value: report.applicationVersion },
          { label: "Operating system", value: report.runtime.operatingSystem },
          { label: "Architecture", value: report.runtime.architecture },
          { label: "WebView2", value: report.runtime.webviewVersion ?? "Unavailable" },
          { label: "Recent events", value: String(report.events.length) },
          {
            label: "Unresolved process connections",
            value: String(report.unresolvedProcessConnections ?? 0),
          },
          {
            label: "Warnings / errors",
            value: String(report.events.filter((event) => event.level !== "info").length),
          },
          { label: "Privacy", value: report.privacyNotice },
        ];
        this.supportDetails = JSON.stringify(report, null, 2);
        this.supportText = "Diagnostics refreshed.";
      } catch (error: unknown) {
        this.supportText = "Diagnostics unavailable: " + describeError(error);
      }
    });
  }
  async createSupportBundle(event: Event): Promise<void> {
    event.preventDefault();
    const data = new FormData(this.supportForm);
    await this.runSupport(async () => {
      try {
        const destination = await invoke<string | null>("pick_support_path");
        if (destination === null) {
          this.supportText = "Support bundle canceled.";
          return;
        }
        const result = await invoke<{
          destination: string;
          bytes: number;
          includedRecentPaths: boolean;
        }>("create_support_bundle", {
          destination,
          includeRecentPaths: data.get("includePaths") === "on",
        });
        this.supportFacts = [
          { label: "Saved to", value: result.destination },
          { label: "Size", value: result.bytes.toLocaleString() + " bytes" },
          {
            label: "Recent paths",
            value: result.includedRecentPaths
              ? "Included by your saved privacy preference"
              : "Excluded",
          },
        ];
        this.supportDetails = JSON.stringify(result, null, 2);
        this.supportText = "Support bundle saved.";
      } catch (error: unknown) {
        this.supportText = "Support bundle failed: " + describeError(error);
        await this.showError("Support bundle could not be saved", describeError(error));
      }
    });
  }
  async prepareUpdate(): Promise<void> {
    await this.runSupport(async () => {
      try {
        await invoke<AppStatus>("prepare_update_handoff");
        this.acceptAppStatus(await invoke<AppStatus>("app_status"));
        this.supportText =
          "Proxy, recording, breakpoints and Windows host changes are stopped. Close Transmog before running the installer.";
      } catch (error: unknown) {
        this.supportText = "Update handoff failed: " + describeError(error);
      }
    });
  }

  async refreshStatus(): Promise<string | null> {
    const revision = ++this.statusRevision;
    try {
      const status = await invoke<AppStatus>("app_status");
      if (this.isConnected && revision === this.statusRevision) {
        this.acceptAppStatus(status);
      }
      return null;
    } catch (error: unknown) {
      if (!this.isConnected || revision !== this.statusRevision) {
        return null;
      }
      const message = `Status unavailable: ${describeError(error)}`;
      this.diagnostic = message;
      return message;
    }
  }

  private populateConnection(state: ProductState, routingRevision: number): void {
    if (routingRevision === this.routingRevision) {
      this.applyProxyRouting(state.preferences.configureSystemProxy ?? true);
    }
    this.connectionDirty = false;
    this.editedConnectionFields.clear();
    this.connectionError = "";
    this.httpsEnableError = false;
    this.processInspectionEditor.populate(
      state.preferences.processInspection ?? { mode: "all", include: [], exclude: [] },
    );
    const elements = this.connectionForm.elements;
    this.customPortEnabled = state.preferences.proxyPort != null;
    (elements.namedItem("customProxyPort") as HTMLInputElement).checked = this.customPortEnabled;
    (elements.namedItem("proxyPort") as HTMLInputElement).value = String(
      state.preferences.proxyPort ?? 8080,
    );
    (elements.namedItem("allowRemoteClients") as HTMLInputElement).checked =
      state.preferences.allowRemoteClients ?? false;
    this.decryptHttps = state.preferences.decryptHttps ?? true;
    this.savedDecryptHttps = this.decryptHttps;
    this.presentCertificateState();
    this.decryptInput.checked = this.decryptHttps;
    (elements.namedItem("isolateOutbound") as HTMLInputElement).checked =
      state.preferences.isolateOutboundConnections ?? true;
  }

  private populatePreferences(state: ProductState): void {
    this.settingsDirty = false;
    this.settingsError = "";
    this.supportPathsAllowed = state.privacy.includePathsInSupportBundles;
    const elements = this.settingsForm.elements;
    this.bufferMode = state.privacy.bufferLimit.mode;
    this.entryLimitEnabled = state.privacy.maxLiveEntries !== null;
    (elements.namedItem("limitEntries") as HTMLInputElement).checked = this.entryLimitEnabled;
    (elements.namedItem("maxEntries") as HTMLInputElement).value = String(
      state.privacy.maxLiveEntries ?? 10000,
    );
    (elements.namedItem("bufferMode") as HTMLSelectElement).value = this.bufferMode;
    (elements.namedItem("bufferSize") as HTMLInputElement).value = String(
      state.privacy.bufferLimit.mode === "custom"
        ? state.privacy.bufferLimit.bytes / 1073741824
        : 1,
    );
    (elements.namedItem("theme") as HTMLSelectElement).value = state.preferences.theme;
    (elements.namedItem("terminateWebsocketsOnClear") as HTMLInputElement).checked =
      state.privacy.terminateWebsocketsOnClear;
    (elements.namedItem("requestBodies") as HTMLInputElement).checked =
      state.privacy.retainRequestBodies;
    this.requestBodyMode =
      state.privacy.requestBodyLimit === null
        ? "unlimited"
        : state.privacy.customRequestBodyLimit || state.privacy.requestBodyLimit !== 25000000
          ? "custom"
          : "25000000";
    (elements.namedItem("requestBodyLimit") as HTMLSelectElement).value = this.requestBodyMode;
    (elements.namedItem("customRequestBodyLimit") as HTMLInputElement).value = String(
      (state.privacy.requestBodyLimit ?? 25000000) / 1000000,
    );
    (elements.namedItem("redactHeaders") as HTMLInputElement).checked =
      state.privacy.redactSensitiveHeaders;
    (elements.namedItem("defaultBodies") as HTMLInputElement).checked =
      state.privacy.retainResponseBodies;
    (elements.namedItem("captureBodies") as HTMLInputElement).checked =
      state.privacy.retainBodySamples;
    (elements.namedItem("rememberArtifacts") as HTMLInputElement).checked =
      state.privacy.rememberRecentArtifacts;
    (elements.namedItem("supportPaths") as HTMLInputElement).checked =
      state.privacy.includePathsInSupportBundles;
  }

  private setProxyOutput(message: string, kind: "progress" | "success" | "error"): void {
    this.proxyOutputFromCertificateBlock = false;
    this.proxyText = message;
    this.proxyKind = kind;
  }
}

SettingsWorkspace.define("settings-workspace");
