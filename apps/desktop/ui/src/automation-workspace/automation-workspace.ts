import initialState from '../initial-state.json';
import { attr, observable } from '@microsoft/webui-framework';
import '../script-editor/script-editor.js';
import { invoke } from '@tauri-apps/api/core';
import { WorkspaceElement } from '../workspace-element.js';
import type { SessionDetail, BodyInspection, AutomationCandidate, AutomationRule, AutomationStatus, ResponseAsset, AutoResponseSource, SelectedResponse } from '../models.js';
import { describeError, optionalText, parseHeaderLines, encodeHeaders, shortUrl, editableCharacterEncoding, encodeEditedText, loadSessionDetail, clientResponseSource, autoResponseUnavailableReason } from '../utilities.js';
import { formatBytes } from '../table-model.js';

const AUTORESPONSE_PRIORITY_BASE = -1_000_000;
const MAX_AUTORESPONSE_EDIT_BYTES = 16 * 1024 * 1024;

export class AutomationWorkspace extends WorkspaceElement {
  @attr view = 'traffic';
  @attr theme = 'system';
  @observable selection: SelectedResponse | null = initialState.selection;
  @observable automationText = initialState.automationText;
  @observable automationKind = initialState.automationKind;
  @observable rules: Array<AutomationRule & {order: number; name: string; criteria: string; state: string; toggleLabel: string; first: boolean; last: boolean}> = initialState.rules;
  @observable editorHidden = initialState.editorHidden;
  @observable editorTitle = initialState.editorTitle;
  @observable sourceText = initialState.sourceText;
  @observable sourceSessionId = initialState.sourceSessionId;
  @observable sourceLabel = initialState.sourceLabel;
  @observable sourceAvailable = initialState.sourceAvailable;
  @observable sourceAvailabilityText = initialState.sourceAvailabilityText;
  @observable bodyStatusText = initialState.bodyStatusText;
  @observable bodyHidden = initialState.bodyHidden;
  @observable encodingOptionsHidden = initialState.encodingOptionsHidden;
  @observable savingAutoResponse = initialState.savingAutoResponse;
  @observable changingAutoResponse = initialState.changingAutoResponse;
  @observable autoResponseError = initialState.autoResponseError;
  @observable requestHeadersError = initialState.requestHeadersError;
  @observable responseHeadersError = initialState.responseHeadersError;
  @observable undoRemoveText = initialState.undoRemoveText;
  @observable responseReadOnly = initialState.responseReadOnly;
  @observable responseHeadersHidden = initialState.responseHeadersHidden;
  @observable capturedOptionsHidden = initialState.capturedOptionsHidden;
  @observable preserveEncodingDisabled = initialState.preserveEncodingDisabled;
  @observable bodyReadOnly = initialState.bodyReadOnly;
  @observable requestHeadersHidden = initialState.requestHeadersHidden;
  @observable editBodyDisabled = initialState.editBodyDisabled;
  @observable responseFieldsHidden = initialState.responseFieldsHidden;
  @observable highlightedRuleId: string | null = initialState.highlightedRuleId;
  automationForm!: HTMLFormElement;
  autoResponseForm!: HTMLFormElement;
  autoResponseList!: HTMLDivElement;
  autoResponseMethod!: HTMLSelectElement;
  autoResponseStatus!: HTMLInputElement;
  autoResponseMediaType!: HTMLInputElement;
  autoResponseBody!: HTMLTextAreaElement;
  autoResponseEditBody!: HTMLInputElement;
  autoResponsePreserveEncoding!: HTMLInputElement;
  newResponseButton!: HTMLButtonElement;
  undoRemoveButton!: HTMLButtonElement;
  private automationStatus: AutomationStatus | null = null;
  private autoResponseSourceState: AutoResponseSource | null = null;
  private editingAutoResponseId: string | null = null;
  private draggedRuleId: string | null = null;
  private editorRevision = 0;
  private sessionLoadRevision = 0;
  private editorOpener: HTMLElement | null = null;
  private sourceQuery: Promise<void> | null = null;
  private removedRule: {rule: AutomationRule; index: number} | null = null;

  protected hydratedCallback(): void { void this.refreshAutomation(); }
  viewChanged(): void { if (this.view === 'automation') void this.refreshSourceAvailability(); }
  private prepareEditor(): void {
    this.editorRevision++;
    this.sessionLoadRevision++;
    if (this.editorHidden) this.editorOpener = (this.getRootNode() as ShadowRoot).activeElement as HTMLElement | null;
    this.autoResponseForm.reset();
    for (const field of ['requestHeaders','responseHeaders']) (this.autoResponseForm.elements.namedItem(field) as HTMLTextAreaElement).setCustomValidity('');
    this.sourceSessionId = ''; this.sourceLabel = ''; this.sourceAvailable = false; this.sourceAvailabilityText = '';
    this.bodyStatusText = ''; this.bodyHidden = false; this.encodingOptionsHidden = true;
    this.autoResponseError = ''; this.requestHeadersError = ''; this.responseHeadersError = '';
    this.sourceQuery = null;
  }
  private openEditor(): void {
    this.editorHidden = false; this.$flushUpdates();
    this.autoResponseForm.scrollIntoView({block:'start'});
    (this.autoResponseForm.elements.namedItem('name') as HTMLInputElement).focus({preventScroll:true});
  }
  async refreshSourceAvailability(): Promise<void> {
    const id = this.sourceSessionId;
    if (!id || this.editorHidden || !this.isConnected) return;
    if (this.sourceQuery) return this.sourceQuery;
    const revision = this.editorRevision;
    const pending = (async () => {
      try {
        const detail = await invoke<SessionDetail>('session_detail',{id});
        if (revision !== this.editorRevision || !this.isConnected) return;
        const request = detail.requests.find((head) => head.boundary === 'client-request');
        this.sourceLabel = request?.target ? `${request.method ?? 'Request'} ${shortUrl(request.target)}` : 'Original captured request';
        this.sourceAvailable = true; this.sourceAvailabilityText = '';
      } catch (error:unknown) {
        if (revision !== this.editorRevision || !this.isConnected) return;
        this.sourceAvailable = false;
        this.sourceAvailabilityText = /evict|unavailable|not found/i.test(describeError(error))
          ? 'Source no longer in Traffic.' : 'Source could not be checked. Try again when Traffic is available.';
      }
    })();
    this.sourceQuery = pending;
    try { await pending; } finally { if (this.sourceQuery === pending) this.sourceQuery = null; }
  }
  async showSourceTraffic(event:Event): Promise<void> {
    event.preventDefault();
    const id = this.sourceSessionId;
    const revision = this.editorRevision;
    await this.refreshSourceAvailability();
    if (this.sourceAvailable && revision === this.editorRevision && id === this.sourceSessionId) this.$emit('source-traffic-request',id);
  }
  clearHeaderError(field:'requestHeaders' | 'responseHeaders'): void {
    (this.autoResponseForm.elements.namedItem(field) as HTMLTextAreaElement).setCustomValidity('');
    if (field === 'requestHeaders') this.requestHeadersError = '';
    else this.responseHeadersError = '';
    this.autoResponseError = '';
  }
  private parseEditorHeaders(field:'requestHeaders' | 'responseHeaders',data:FormData): ReturnType<typeof parseHeaderLines> {
    try { return parseHeaderLines(String(data.get(field) ?? '')); }
    catch (error:unknown) {
      const message = describeError(error);
      if (field === 'requestHeaders') this.requestHeadersError = message;
      else this.responseHeadersError = message;
      const input = this.autoResponseForm.elements.namedItem(field) as HTMLTextAreaElement;
      input.setCustomValidity(message); input.focus();
      throw error;
    }
  }
  private renderAutoResponseRules(status: AutomationStatus): void {
    const rules = this.autoResponseRules(status);
    this.rules = rules.map((rule,index) => {
      const headers = rule.matcher.requestHeaders.length;
      const enabled = rule.enabled ?? true;
      return { ...rule, enabled, order: index + 1, name: rule.displayName ?? 'Auto-response',
        criteria: `${rule.matcher.method ?? 'ANY'} ${rule.matcher.url?.value ?? 'any URL'}${headers === 0 ? '' : ` · ${headers} exact header match${headers === 1 ? '' : 'es'}`} · saved response`,
        state: enabled ? 'Enabled' : 'Disabled', toggleLabel: enabled ? 'Disable' : 'Enable', first: index === 0, last: index === rules.length - 1 };
    });
  }
  allowSessionDrop(event: DragEvent): void { if (!this.savingAutoResponse && event.dataTransfer?.types.includes('application/x-transmog-session')) event.preventDefault(); }
  dropSession(event: DragEvent): void {
    event.preventDefault();
    const sessionId = event.dataTransfer?.getData('application/x-transmog-session');
    if (sessionId) void this.beginAutoResponseFromSessionId(sessionId);
  }
  dragRule(id: string,event: DragEvent): void {
    if (this.savingAutoResponse || this.changingAutoResponse) { event.preventDefault(); return; }
    this.draggedRuleId = id;
    event.dataTransfer?.setData('application/x-transmog-autoresponse-rule',id);
    if (event.dataTransfer) event.dataTransfer.effectAllowed = 'move';
  }
  allowRuleDrop(id: string,event: DragEvent): void { if (!this.savingAutoResponse && !this.changingAutoResponse && this.draggedRuleId !== null && this.draggedRuleId !== id) event.preventDefault(); }
  finishRuleDrag(): void { this.draggedRuleId = null; }
  async dropRule(id: string,event: DragEvent): Promise<void> {
    event.preventDefault();
    if (this.savingAutoResponse || this.changingAutoResponse) return;
    const dragged = event.dataTransfer?.getData('application/x-transmog-autoresponse-rule') ?? this.draggedRuleId;
    this.draggedRuleId = null;
    const current = this.automationStatus;
    if (!dragged || dragged === id || current === null) return;
    const ordered = this.autoResponseRules(current);
    const from = ordered.findIndex((rule) => rule.id === dragged);
    const to = ordered.findIndex((rule) => rule.id === id);
    if (from < 0 || to < 0) return;
    const [moved] = ordered.splice(from,1);
    if (moved) ordered.splice(to,0,moved);
    this.changingAutoResponse = true;
    try { this.renderAutomation(await this.activateAutoResponseOrder(current,ordered)); this.focusRule(dragged); }
    catch (error: unknown) { this.automationText = `Auto-response reorder failed: ${describeError(error)}`; this.automationKind = 'error'; }
    finally { this.changingAutoResponse = false; }
  }
  showMatchedRule(id: string): void {
    this.highlightedRuleId = id;
    this.$flushUpdates();
    const card = this.getRootNode() as ShadowRoot;
    const element = card.getElementById(`rule-${id}`);
    if (!element) { this.showNotice('Historical rule','The response records which rule won, but that rule is no longer in the active list.',null,null); return; }
    element.scrollIntoView({block:'center'});
    element.focus({preventScroll:true});
  }
  async activateUserAgentRule(event: Event): Promise<void> {
    event.preventDefault();
    const data = new FormData(this.automationForm);
    const encode = (value: string): number[] => Array.from(new TextEncoder().encode(value));
    const host = optionalText(data.get('host'));
    const path = optionalText(data.get('path'));
    try {
      const current = await invoke<AutomationStatus>('automation_status');
      const previous = current.rules.find((rule) => rule.id === 'desktop-user-agent');
      const rule = {
        id: 'desktop-user-agent',
        displayName: 'User-Agent override',
        enabled: true,
        revision: (previous?.revision ?? 0) + 1,
        priority: 0,
        matcher: {
          method: null,
          url: null,
          scheme: null,
          host,
          port: null,
          pathPrefix: path,
          query: null,
          requestHeaders: [],
          responseHeaders: [],
          responseStatus: null,
          responseStatusClass: null,
        },
        request: {
          headers: [{
            operation: 'set',
            field: { name: encode('User-Agent'), value: encode(String(data.get('userAgent') ?? '')) },
          }],
          replaceBody: null,
          discardBody: false,
          abortReason: null,
          responseAsset: null,
          allowNonIdempotentBodyReplacement: false,
        },
        response: { headers: [], replaceBody: null, discardBody: false, abortReason: null },
      };
      const rules = current.rules.filter((candidate) => candidate.id !== rule.id);
      rules.push(rule);
      const status = await this.activateRuleSet(rules, current.generation);
      this.renderAutomation(status);
      this.diagnostic = 'User-Agent override enabled for new matching requests.';
    } catch (error: unknown) {
      this.automationText = `User-Agent override could not be enabled: ${describeError(error)}`;
      this.automationKind = 'error';
    }
  }

  async refreshAutomation(): Promise<void> {
    try {
      this.renderAutomation(await invoke<AutomationStatus>('automation_status'));
    } catch (error: unknown) {
      this.automationText = `Automation query failed: ${describeError(error)}`;
    }
  }

  async beginAutoResponseFromSelected(): Promise<void> {
    if (this.savingAutoResponse) return;
    if (this.selection === null) {
      this.showNotice('Select a completed response', 'Choose a request in Traffic whose client response body was retained, then try again.', null, null);
      return;
    }
    await this.populateCapturedAutoResponse(this.selection.sessionId, this.selection.detail);
  }

  async beginAutoResponseFromSessionId(sessionId: string): Promise<void> {
    if (this.savingAutoResponse) return;
    const revision = this.editorRevision;
    const loadRevision = ++this.sessionLoadRevision;
    try {
      const detail = await loadSessionDetail(sessionId, true);
      if (revision !== this.editorRevision || loadRevision !== this.sessionLoadRevision || !this.isConnected || this.savingAutoResponse) return;
      if (this.selection?.sessionId === sessionId) this.selection = { ...this.selection, detail };
      await this.populateCapturedAutoResponse(sessionId, detail);
    } catch (error: unknown) {
      if (revision === this.editorRevision && loadRevision === this.sessionLoadRevision) this.showNotice('Response cannot be reused', describeError(error), null, null);
    }
  }

  async populateCapturedAutoResponse(sessionId: string, detail: SessionDetail): Promise<void> {
    if (this.savingAutoResponse) return;
    const source = clientResponseSource(detail);
    if (source === null || source.request.method === null || source.request.target === null || source.response.status === null) {
      this.showNotice(
        'Response cannot be reused',
        autoResponseUnavailableReason(detail),
        null,
        null,
      );
      return;
    }
    this.editingAutoResponseId = null;
    this.prepareEditor();
    const elements = this.autoResponseForm.elements;
    (elements.namedItem('name') as HTMLInputElement).value = `${source.request.method} ${shortUrl(source.request.target)}`;
    this.autoResponseMethod.value = source.request.method.toUpperCase();
    (elements.namedItem('url') as HTMLInputElement).value = source.request.target;
    this.autoResponseStatus.value = String(source.response.status);
    this.responseReadOnly = true;
    this.autoResponseMediaType.value = source.body.mediaType ?? '';
    this.responseFieldsHidden = false;
    (elements.namedItem('responseHeaders') as HTMLTextAreaElement).value = '';
    this.responseHeadersHidden = true;
    this.capturedOptionsHidden = false;
    this.autoResponseEditBody.checked = false;
    this.autoResponsePreserveEncoding.checked = true;
    this.preserveEncodingDisabled = source.body.contentCodings.length === 0;
    this.encodingOptionsHidden = source.body.contentCodings.length === 0;
    this.autoResponseBody.value = '';
    this.bodyReadOnly = true;
    this.editorTitle = 'New rule from captured response';
    this.sourceText = `${source.response.status} · ${formatBytes(source.body.retainedBytes)} · ${source.body.mediaType ?? 'Unknown content type'}${source.body.contentCodings.length === 0 ? '' : ` · ${source.body.contentCodings.join(', ')}`}`;
    this.sourceSessionId = sessionId;
    this.sourceLabel = `${source.request.method} ${shortUrl(source.request.target)}`;
    this.sourceAvailabilityText = 'Checking source in Traffic…';
    const captured: AutoResponseSource = {
      kind: 'captured',
      sessionId,
      boundary: 'client-response',
      contentCodings: source.body.contentCodings,
      bodyEditable: false,
      textEncoding: editableCharacterEncoding(source.body.charset),
    };
    this.autoResponseSourceState = captured;
    const revision = this.editorRevision;
    this.updateAutoResponseHeaderFilterState();
    this.updateAutoResponseBodyEditState();
    this.openEditor();
    void this.refreshSourceAvailability();

    if (source.body.retainedBytes === 0) {
      captured.bodyEditable = captured.textEncoding !== null;
      this.bodyStatusText = 'Empty response body. Enable editing to replace it.';
      this.updateAutoResponseBodyEditState();
      return;
    }
    if (source.body.retainedBytes > MAX_AUTORESPONSE_EDIT_BYTES) {
      this.bodyStatusText = 'This body is too large to edit here. The complete saved response can still be replayed.';
      return;
    }
    this.bodyStatusText = 'Loading response body…';
    try {
      const inspection = await invoke<BodyInspection>('inspect_body', {
        request: {
          sessionId,
          boundary: 'client-response',
          representation: 'original-text',
          decodeContent: true,
          offset: 0,
          maxBytes: MAX_AUTORESPONSE_EDIT_BYTES,
        },
      });
      if (revision !== this.editorRevision || this.autoResponseSourceState !== captured || !this.isConnected) return;
      if (!inspection.truncated && inspection.representation === 'original-text' && inspection.textEncoding !== null) {
        this.autoResponseBody.value = inspection.display;
        captured.bodyEditable = true;
        captured.textEncoding = inspection.textEncoding;
        this.bodyStatusText = `Read-only preview. Enable editing to replace the body; text uses ${inspection.textEncoding}.`;
        this.updateAutoResponseBodyEditState();
      } else {
        this.bodyStatusText = 'A complete text preview is unavailable. The saved response can still be replayed.';
      }
    } catch {
      if (revision === this.editorRevision && this.autoResponseSourceState === captured) this.bodyStatusText = 'A text preview is unavailable. The saved response can still be replayed.';
    }
  }

  beginScratchAutoResponse(): void {
    if (this.savingAutoResponse) return;
    this.prepareEditor();
    this.editingAutoResponseId = null;
    this.autoResponseSourceState = { kind: 'scratch' };
    this.editorTitle = 'New response from scratch';
    this.sourceText = 'Custom response. Matching requests receive the status, headers, and body below.';
    this.responseReadOnly = false;
    this.responseFieldsHidden = false;
    this.responseHeadersHidden = false;
    this.capturedOptionsHidden = true;
    this.bodyReadOnly = false;
    this.autoResponseBody.value = '';
    this.updateAutoResponseHeaderFilterState();
    this.openEditor();
  }

  cancelAutoResponseEdit(): void {
    if (this.savingAutoResponse) return;
    this.editorRevision++;
    this.editorHidden = true;
    this.autoResponseSourceState = null;
    this.editingAutoResponseId = null;
    this.$flushUpdates();
    if (this.editorOpener?.isConnected && this.editorOpener.getClientRects().length) this.editorOpener.focus();
    else this.newResponseButton.focus();
  }

  updateAutoResponseHeaderFilterState(): void {
    const enabled = this.autoResponseMethod.value.toUpperCase() === 'POST';
    this.requestHeadersHidden = !enabled;
    if (!enabled) this.clearHeaderError('requestHeaders');
  }

  updateAutoResponseBodyEditState(): void {
    const source = this.autoResponseSourceState;
    const editable = source?.kind === 'captured' && source.bodyEditable;
    if (!editable) this.autoResponseEditBody.checked = false;
    this.editBodyDisabled = !editable;
    this.bodyReadOnly = !this.autoResponseEditBody.checked;
    this.preserveEncodingDisabled = !this.autoResponseEditBody.checked
      || source?.kind !== 'captured'
      || source.contentCodings.length === 0;
    if (editable) this.bodyStatusText = this.autoResponseEditBody.checked
      ? `Edited text will replace the captured body using ${source.textEncoding}.`
      : `Read-only preview. Enable editing to replace the body; text uses ${source.textEncoding}.`;
  }

  async saveAutoResponse(event: Event): Promise<void> {
    event.preventDefault();
    if (this.savingAutoResponse || this.changingAutoResponse) return;
    const data = new FormData(this.autoResponseForm);
    const source = this.autoResponseSourceState;
    if (source === null) return;
    const editingId = this.editingAutoResponseId;
    const replaceBody = this.autoResponseEditBody.checked;
    const preserveEncoding = this.autoResponsePreserveEncoding.checked;
    this.savingAutoResponse = true;
    this.autoResponseError = ''; this.requestHeadersError = ''; this.responseHeadersError = '';
    this.automationText = 'Validating and saving the auto-response…';
    this.automationKind = 'progress';
    try {
      const displayName = String(data.get('name') ?? '').trim();
      if (!displayName) throw new Error('Enter a rule name.');
      const method = String(data.get('method') ?? '').toUpperCase();
      const requestHeaders = method === 'POST'
        ? this.parseEditorHeaders('requestHeaders',data).map((header) => ({
            name: header.name,
            condition: { kind: 'exact', value: Array.from(new TextEncoder().encode(header.value)) },
          }))
        : [];
      const responseHeaders = source.kind === 'scratch' ? encodeHeaders(this.parseEditorHeaders('responseHeaders',data)) : [];
      const editedBody = source.kind === 'captured' && replaceBody
        ? encodeEditedText(String(data.get('body') ?? ''), source.textEncoding) : null;
      const current = await invoke<AutomationStatus>('automation_status');
      const existing = editingId === null ? undefined : current.rules.find((candidate) => candidate.id === editingId);
      if (editingId !== null && existing === undefined) throw new Error('This rule was removed. Cancel editing and create a new rule.');
      let assetReference: string;
      if (source.kind === 'existing') {
        assetReference = source.assetReference;
      } else {
        const suffix = crypto.randomUUID().replaceAll('-', '');
        const assetId = `autoresponse-${suffix}`;
        const asset = source.kind === 'captured'
          ? await invoke<ResponseAsset>('create_response_asset_from_session', {
              input: {
                id: assetId,
                revision: 1,
                exchangeId: source.sessionId,
                boundary: source.boundary,
                decodedBody: editedBody,
                preserveContentEncoding: preserveEncoding,
              },
            })
          : await invoke<ResponseAsset>('create_response_asset', {
              input: {
                id: assetId,
                revision: 1,
                status: Number(data.get('status') ?? 200),
                headers: responseHeaders,
                body: Array.from(new TextEncoder().encode(String(data.get('body') ?? ''))),
                mediaType: optionalText(data.get('mediaType')),
              },
            });
        assetReference = `${asset.id}@${asset.revision}`;
      }

      const id = existing?.id ?? `autoresponse-rule-${crypto.randomUUID().replaceAll('-', '')}`;
      const rule: AutomationRule = {
        id,
        displayName,
        enabled: existing?.enabled ?? true,
        revision: (existing?.revision ?? 0) + 1,
        priority: existing?.priority ?? AUTORESPONSE_PRIORITY_BASE,
        matcher: {
          method,
          url: { kind: 'exact', value: String(data.get('url') ?? '').trim() },
          scheme: null,
          host: null,
          port: null,
          pathPrefix: null,
          query: null,
          requestHeaders,
          responseHeaders: [],
          responseStatus: null,
          responseStatusClass: null,
        },
        request: {
          headers: [], replaceBody: null, discardBody: false, abortReason: null,
          responseAsset: assetReference, allowNonIdempotentBodyReplacement: false,
        },
        response: { headers: [], replaceBody: null, discardBody: false, abortReason: null },
      };
      const ordered = this.autoResponseRules(current).filter((candidate) => candidate.id !== id);
      if (existing === undefined) ordered.unshift(rule);
      else ordered.splice(Math.max(0, this.autoResponseRules(current).findIndex((candidate) => candidate.id === id)), 0, rule);
      const status = await this.activateAutoResponseOrder(current, ordered);
      this.renderAutomation(status);
      this.savingAutoResponse = false;
      this.cancelAutoResponseEdit();
      this.automationText = `Saved “${rule.displayName}”${existing === undefined ? ' at the top' : ''}. The first enabled matching rule will win.`;
      this.automationKind = 'success';
      this.focusRule(id);
    } catch (error: unknown) {
      this.automationText = `Auto-response could not be saved: ${describeError(error)}`;
      this.automationKind = 'error';
      this.autoResponseError = describeError(error);
    } finally {
      this.savingAutoResponse = false;
      this.$flushUpdates();
      if (this.requestHeadersError) (this.autoResponseForm.elements.namedItem('requestHeaders') as HTMLTextAreaElement).focus();
      else if (this.responseHeadersError) (this.autoResponseForm.elements.namedItem('responseHeaders') as HTMLTextAreaElement).focus();
    }
  }

  private autoResponseRules(status: AutomationStatus): AutomationRule[] {
    return status.rules
      .filter((rule) => rule.request.responseAsset !== null)
      .sort((left, right) => left.priority - right.priority || left.id.localeCompare(right.id));
  }

  private async activateAutoResponseOrder(current: AutomationStatus, ordered: AutomationRule[]): Promise<AutomationStatus> {
    const normalized = ordered.map((rule, index) => {
      const priority = AUTORESPONSE_PRIORITY_BASE + index;
      return rule.priority === priority ? rule : { ...rule, revision: rule.revision + 1, priority };
    });
    const ids = new Set(normalized.map((rule) => rule.id));
    const other = current.rules.filter((rule) => rule.request.responseAsset === null && !ids.has(rule.id));
    return this.activateRuleSet([...normalized, ...other], current.generation);
  }

  async mutateAutoResponse(ruleId: string, mutation: 'toggle' | 'remove' | 'up' | 'down'): Promise<void> {
    if (this.savingAutoResponse || this.changingAutoResponse) return;
    this.changingAutoResponse = true;
    try {
      const current = await invoke<AutomationStatus>('automation_status');
      const ordered = this.autoResponseRules(current);
      const index = ordered.findIndex((rule) => rule.id === ruleId);
      if (index < 0) return;
      const original = ordered[index]!;
      if (mutation === 'remove') ordered.splice(index, 1);
      else if (mutation === 'toggle') {
        const rule = ordered[index];
        if (rule !== undefined) ordered[index] = { ...rule, enabled: !(rule.enabled ?? true), revision: rule.revision + 1 };
      } else {
        const destination = mutation === 'up' ? index - 1 : index + 1;
        if (destination < 0 || destination >= ordered.length) return;
        [ordered[index], ordered[destination]] = [ordered[destination]!, ordered[index]!];
      }
      this.renderAutomation(await this.activateAutoResponseOrder(current, ordered));
      if (mutation === 'remove') {
        this.removedRule = {rule:original,index};
        this.undoRemoveText = `Removed “${original.displayName ?? 'Auto-response'}”.`;
        this.changingAutoResponse = false;
        this.$flushUpdates(); this.undoRemoveButton.focus();
      } else this.focusRule(ruleId);
    } catch (error: unknown) {
      this.automationText = `Auto-response change failed: ${describeError(error)}`;
      this.automationKind = 'error';
    } finally { this.changingAutoResponse = false; }
  }

  private focusRule(id:string): void {
    this.$flushUpdates();
    const element = (this.getRootNode() as ShadowRoot).getElementById('rule-'+id);
    element?.scrollIntoView({block:'nearest'}); element?.focus({preventScroll:true});
  }

  async undoRemoveAutoResponse(): Promise<void> {
    const removed = this.removedRule;
    if (!removed || this.savingAutoResponse || this.changingAutoResponse) return;
    this.changingAutoResponse = true;
    try {
      const current = await invoke<AutomationStatus>('automation_status');
      const ordered = this.autoResponseRules(current);
      if (!ordered.some((rule) => rule.id === removed.rule.id)) {
        ordered.splice(Math.min(removed.index,ordered.length),0,{...removed.rule,revision:removed.rule.revision+1});
        this.renderAutomation(await this.activateAutoResponseOrder(current,ordered));
      } else this.renderAutomation(current);
      this.removedRule = null; this.undoRemoveText = ''; this.focusRule(removed.rule.id);
    } catch (error:unknown) {
      this.automationText = 'Rule could not be restored: '+describeError(error); this.automationKind = 'error';
    } finally { this.changingAutoResponse = false; }
  }

  async editAutoResponse(rule: AutomationRule): Promise<void> {
    if (this.savingAutoResponse) return;
    this.prepareEditor();
    const revision = this.editorRevision;
    const url = rule.matcher.url?.kind === 'exact' ? rule.matcher.url.value : '';
    this.editingAutoResponseId = rule.id;
    this.autoResponseSourceState = { kind: 'existing', assetReference: rule.request.responseAsset ?? '' };
    const elements = this.autoResponseForm.elements;
    (elements.namedItem('name') as HTMLInputElement).value = rule.displayName ?? 'Auto-response';
    this.autoResponseMethod.value = rule.matcher.method ?? 'GET';
    (elements.namedItem('url') as HTMLInputElement).value = url;
    (elements.namedItem('requestHeaders') as HTMLTextAreaElement).value = rule.matcher.requestHeaders
      .filter((header) => header.condition.kind === 'exact')
      .map((header) => `${header.name}: ${new TextDecoder().decode(new Uint8Array(header.condition.value ?? []))}`)
      .join('\n');
    this.editorTitle = `Edit “${rule.displayName ?? 'Auto-response'}”`;
    this.sourceText = 'Loading saved response…';
    this.responseReadOnly = true;
    this.responseFieldsHidden = true;
    this.responseHeadersHidden = true;
    this.capturedOptionsHidden = true;
    this.bodyHidden = true;
    this.autoResponseBody.value = '';
    this.bodyReadOnly = true;
    this.updateAutoResponseHeaderFilterState();
    this.openEditor();
    try {
      const assets = await invoke<ResponseAsset[]>('response_assets');
      if (revision !== this.editorRevision || !this.isConnected) return;
      const asset = assets.find((candidate) => `${candidate.id}@${candidate.revision}` === rule.request.responseAsset);
      if (!asset) { this.sourceText = 'Saved response metadata is unavailable.'; return; }
      this.sourceText = `${asset.status} · ${formatBytes(asset.bodyBytes)} · ${asset.mediaType ?? 'Unknown content type'} · Saved response`;
      this.autoResponseStatus.value = String(asset.status);
      this.autoResponseMediaType.value = asset.mediaType ?? '';
      this.responseFieldsHidden = false;
      if (asset.provenance.kind === 'session') {
        this.sourceSessionId = asset.provenance.exchange_id;
        this.sourceLabel = 'Original captured request';
        this.sourceAvailabilityText = 'Checking source in Traffic…';
        await this.refreshSourceAvailability();
      }
    } catch (error:unknown) {
      if (revision === this.editorRevision) this.sourceText = 'Saved response metadata could not be loaded: '+describeError(error);
    }
  }

  async disableBuiltInAutomation(id: string): Promise<void> {
    try {
      const current = await invoke<AutomationStatus>('automation_status');
      const rules = current.rules.filter((rule) => rule.id !== id);
      if (rules.length === current.rules.length) {
        this.renderAutomation(current);
        return;
      }
      const status = await this.activateRuleSet(rules, current.generation);
      this.renderAutomation(status);
      this.diagnostic = 'Traffic action disabled for new requests.';
    } catch (error: unknown) {
      this.automationText = `Traffic action could not be disabled: ${describeError(error)}`;
      this.automationKind = 'error';
    }
  }

  private async activateRuleSet(
    rules: AutomationStatus['rules'],
    generation: number,
  ): Promise<AutomationStatus> {
    const candidate = await invoke<AutomationCandidate>('validate_automation', {
      document: { schemaVersion: 1, generation, rules },
    });
    return invoke<AutomationStatus>('activate_automation', { candidateId: candidate.candidateId });
  }

  private renderAutomation(status: AutomationStatus): void {
    this.automationStatus = status;
    this.renderAutoResponseRules(status);
    const actions: string[] = [];
    if (status.rules.some((rule) => rule.id === 'desktop-user-agent')) actions.push('User-Agent override');
    const autoResponses = this.autoResponseRules(status);
    if (autoResponses.length > 0) actions.push(`${autoResponses.length} auto-response rule${autoResponses.length === 1 ? '' : 's'}`);
    const other = status.rules.filter((rule) => !rule.id.startsWith('desktop-') && rule.request.responseAsset === null).length;
    if (other > 0) actions.push(`${other} advanced rule${other === 1 ? '' : 's'}`);
    this.automationText = actions.length === 0
      ? 'No built-in traffic actions are active.'
      : `Active for new requests: ${actions.join(', ')}.`;
    this.automationKind = 'success';
  }

}

AutomationWorkspace.define('automation-workspace');
