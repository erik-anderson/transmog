import { WebUIElement } from '@microsoft/webui-framework';
import { Channel, invoke } from '@tauri-apps/api/core';

interface DeliveryProbe {
  transport: 'custom-protocol';
  bounded: boolean;
}

interface ProbeHint {
  revision: number;
  kind: 'refresh-available';
}

interface ProbeResult {
  acceptedLabel: string;
  rustCommandOk: boolean;
}

export class PhaseZeroProbe extends WebUIElement {
  resultOutput!: HTMLOutputElement;

  async runProbe(): Promise<void> {
    const button = this.shadowRoot?.querySelector('button');
    if (!(button instanceof HTMLButtonElement)) {
      this.showFailure('The hydrated button was unavailable.');
      return;
    }

    button.disabled = true;
    this.resultOutput.dataset.status = 'running';
    this.resultOutput.textContent = 'Checking the embedded origin…';

    try {
      const response = await fetch('/probe.json', {
        cache: 'no-store',
        credentials: 'same-origin',
        redirect: 'error'
      });
      if (!response.ok) {
        throw new Error(`custom protocol returned ${response.status}`);
      }
      const delivery = (await response.json()) as DeliveryProbe;
      const hints: ProbeHint[] = [];
      const onEvent = new Channel<ProbeHint>();
      onEvent.onmessage = (hint) => {
        if (hints.length < 1) {
          hints.push(hint);
        }
      };

      const result = await invoke<ProbeResult>('phase_zero_probe', {
        input: {
          label: 'WebView2',
          customProtocolOk:
            delivery.transport === 'custom-protocol' && delivery.bounded
        },
        onEvent
      });

      if (!result.rustCommandOk || hints.length !== 1) {
        throw new Error('native command or bounded notification proof failed');
      }
      this.resultOutput.dataset.status = 'passed';
      this.resultOutput.textContent =
        `Passed · fetch=${delivery.transport} · command=${result.acceptedLabel} · hint=${hints[0]?.revision ?? 0}`;
    } catch (error: unknown) {
      this.showFailure(describeError(error));
    } finally {
      button.disabled = false;
    }
  }

  private showFailure(message: string): void {
    this.resultOutput.dataset.status = 'failed';
    this.resultOutput.textContent = `Failed · ${message}`;
  }
}

PhaseZeroProbe.define('phase-zero-probe');

function describeError(error: unknown): string {
  if (error instanceof Error) {
    return error.message;
  }
  if (typeof error === 'string') {
    return error;
  }
  try {
    return JSON.stringify(error);
  } catch {
    return 'unknown failure';
  }
}
