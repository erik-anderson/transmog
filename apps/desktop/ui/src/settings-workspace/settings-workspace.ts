import initialState from '../initial-state.json';
import { attr, observable } from '@microsoft/webui-framework';
import { invoke } from '@tauri-apps/api/core';
import { WorkspaceElement } from '../workspace-element.js';
import type { AppStatus, CaIdentity, DesktopBootstrap, ProductState, WorkspacePreferences } from '../models.js';
import { describeError } from '../utilities.js';

export class SettingsWorkspace extends WorkspaceElement {
  @attr view = 'traffic';
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

  protected hydratedCallback(): void { this.ready = this.initializeShell(); }
  private renderAppStatus(status: AppStatus): void { this.proxyLifecycle = status.lifecycle; this.$emit('status-changed', status); }
  private applyTheme(theme: ProductState['preferences']['theme'], workspace?: WorkspacePreferences): void {
    this.$emit('preferences-changed', { theme, pageSize: Number(new FormData(this.settingsForm).get('pageSize')), workspace });
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
      this.applyTheme(productState.preferences.theme, productState.workspace);
      this.renderAppStatus(status);
      const recovery = this.caRecoveryRequired(bootstrap);
      const certificate = recovery
        ? 'The saved interception certificate needs to be reset. Reset certificate and set up again to create and trust a new one.'
        : bootstrap.ownedCaSha256 === null
          ? 'No app-owned trusted CA is recorded. Create and trust one before intercepting HTTPS.'
          : bootstrap.ownedCaTrusted
            ? `Owned CA ${bootstrap.ownedCaSha256} is present in current-user trust.`
            : `Owned CA ${bootstrap.ownedCaSha256} is not present in current-user trust.`;
      const ready = bootstrap.caFilesPresent && bootstrap.ownedCaTrusted;
      this.setProxyOutput(`${certificate} Diagnostic log: ${bootstrap.diagnosticsPath}`, ready ? 'success' : recovery ? 'error' : 'progress');
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
    this.proxyPending = kind;
    this.$emit('proxy-operation',kind);
    try { await operation(); await this.refreshStatus(); }
    finally { this.proxyPending = ''; this.$emit('proxy-operation',''); }
  }

  private async startProxyNow(event?: Event): Promise<void> {
    event?.preventDefault();
    const data = new FormData(this.proxyForm);
    try {
      const bootstrap = await invoke<DesktopBootstrap>('desktop_bootstrap');
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
      this.setProxyOutput('Proxy stopped and any current-user Windows proxy changes were restored.', 'success');
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

  async loadSettings(): Promise<void> {
    try {
      const state = await invoke<ProductState>('product_state');
      this.populateSettings(state);
      this.applyTheme(state.preferences.theme, state.workspace);
      this.supportText = `Loaded schema ${state.schemaVersion}; ${state.recentArtifacts.length} recent artifact reference(s).`;
    } catch (error: unknown) {
      this.supportText = `Settings load failed: ${describeError(error)}`;
    }
  }

  async saveSettings(event: Event): Promise<void> {
    event.preventDefault();
    try {
      const state = await invoke<ProductState>('product_state');
      const data = new FormData(this.settingsForm);
      state.preferences.theme = String(data.get('theme')) as ProductState['preferences']['theme'];
      state.preferences.sessionPageSize = Number(data.get('pageSize'));
      state.preferences.configureSystemProxy = true;
      state.privacy.retainResponseBodies = data.get('defaultBodies') === 'on';
      state.privacy.retainBodySamples = data.get('captureBodies') === 'on';
      state.privacy.rememberRecentArtifacts = data.get('rememberArtifacts') === 'on';
      state.privacy.includePathsInSupportBundles = data.get('supportPaths') === 'on';
      const saved = await invoke<ProductState>('save_product_state', { productState: state });
      this.populateSettings(saved);
      this.applyTheme(saved.preferences.theme, saved.workspace);
      this.supportText = `Saved product-state schema ${saved.schemaVersion}.`;
    } catch (error: unknown) {
      this.supportText = `Settings save failed: ${describeError(error)}`;
    }
  }

  async refreshDiagnostics(): Promise<void> {
    try {
      const report = await invoke<Record<string, unknown>>('diagnostics_report');
      this.supportText = JSON.stringify(report, null, 2);
    } catch (error: unknown) {
      this.supportText = `Diagnostics unavailable: ${describeError(error)}`;
    }
  }

  async createSupportBundle(event: Event): Promise<void> {
    event.preventDefault();
    const data = new FormData(this.supportForm);
    try {
      const result = await invoke<Record<string, unknown>>('create_support_bundle', {
        destination: String(data.get('destination') ?? ''),
        includeRecentPaths: data.get('includePaths') === 'on',
      });
      this.supportText = JSON.stringify(result, null, 2);
    } catch (error: unknown) {
      this.supportText = `Support bundle failed: ${describeError(error)}`;
    }
  }

  async prepareUpdate(): Promise<void> {
    try {
      await invoke<AppStatus>('prepare_update_handoff');
      this.renderAppStatus(await invoke<AppStatus>('app_status'));
      this.supportText = 'Proxy, capture, breakpoints, and Windows host changes are stopped. Close Transmog before running the installer.';
    } catch (error: unknown) {
      this.supportText = `Update handoff failed: ${describeError(error)}`;
    }
  }

  async refreshStatus(): Promise<void> {
    try {
      const status = await invoke<AppStatus>('app_status');
      this.renderAppStatus(status);
    } catch (error: unknown) {
      this.diagnostic = `Status unavailable: ${describeError(error)}`;
    }
  }

  private populateSettings(state: ProductState): void {
    const elements = this.settingsForm.elements;
    (elements.namedItem('theme') as HTMLSelectElement).value = state.preferences.theme;
    (elements.namedItem('pageSize') as HTMLInputElement).value = String(state.preferences.sessionPageSize);
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
