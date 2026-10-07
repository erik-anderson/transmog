import initialState from '../initial-state.json';
import { attr, observable } from '@microsoft/webui-framework';
import { invoke } from '@tauri-apps/api/core';
import { WorkspaceElement } from '../workspace-element.js';
import type { BodyInspection, ComposerResult, SessionDetail } from '../models.js';
import { describeError, parseHeaderLines } from '../utilities.js';

export class ComposerWorkspace extends WorkspaceElement {
  @attr view = 'traffic';
  @observable composerText = initialState.composerText;
  composerForm!: HTMLFormElement;
  private draftGeneration = 0;
  async populateRequest(detail: SessionDetail): Promise<void> {
    const generation = ++this.draftGeneration;
    const request = detail.requests.find((head) => head.boundary === 'client-request') ?? detail.requests[0];
    if (!request) { this.composerText = 'The captured request is no longer available.'; return; }
    const field = (name: string) => this.composerForm.elements.namedItem(name) as HTMLInputElement;
    field('method').value = request.method ?? 'GET';
    field('url').value = request.target ?? '';
    field('headers').value = request.headers.filter((header) => !header.sensitive && !['host','content-length','transfer-encoding','connection','content-encoding'].includes(header.name.toLowerCase())).map((header) => header.name+': '+header.value).join('\n');
    field('body').value = '';
    field('bodyHex').checked = false;
    field('nonIdempotent').checked = false;
    field('credentials').checked = false;
    this.composerText = 'Captured request loaded. Review the method, URL, headers, and body before executing replay. Credential headers were omitted.';
    const body = detail.storedBodies.find((body) => body.boundary === request.boundary);
    if (body?.observedBytes) {
      if (body.availability !== 'complete' || body.retainedBytes > 1024*1024) {
        this.composerText += ' The complete request body is unavailable; supply it before replaying.';
      } else {
        try {
          const inspection = await invoke<BodyInspection>('inspect_body',{request:{sessionId:detail.id,boundary:request.boundary,representation:'bytes',decodeContent:true,offset:0,maxBytes:1024*1024}});
          if (generation !== this.draftGeneration || !this.isConnected) return;
          if (inspection.truncated || inspection.nextOffset !== null) throw new Error('request body exceeds the replay limit');
          if (inspection.representation !== 'bytes') throw new Error('request bytes are unavailable');
          field('body').value = inspection.display.split('\n').filter(Boolean).map((line) => line.slice(10).split('|')[0]!.trim().replace(/\s+/g,'')).join('');
          field('bodyHex').checked = true;
        } catch (error: unknown) { this.composerText += ' Request body could not be loaded: '+describeError(error); }
      }
    }
    field('url').focus();
  }
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
