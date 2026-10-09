import { WebUIElement, observable } from '@microsoft/webui-framework';
import { Channel, invoke } from '@tauri-apps/api/core';
import initialState from '../initial-state.json';
import { describeError } from '../utilities.js';

export interface UpdateStatus {
  phase: string;
  currentVersion: string;
  version: string | null;
  notes: string;
  message: string;
  downloadedBytes: number;
  totalBytes: number | null;
  scheduled: boolean;
  suppressed: boolean;
  remindAfterUnixMs: number;
}

/** The native coordinator owns eligibility, signed bytes, and installation consent. */
export class AppUpdates extends WebUIElement {
  @observable updateState: UpdateStatus = initialState.updateState;
  @observable updatePanelVisible = initialState.updatePanelVisible;
  @observable updateBusy = initialState.updateBusy;
  @observable updateButtonLabel = initialState.updateButtonLabel;
  @observable updateProgressText = initialState.updateProgressText;
  @observable updateReminderText = initialState.updateReminderText;
  @observable updateError = initialState.updateError;
  updateButton!: HTMLButtonElement;
  private generation = 0;
  private dismissedGeneration = -1;
  private closeChannel: Channel<null> | undefined;

  protected hydratedCallback(): void {
    this.closeChannel = new Channel<null>();
    this.closeChannel.onmessage = () => this.$emit('install-update', { reopen: false });
    void invoke('watch_update_close', { channel: this.closeChannel }).catch(() => undefined);
    void this.check(false);
  }

  private apply(status: UpdateStatus): void {
    this.updateState = status;
    this.updateButtonLabel = status.scheduled ? 'Update on exit' : status.version && !status.suppressed ? 'Update available' : 'Updates';
    this.updateProgressText = status.totalBytes
      ? `${Math.min(100, Math.floor(status.downloadedBytes / status.totalBytes * 100))}% downloaded`
      : `${(status.downloadedBytes / 1024 / 1024).toFixed(1)} MiB downloaded`;
    this.updateReminderText = status.remindAfterUnixMs > Date.now()
      ? `Automatic prompts paused until ${new Date(status.remindAfterUnixMs).toLocaleDateString()}.` : '';
  }

  togglePanel(): void { this.updatePanelVisible = !this.updatePanelVisible; }
  dismiss(): void { this.dismissedGeneration = this.generation; this.updatePanelVisible = false; this.updateButton.focus(); }

  async check(manual = true): Promise<void> {
    if (this.updateBusy) { if (manual) this.updatePanelVisible = true; return; }
    const generation = ++this.generation;
    this.updateBusy = true;
    this.updateError = '';
    if (manual) this.updatePanelVisible = true;
    try {
      const snapshot = await invoke<UpdateStatus>('update_status').catch(() => null);
      if (generation !== this.generation || !this.isConnected) return;
      if (snapshot) this.apply({...snapshot, phase: 'checking', message: 'Checking for a stable release…'});
      const status = await invoke<UpdateStatus>('check_updates', { manual });
      if (generation !== this.generation || !this.isConnected) return;
      this.apply(status);
      if (status.version && !status.suppressed && this.dismissedGeneration !== generation) this.updatePanelVisible = true;
    } catch (error: unknown) {
      const status = await invoke<UpdateStatus>('update_status').catch(() => null);
      if (generation === this.generation && this.isConnected) {
        if (status) this.apply(status);
        if (manual && describeError(error) !== status?.message) this.updateError = describeError(error);
      }
    } finally { if (generation === this.generation) this.updateBusy = false; }
  }

  async choose(afterSession: boolean): Promise<void> {
    if (this.updateBusy) return;
    const generation = ++this.generation;
    this.updateBusy = true;
    this.updateError = '';
    this.updateState = { ...this.updateState, phase: 'downloading' };
    const progress = new Channel<UpdateStatus>();
    progress.onmessage = status => { if (generation === this.generation && this.isConnected) this.apply(status); };
    try {
      const status = await invoke<UpdateStatus>('download_update', { afterSession, progress });
      if (generation !== this.generation || !this.isConnected) return;
      this.apply(status);
      if (!afterSession && status.phase === 'ready') this.$emit('install-update', { reopen: true });
    } catch (error: unknown) {
      if (generation === this.generation) {
        this.updateError = describeError(error);
        this.updateState = { ...this.updateState, phase: 'error' };
      }
    } finally { if (generation === this.generation) this.updateBusy = false; }
  }

  async cancel(): Promise<void> {
    ++this.generation;
    this.updateError = '';
    this.updateBusy = true;
    try { this.apply(await invoke<UpdateStatus>('cancel_update')); }
    catch (error: unknown) { this.updateError = describeError(error); }
    finally { this.updateBusy = false; }
  }

  async remind(): Promise<void> {
    if (this.updateBusy) return;
    this.updateBusy = true;
    this.updateError = '';
    try { this.apply(await invoke<UpdateStatus>('remind_update')); this.dismiss(); }
    catch (error: unknown) { this.updateError = describeError(error); }
    finally { this.updateBusy = false; }
  }

  async install(reopen: boolean): Promise<void> {
    ++this.generation;
    this.updateBusy = true;
    this.updateError = '';
    this.updatePanelVisible = true;
    this.updateState = { ...this.updateState, phase: 'installing', message: 'Stopping capture and restoring Windows settings before installation…' };
    try { await invoke('install_update', { reopen }); }
    catch (error: unknown) {
      this.updateError = describeError(error);
      this.apply(await invoke<UpdateStatus>('update_status').catch(() => ({ ...this.updateState, phase: 'error' })));
    } finally { this.updateBusy = false; }
  }

  installReady(): void { this.$emit('install-update', { reopen: true }); }
  disconnectedCallback(): void { ++this.generation; this.closeChannel = undefined; super.disconnectedCallback(); }
}

AppUpdates.define('app-updates');
