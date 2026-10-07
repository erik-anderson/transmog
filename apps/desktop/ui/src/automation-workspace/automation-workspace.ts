import initialState from '../initial-state.json';
import { attr, observable } from '@microsoft/webui-framework';
import '../script-editor/script-editor.js';
import { invoke } from '@tauri-apps/api/core';
import { WorkspaceElement } from '../workspace-element.js';
import type { SessionDetail, BodyInspection, AutomationCandidate, AutomationRule, AutomationStatus, ResponseAsset, AutoResponseSource, SelectedResponse } from '../models.js';
import { describeError, optionalText, parseHeaderLines, encodeHeaders, shortUrl, editableCharacterEncoding, encodeEditedText, loadSessionDetail, clientResponseSource, autoResponseUnavailableReason } from '../utilities.js';

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
  private automationStatus: AutomationStatus | null = null;
  private autoResponseSourceState: AutoResponseSource | null = null;
  private editingAutoResponseId: string | null = null;
  private draggedRuleId: string | null = null;

  protected hydratedCallback(): void { void this.refreshAutomation(); }
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
  allowSessionDrop(event: DragEvent): void { if (event.dataTransfer?.types.includes('application/x-transmog-session')) event.preventDefault(); }
  dropSession(event: DragEvent): void {
    event.preventDefault();
    const sessionId = event.dataTransfer?.getData('application/x-transmog-session');
    if (sessionId) void this.beginAutoResponseFromSessionId(sessionId);
  }
  dragRule(id: string,event: DragEvent): void {
    this.draggedRuleId = id;
    event.dataTransfer?.setData('application/x-transmog-autoresponse-rule',id);
    if (event.dataTransfer) event.dataTransfer.effectAllowed = 'move';
  }
  allowRuleDrop(id: string,event: DragEvent): void { if (this.draggedRuleId !== null && this.draggedRuleId !== id) event.preventDefault(); }
  finishRuleDrag(): void { this.draggedRuleId = null; }
  async dropRule(id: string,event: DragEvent): Promise<void> {
    event.preventDefault();
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
    try { this.renderAutomation(await this.activateAutoResponseOrder(current,ordered)); }
    catch (error: unknown) { this.automationText = `Auto-response reorder failed: ${describeError(error)}`; this.automationKind = 'error'; }
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
    if (this.selection === null) {
      this.showNotice('Select a completed response', 'Choose a request in Traffic whose client response body was retained, then try again.', null, null);
      return;
    }
    await this.populateCapturedAutoResponse(this.selection.sessionId, this.selection.detail);
  }

  async beginAutoResponseFromSessionId(sessionId: string): Promise<void> {
    try {
      const detail = await loadSessionDetail(sessionId, true);
      if (this.selection?.sessionId === sessionId) this.selection = { ...this.selection, detail };
      await this.populateCapturedAutoResponse(sessionId, detail);
    } catch (error: unknown) {
      this.showNotice('Response cannot be reused', describeError(error), null, null);
    }
  }

  async populateCapturedAutoResponse(sessionId: string, detail: SessionDetail): Promise<void> {
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
    this.autoResponseForm.reset();
    const elements = this.autoResponseForm.elements;
    (elements.namedItem('name') as HTMLInputElement).value = `${source.request.method} ${shortUrl(source.request.target)}`;
    this.autoResponseMethod.value = source.request.method.toUpperCase();
    (elements.namedItem('url') as HTMLInputElement).value = source.request.target;
    this.autoResponseStatus.value = String(source.response.status);
    this.responseReadOnly = true;
    this.autoResponseMediaType.value = source.body.mediaType ?? '';
    this.responseReadOnly = true;
    this.responseFieldsHidden = false;
    (elements.namedItem('responseHeaders') as HTMLTextAreaElement).value = '';
    this.responseHeadersHidden = true;
    this.capturedOptionsHidden = false;
    this.autoResponseEditBody.checked = false;
    this.autoResponsePreserveEncoding.checked = true;
    this.preserveEncodingDisabled = source.body.contentCodings.length === 0;
    this.autoResponseBody.value = '';
    this.bodyReadOnly = true;
    this.editorTitle = 'New rule from captured response';
    this.sourceText = `Client-visible response ${source.response.status} from ${sessionId}. ${source.body.retainedBytes} encoded bytes retained${source.body.contentCodings.length === 0 ? '' : ` · ${source.body.contentCodings.join(', ')}`}.`;
    this.autoResponseSourceState = {
      kind: 'captured',
      sessionId,
      boundary: 'client-response',
      contentCodings: source.body.contentCodings,
      bodyEditable: false,
      textEncoding: editableCharacterEncoding(source.body.charset),
    };
    this.updateAutoResponseHeaderFilterState();
    this.updateAutoResponseBodyEditState();
    this.editorHidden = false;
    this.$flushUpdates();
    this.autoResponseForm.scrollIntoView({ block: 'nearest' });

    if (source.body.retainedBytes === 0) {
      this.autoResponseSourceState.bodyEditable = this.autoResponseSourceState.textEncoding !== null;
      this.updateAutoResponseBodyEditState();
      return;
    }
    if (source.body.retainedBytes > MAX_AUTORESPONSE_EDIT_BYTES) {
      this.sourceText += ' The body is too large for decoded editing; exact replay remains available.';
      return;
    }
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
      if (!inspection.truncated && inspection.representation === 'original-text' && inspection.textEncoding !== null) {
        this.autoResponseBody.value = inspection.display;
        this.autoResponseSourceState.bodyEditable = true;
        this.autoResponseSourceState.textEncoding = inspection.textEncoding;
        this.sourceText += ` Decoded editing will preserve ${inspection.textEncoding}.`;
        this.updateAutoResponseBodyEditState();
      } else {
        this.sourceText += ' This body is not safely editable as complete decoded text; exact replay remains available.';
      }
    } catch {
      this.sourceText += ' This body is not safely editable as decoded text; exact replay remains available.';
    }
  }

  beginScratchAutoResponse(): void {
    this.editingAutoResponseId = null;
    this.autoResponseSourceState = { kind: 'scratch' };
    this.autoResponseForm.reset();
    this.editorTitle = 'New response from scratch';
    this.sourceText = 'New authored response. Transmog will repair Content-Length and omit invalid hop-by-hop fields.';
    this.responseReadOnly = false;
    this.responseReadOnly = false;
    this.responseFieldsHidden = false;
    this.responseHeadersHidden = false;
    this.capturedOptionsHidden = true;
    this.bodyReadOnly = false;
    this.autoResponseBody.value = '';
    this.updateAutoResponseHeaderFilterState();
    this.editorHidden = false;
    this.$flushUpdates();
    this.autoResponseForm.scrollIntoView({ block: 'nearest' });
  }

  cancelAutoResponseEdit(): void {
    this.editorHidden = true;
    this.autoResponseSourceState = null;
    this.editingAutoResponseId = null;
  }

  updateAutoResponseHeaderFilterState(): void {
    const enabled = this.autoResponseMethod.value.toUpperCase() === 'POST';
    this.requestHeadersHidden = !enabled;
    const input = this.autoResponseForm.elements.namedItem('requestHeaders') as HTMLTextAreaElement;

    if (!enabled) input.value = '';
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
  }

  async saveAutoResponse(event: Event): Promise<void> {
    event.preventDefault();
    const data = new FormData(this.autoResponseForm);
    const source = this.autoResponseSourceState;
    if (source === null) return;
    this.automationText = 'Validating and saving the auto-response…';
    this.automationKind = 'progress';
    try {
      let assetReference: string;
      if (source.kind === 'existing') {
        assetReference = source.assetReference;
      } else {
        const suffix = crypto.randomUUID().replaceAll('-', '');
        const assetId = `autoresponse-${suffix}`;
        const editedBody = source.kind === 'captured' && this.autoResponseEditBody.checked
          ? encodeEditedText(String(data.get('body') ?? ''), source.textEncoding)
          : null;
        const asset = source.kind === 'captured'
          ? await invoke<ResponseAsset>('create_response_asset_from_session', {
              input: {
                id: assetId,
                revision: 1,
                exchangeId: source.sessionId,
                boundary: source.boundary,
                decodedBody: editedBody,
                preserveContentEncoding: this.autoResponsePreserveEncoding.checked,
              },
            })
          : await invoke<ResponseAsset>('create_response_asset', {
              input: {
                id: assetId,
                revision: 1,
                status: Number(data.get('status') ?? 200),
                headers: encodeHeaders(parseHeaderLines(String(data.get('responseHeaders') ?? ''))),
                body: Array.from(new TextEncoder().encode(String(data.get('body') ?? ''))),
                mediaType: optionalText(data.get('mediaType')),
              },
            });
        assetReference = `${asset.id}@${asset.revision}`;
      }

      const current = await invoke<AutomationStatus>('automation_status');
      const existing = this.editingAutoResponseId === null
        ? undefined
        : current.rules.find((candidate) => candidate.id === this.editingAutoResponseId);
      const id = existing?.id ?? `autoresponse-rule-${crypto.randomUUID().replaceAll('-', '')}`;
      const method = String(data.get('method') ?? '').toUpperCase();
      const requestHeaders = method === 'POST'
        ? parseHeaderLines(String(data.get('requestHeaders') ?? '')).map((header) => ({
            name: header.name,
            condition: { kind: 'exact', value: Array.from(new TextEncoder().encode(header.value)) },
          }))
        : [];
      const rule: AutomationRule = {
        id,
        displayName: String(data.get('name') ?? '').trim(),
        enabled: true,
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
      this.cancelAutoResponseEdit();
      this.automationText = `Saved “${rule.displayName}”${existing === undefined ? ' at the top' : ''}. The first enabled matching rule will win.`;
      this.automationKind = 'success';
    } catch (error: unknown) {
      this.automationText = `Auto-response could not be saved: ${describeError(error)}`;
      this.automationKind = 'error';
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
    try {
      const current = await invoke<AutomationStatus>('automation_status');
      const ordered = this.autoResponseRules(current);
      const index = ordered.findIndex((rule) => rule.id === ruleId);
      if (index < 0) return;
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
    } catch (error: unknown) {
      this.automationText = `Auto-response change failed: ${describeError(error)}`;
      this.automationKind = 'error';
    }
  }

  editAutoResponse(rule: AutomationRule): void {
    const url = rule.matcher.url?.kind === 'exact' ? rule.matcher.url.value : '';
    this.editingAutoResponseId = rule.id;
    this.autoResponseSourceState = { kind: 'existing', assetReference: rule.request.responseAsset ?? '' };
    this.autoResponseForm.reset();
    const elements = this.autoResponseForm.elements;
    (elements.namedItem('name') as HTMLInputElement).value = rule.displayName ?? 'Auto-response';
    this.autoResponseMethod.value = rule.matcher.method ?? 'GET';
    (elements.namedItem('url') as HTMLInputElement).value = url;
    (elements.namedItem('requestHeaders') as HTMLTextAreaElement).value = rule.matcher.requestHeaders
      .filter((header) => header.condition.kind === 'exact')
      .map((header) => `${header.name}: ${new TextDecoder().decode(new Uint8Array(header.condition.value ?? []))}`)
      .join('\n');
    this.editorTitle = `Edit “${rule.displayName ?? 'Auto-response'}”`;
    this.sourceText = 'Using the immutable saved response attached to this rule. Criteria can be changed without duplicating its response bytes.';
    this.responseReadOnly = true;
    this.responseReadOnly = true;
    this.responseFieldsHidden = true;
    this.responseHeadersHidden = true;
    this.capturedOptionsHidden = true;
    this.autoResponseBody.value = '';
    this.bodyReadOnly = true;
    this.updateAutoResponseHeaderFilterState();
    this.editorHidden = false;
    this.$flushUpdates();
    this.autoResponseForm.scrollIntoView({ block: 'nearest' });
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
