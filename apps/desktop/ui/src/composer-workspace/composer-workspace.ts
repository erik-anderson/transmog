import initialState from '../initial-state.json';
import { attr, observable } from '@microsoft/webui-framework';
import { invoke } from '@tauri-apps/api/core';
import { WorkspaceElement } from '../workspace-element.js';
import type { ComposerResult } from '../models.js';
import { describeError, parseHeaderLines } from '../utilities.js';

export class ComposerWorkspace extends WorkspaceElement {
  @attr view = 'traffic';
  @observable composerText = initialState.composerText;
  composerForm!: HTMLFormElement;
  async executeComposer(event: Event): Promise<void> {
    event.preventDefault();
    const data = new FormData(this.composerForm);
    try {
      const result = await invoke<ComposerResult>('execute_composer', {
        request: {
          method: String(data.get('method') ?? ''),
          url: String(data.get('url') ?? ''),
          headers: parseHeaderLines(String(data.get('headers') ?? '')),
          body: String(data.get('body') ?? ''),
          bodyIsHex: data.get('bodyHex') === 'on',
          acknowledgeNonIdempotent: data.get('nonIdempotent') === 'on',
          acknowledgeCredentials: data.get('credentials') === 'on',
        },
      });
      this.composerText = JSON.stringify(result, null, 2);
    } catch (error: unknown) {
      this.composerText = `Replay failed: ${describeError(error)}`;
    }
  }

}

ComposerWorkspace.define('composer-workspace');
