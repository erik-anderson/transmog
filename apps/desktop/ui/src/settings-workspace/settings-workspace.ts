import { captureFocus, captureRenderScope } from "../rendering.js";
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
      this.settingsForm.elements.namedItem("customProxyPort") as HTMLInputElement
    ).checked;
  }
  @observable requestBodyMode = initialState.requestBodyMode;
  updateRequestBodyLimit(): void {
    this.requestBodyMode = (
      this.settingsForm.elements.namedItem("requestBodyLimit") as HTMLSelectElement
    ).value;
  }
  @observable bufferMode = "automatic";
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
  @observable settingsSection = "preferences";
  @observable settingsDirty = false;
  @observable settingsBusy = false;
  @observable settingsText = "";
  @observable settingsError = "";
  @observable certificateReady = false;
  @observable certificateRecovery = false;
  @observable interceptionCertificateUnavailable = true;
  @observable certificateReturnVisible = initialState.certificateReturnVisible;
  @observable certificateDetailsOpen = initialState.certificateDetailsOpen;
  @observable httpsEnableError = initialState.httpsEnableError;
  certificateSettingsButton!: HTMLButtonElement;
  interceptionSummary!: HTMLElement;
  private certificateReturnFocus: HTMLElement | null = null;
  @observable hostRecoveryPending = false;
  @observable supportBusy = false;
  @observable supportPathsAllowed = false;
  @observable supportDetails = "";
  @observable supportFacts: Array<{ label: string; value: string }> = [];
  showSettingsSection(section: string): void {
    this.settingsSection = section;
    if (section !== "connection") {
      this.certificateReturnVisible = false;
    }
  }
  async openInterceptionCertificate(event?: Event, returnToPreferences = true): Promise<void> {
    const focus = captureFocus(this);
    const eventTarget = event?.currentTarget;
    this.certificateReturnFocus =
      eventTarget instanceof HTMLElement ? eventTarget : this.certificateSettingsButton;
    this.certificateReturnVisible = returnToPreferences;
    this.certificateDetailsOpen = true;
    this.settingsSection = "connection";
    const current = captureRenderScope(this, () => [
      this.view,
      this.settingsSection,
      this.certificateDetailsOpen,
    ]);
    await this.$whenRendered();
    if (!current() || !focus()) {
      return;
    }
    this.interceptionSummary.focus();
    this.interceptionSummary.scrollIntoView({ block: "nearest" });
  }
  certificateDisclosureChanged(event: Event): void {
    this.certificateDetailsOpen = (event.target as HTMLDetailsElement).open;
  }
  async backToPreferences(): Promise<void> {
    const focus = captureFocus(this);
    this.showSettingsSection("preferences");
    const current = captureRenderScope(this, () => [
      this.view,
      this.settingsSection,
      this.certificateReturnFocus,
    ]);
    await this.$whenRendered();
    if (!current() || !focus()) {
      return;
    }
    const target = this.certificateReturnFocus;
    if (target?.isConnected && target.getClientRects().length) {
      target.focus();
    } else {
      this.certificateSettingsButton.focus();
    }
  }
  private profileEvent(event?: Event): boolean {
    return (
      event
        ?.composedPath()
        .some(
          (target) =>
            target instanceof HTMLElement && target.localName === "network-profile-settings",
        ) ?? false
    );
  }
  markSettingsDirty(event?: Event): void {
    if (this.profileEvent(event)) {
      return;
    }
    this.settingsDirty = true;
    this.settingsText = "Unsaved changes";
  }
  settingsKeyboard(event: KeyboardEvent): void {
    if (this.profileEvent(event)) {
      return;
    }
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
      event.preventDefault();
      if (this.settingsDirty) {
        this.settingsForm.requestSubmit();
      }
    }
  }
  private certificateBootstrap: DesktopBootstrap | undefined;
  private certificateRevision = 0;
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
    this.interceptionCertificateUnavailable = decrypting && (!available || (system && !trusted));
    this.certificateRecovery = decrypting && recovery;
    this.certificateReady = !this.interceptionCertificateUnavailable;
    this.interceptionCertificateStatus = !decrypting
      ? "HTTP remains inspectable. HTTPS tunnels keep message contents encrypted."
      : recovery
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
      const [bootstrap, productState, status] = await Promise.all([
        invoke<DesktopBootstrap>("desktop_bootstrap"),
        invoke<ProductState>("product_state"),
        invoke<AppStatus>("app_status"),
      ]);
      this.certificatePath = bootstrap.caCertificatePath;
      this.privateKeyPath = bootstrap.caPrivateKeyPath;
      this.caSha256 = bootstrap.ownedCaSha256 ?? "";
      this.diagnosticsPathText = bootstrap.diagnosticsPath;
      this.populateSettings(productState, routingRevision);
      void this.refreshBufferStatus();
      this.applyTheme(productState.preferences.theme, productState.workspace);
      this.acceptAppStatus(status);
      this.setProxyOutput("", "success");
      if (status.hostRestorePending) {
        this.showNotice(
          "Windows proxy recovery required",
          "Restore the Windows proxy settings saved by a previous run. Manual proxy is available while recovery is pending.",
          "Restore settings",
          "recover-proxy",
        );
      }
      this.updateCertificateState(bootstrap);
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
    await this.runProxyChange("starting", () => this.startProxyNow());
  }
  async startHttpsTunnels(): Promise<void> {
    await this.runProxyChange("starting", async () => {
      this.decryptHttps = false;
      (this.settingsForm.elements.namedItem("decryptHttps") as HTMLInputElement).checked = false;
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
        this.decryptHttps = false;
        // Keep unrelated preference drafts, including the process lists, intact.
        (this.settingsForm.elements.namedItem("decryptHttps") as HTMLInputElement).checked = false;
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
      } finally {
        // The explicit HTTPS choice commits before binding; even a later start
        // failure must not let an unrelated draft save turn decryption back on.
        try {
          const saved = await invoke<ProductState>("product_state");
          this.decryptHttps = saved.preferences.decryptHttps;
          this.savedDecryptHttps = this.decryptHttps;
          (this.settingsForm.elements.namedItem("decryptHttps") as HTMLInputElement).checked =
            this.decryptHttps;
        } catch (error: unknown) {
          this.diagnostic = "Saved HTTPS choice could not be refreshed: " + describeError(error);
        }
        try {
          this.updateCertificateState(await invoke<DesktopBootstrap>("desktop_bootstrap"));
        } catch (error: unknown) {
          this.diagnostic = "Certificate status could not be refreshed: " + describeError(error);
        }
      }
    });
  }
  async stopProxy(): Promise<void> {
    await this.runProxyChange("stopping", () => this.stopProxyNow());
  }
  private async runProxyChange(kind: string, operation: () => Promise<void>): Promise<void> {
    if (this.proxyPending || this.routingSavePending) {
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

  private async startProxyNow(event?: Event): Promise<void> {
    event?.preventDefault();

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
      const decryptInput = this.settingsForm.elements.namedItem("decryptHttps") as HTMLInputElement;
      const hasHttpsDraft = decryptInput.checked !== this.savedDecryptHttps;
      this.decryptHttps = productState.preferences.decryptHttps;
      this.savedDecryptHttps = this.decryptHttps;
      if (!hasHttpsDraft) {
        decryptInput.checked = this.decryptHttps;
      }
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
        this.showSettingsSection("connection");
        this.activateView("settings");
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
      if (
        this.proxyLifecycle !== "stopped" ||
        !this.decryptHttps ||
        this.certificateReturnVisible
      ) {
        const requested = (this.settingsForm.elements.namedItem("decryptHttps") as HTMLInputElement)
          .checked;
        const next = this.decryptHttps
          ? "Review HTTPS settings in Preferences."
          : requested
            ? "Save settings in Preferences to enable HTTPS decryption."
            : "Enable HTTPS decryption in Preferences, then save.";
        this.showNotice(
          "Interception certificate is ready",
          systemRouting ? next : `${trustClient} ${next}`,
          "Back to preferences",
          "https-preferences",
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

  async loadSettings(): Promise<void> {
    if (this.settingsBusy || this.proxyPending) {
      return;
    }
    const hadHttpsError = this.httpsEnableError;
    const routingRevision = this.routingRevision;
    this.settingsBusy = true;
    this.settingsError = "";
    this.httpsEnableError = false;
    try {
      const state = await invoke<ProductState>("product_state");
      this.populateSettings(state, routingRevision);
      if (hadHttpsError) {
        this.setProxyOutput("HTTPS changes reverted.", "success");
      }
      this.applyTheme(state.preferences.theme, state.workspace);
      this.settingsText = "Changes reverted.";
    } catch (error: unknown) {
      this.settingsError = "Settings could not be loaded: " + describeError(error);
    } finally {
      this.settingsBusy = false;
    }
  }
  async saveSettings(event: Event): Promise<void> {
    event.preventDefault();
    if (this.settingsBusy || this.proxyPending) {
      return;
    }
    const data = new FormData(this.settingsForm);
    const decryptRequested = (
      this.settingsForm.elements.namedItem("decryptHttps") as HTMLInputElement
    ).checked;
    const expectedDecryptHttps = this.savedDecryptHttps;
    const routingRevision = this.routingRevision;
    const expectedProcessInspection = structuredClone(this.processInspectionEditor.baseline);
    const httpsChanged = decryptRequested !== expectedDecryptHttps;
    const enablingHttps = decryptRequested && httpsChanged;
    const hadHttpsError = this.httpsEnableError;
    this.httpsEnableError = false;
    let processInspection;
    try {
      processInspection = this.processInspectionEditor.policy();
    } catch (error: unknown) {
      this.settingsError = describeError(error);
      return;
    }
    this.settingsBusy = true;
    this.settingsError = "";
    try {
      const state = await invoke<ProductState>("product_state");
      state.preferences.processInspection = processInspection;
      state.preferences.theme = String(data.get("theme")) as ProductState["preferences"]["theme"];
      state.preferences.decryptHttps = decryptRequested;
      state.preferences.allowRemoteClients = data.get("allowRemoteClients") === "on";
      const port = data.get("customProxyPort") === "on" ? Number(data.get("proxyPort")) : null;
      if (port !== null && (!Number.isSafeInteger(port) || port < 1 || port > 65535)) {
        throw new Error("Choose a proxy port between 1 and 65535.");
      }
      state.preferences.proxyPort = port;
      state.preferences.isolateOutboundConnections = data.get("isolateOutbound") === "on";
      state.privacy.bufferLimit =
        this.bufferMode === "custom"
          ? { mode: "custom", bytes: Math.round(Number(data.get("bufferSize")) * 1073741824) }
          : { mode: this.bufferMode === "unlimited" ? "unlimited" : "automatic" };
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
      const saved = await invoke<ProductState>("save_product_state", {
        productState: state,
        expectedProcessInspection,
        expectedDecryptHttps,
        preserveProxyRouting: true,
      });
      this.populateSettings(saved, routingRevision);
      this.applyTheme(saved.preferences.theme, saved.workspace);
      await this.refreshBufferStatus();
      await this.refreshStatus();
      if (httpsChanged || hadHttpsError) {
        this.setProxyOutput("HTTPS setting saved.", "success");
      }
      this.settingsText =
        this.proxyLifecycle === "running" || this.proxyLifecycle === "draining"
          ? "Saved. Inspection changes apply to new connections."
          : "Settings saved.";
    } catch (error: unknown) {
      this.settingsError = "Settings save failed: " + describeError(error);
      this.httpsEnableError = enablingHttps;
      if (enablingHttps) {
        if (!this.proxyPending) {
          await this.refreshCertificateState();
        }
        this.setProxyOutput(this.settingsError, "error");
      }
    } finally {
      this.settingsBusy = false;
    }
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

  private populateSettings(state: ProductState, routingRevision: number): void {
    if (routingRevision === this.routingRevision) {
      this.applyProxyRouting(state.preferences.configureSystemProxy ?? true);
    }
    this.settingsDirty = false;
    this.processInspectionEditor.populate(
      state.preferences.processInspection ?? { mode: "all", include: [], exclude: [] },
    );
    this.supportPathsAllowed = state.privacy.includePathsInSupportBundles;
    const elements = this.settingsForm.elements;
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
    (elements.namedItem("decryptHttps") as HTMLInputElement).checked = this.decryptHttps;
    (elements.namedItem("isolateOutbound") as HTMLInputElement).checked =
      state.preferences.isolateOutboundConnections ?? true;
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
