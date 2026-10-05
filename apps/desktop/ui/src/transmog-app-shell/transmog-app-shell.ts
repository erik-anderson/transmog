import { WebUIElement } from '@microsoft/webui-framework';
import { invoke } from '@tauri-apps/api/core';

type Lifecycle = 'stopped' | 'running' | 'stopping' | 'failed';

interface AppStatus {
  lifecycle: Lifecycle;
  listener: string | null;
  summary: string;
  hostRestorePending: boolean;
}

export class TransmogAppShell extends WebUIElement {
  statusLabel!: HTMLSpanElement;
  listenerValue!: HTMLElement;
  diagnostics!: HTMLOutputElement;

  async refreshStatus(): Promise<void> {
    try {
      const status = await invoke<AppStatus>('app_status');
      this.statusLabel.textContent = lifecycleLabel(status.lifecycle);
      this.listenerValue.textContent = status.listener ?? 'Not listening';
      this.diagnostics.textContent = status.hostRestorePending
        ? 'Host restoration is pending and must be retried before restart.'
        : status.summary;
      this.shadowRoot?.querySelector('.status')?.setAttribute('data-lifecycle', status.lifecycle);
    } catch (error: unknown) {
      this.diagnostics.textContent = `Status unavailable: ${describeError(error)}`;
    }
  }
}

TransmogAppShell.define('transmog-app-shell');

function lifecycleLabel(lifecycle: Lifecycle): string {
  switch (lifecycle) {
    case 'running': return 'Running';
    case 'stopping': return 'Stopping';
    case 'failed': return 'Needs attention';
    default: return 'Stopped';
  }
}

function describeError(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === 'string') return error;
  return 'unknown failure';
}
