import initialState from '../initial-state.json';
import { attr, observable } from '@microsoft/webui-framework';
import { invoke } from '@tauri-apps/api/core';
import { WorkspaceElement } from '../workspace-element.js';
import type { CaptureReadModel } from '../models.js';
import { describeError } from '../utilities.js';

export class CaptureWorkspace extends WorkspaceElement {
  @attr view = 'traffic';
  @observable captureText = initialState.captureText;
  captureForm!: HTMLFormElement;
  artifactForm!: HTMLFormElement;
  async startCapture(event: Event): Promise<void> {
    event.preventDefault();
    const data = new FormData(this.captureForm);
    try {
      const status = await invoke<CaptureReadModel>('start_capture', {
        request: {
          path: String(data.get('capturePath') ?? ''),
          maxFileBytes: Number(data.get('quota')),
          retainBodySamples: data.get('bodies') === 'on',
        },
      });
      this.captureText = JSON.stringify(status, null, 2);
    } catch (error: unknown) {
      this.captureText = `Capture start failed: ${describeError(error)}`;
    }
  }

  async stopCapture(): Promise<void> {
    try {
      const status = await invoke<CaptureReadModel>('stop_capture');
      this.captureText = JSON.stringify(status, null, 2);
    } catch (error: unknown) {
      this.captureText = `Capture finalization failed: ${describeError(error)}`;
    }
  }

  async refreshCapture(): Promise<void> {
    this.captureText = JSON.stringify(await invoke<CaptureReadModel>('capture_status'), null, 2);
  }

  async inspectCapture(event: Event): Promise<void> {
    event.preventDefault();
    const data = new FormData(this.artifactForm);
    try {
      const summary = await invoke<Record<string, unknown>>('import_capture', {
        request: { path: String(data.get('source') ?? ''), maxFileBytes: 4_294_967_296 },
      });
      this.captureText = JSON.stringify(summary, null, 2);
    } catch (error: unknown) {
      this.captureText = `Import failed: ${describeError(error)}`;
    }
  }

  async exportCapture(): Promise<void> {
    const data = new FormData(this.artifactForm);
    try {
      const report = await invoke<Record<string, unknown>>('export_capture', {
        request: {
          source: String(data.get('source') ?? ''),
          destination: String(data.get('destination') ?? ''),
          format: String(data.get('format') ?? 'json-lines'),
          maxSourceBytes: 4_294_967_296,
        },
      });
      this.captureText = JSON.stringify(report, null, 2);
    } catch (error: unknown) {
      this.captureText = `Export failed: ${describeError(error)}`;
    }
  }

}

CaptureWorkspace.define('capture-workspace');
