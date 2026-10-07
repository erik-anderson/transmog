import initialState from '../initial-state.json';
import { attr, observable } from '@microsoft/webui-framework';
import { Channel, convertFileSrc, invoke } from '@tauri-apps/api/core';
import { WorkspaceElement } from '../workspace-element.js';
import type { SessionSummary, SessionPage, SessionHint, SessionDetail, BodyInspection } from '../models.js';
import { callerLabel, describeError, optionalText, loadSessionDetail, clientResponseSource, autoResponseUnavailableReason } from '../utilities.js';

export class TrafficWorkspace extends WorkspaceElement {
  @attr view = 'traffic';
  @attr({ attribute: 'page-size' }) pageSize = '100';
  @observable sessions: Array<SessionSummary & {statusText: string; callerText: string; durationText: string; responseText: string; stateText: string; selectionState: string}> = initialState.sessions;
  @observable sessionText = initialState.sessionText;
  @observable sessionKind = initialState.sessionKind;
  @observable followLatest = initialState.followLatest;
  @observable followText = initialState.followText;
  @observable selectedSessionId: string | null = initialState.selectedSessionId;
  @observable selectedMethodText = initialState.selectedMethodText;
  @observable selectedUrlText = initialState.selectedUrlText;
  @observable selectedStatusText = initialState.selectedStatusText;
  @observable selectedSuccess = initialState.selectedSuccess;
  @observable selectedAutoResponded = initialState.selectedAutoResponded;
  @observable reuseDisabled = initialState.reuseDisabled;
  @observable reuseTitle = initialState.reuseTitle;
  @observable matchedRuleHidden = initialState.matchedRuleHidden;
  @observable matchedRuleLabel = initialState.matchedRuleLabel;
  @observable requestText = initialState.requestText;
  @observable responseText = initialState.responseText;
  @observable bodyText = initialState.bodyText;
  @observable imageHidden = initialState.imageHidden;
  @observable imageSource = initialState.imageSource;
  @observable bodyBoundaries: Array<{boundary: string; label: string}> = initialState.bodyBoundaries;
  filterForm!: HTMLFormElement;
  sessionScroller!: HTMLElement;
  bodyBoundary!: HTMLSelectElement;
  bodyRepresentation!: HTMLSelectElement;
  bodyDecoded!: HTMLInputElement;
  bodyMaxBytes!: HTMLInputElement;
  private watching = false;
  private sessionUpdates: Channel<SessionHint> | null = null;
  private programmaticSessionScroll = false;
  private selectedSessionDetail: SessionDetail | null = null;
  private matchedAutoResponseId: string | null = null;
  private inspectionGeneration = 0;
  private queryPending: Promise<void> | null = null;
  private queryAgain = false;

  selectedSessionIdChanged(): void {
    this.sessions = this.sessions.map((session) => {
      const selectionState = session.id === this.selectedSessionId ? 'true' : 'false';
      return session.selectionState === selectionState ? session : { ...session, selectionState };
    });
  }

  protected hydratedCallback(): void { void this.watchSessions(); }
  disconnectedCallback(): void {
    if (this.sessionUpdates) this.sessionUpdates.onmessage = () => undefined;
    this.sessionUpdates = null;
    this.watching = false;
    super.disconnectedCallback();
  }
  async refreshSessions(event?: Event): Promise<void> {
    event?.preventDefault();
    this.queryAgain = true;
    if (this.queryPending) return this.queryPending;
    this.queryPending = (async () => {
      while (this.queryAgain && this.isConnected) {
        this.queryAgain = false;
        await this.querySessions();
      }
    })();
    try { await this.queryPending; } finally { this.queryPending = null; }
  }
  private renderSessions(sessions: SessionSummary[]): void {
    const previous = new Map(this.sessions.map((session) => [session.id, session]));
    this.sessions = sessions.slice(0, 200).map((session) => {
      const row = { ...session,
        selectionState: session.id === this.selectedSessionId ? 'true' : 'false',
        statusText: session.autoResponse === null ? session.status?.toString() ?? '—' : `${session.status ?? session.autoResponse.status} · AUTO`,
        callerText: callerLabel(session.caller), durationText: `${session.durationMs} ms`, responseText: `${session.responseBytes} B`,
        stateText: `${session.terminal}${session.loss ? ' · loss' : ''}${session.capturing ? ' · capture' : ''}`,
      };
      const old = previous.get(session.id);
      return old && JSON.stringify(old) === JSON.stringify(row) ? old : row;
    });
    this.$flushUpdates();
    if (this.followLatest) {
      this.programmaticSessionScroll = true;
      this.sessionScroller.scrollTop = this.sessionScroller.scrollHeight;
      requestAnimationFrame(() => { this.programmaticSessionScroll = false; });
    }
    this.renderFollowState();
  }
  sessionScrolled(): void {
    if (this.programmaticSessionScroll || this.selectedSessionId !== null) return;
    const distance = this.sessionScroller.scrollHeight - this.sessionScroller.scrollTop - this.sessionScroller.clientHeight;
    this.followLatest = distance <= 4;
    this.renderFollowState();
  }
  selectWithKeyboard(session: SessionSummary, event: KeyboardEvent): void {
    if (event.target instanceof HTMLButtonElement) return;
    if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); void this.inspectSession(session); }
  }
  selectWithButton(session: SessionSummary, event: Event): void { event.stopPropagation(); void this.inspectSession(session); }
  dragSession(session: SessionSummary, event: DragEvent): void {
    event.dataTransfer?.setData('application/x-transmog-session', session.id);
    event.dataTransfer?.setData('text/plain', `${session.method} ${session.host}${session.path}`);
    if (event.dataTransfer) event.dataTransfer.effectAllowed = 'copy';
  }
  beginAutoResponseFromSelected(): void {
    if (this.selectedSessionId && this.selectedSessionDetail) this.$emit('autoresponse-request', { sessionId: this.selectedSessionId, detail: this.selectedSessionDetail });
  }
  showMatchedAutoResponse(): void { if (this.matchedAutoResponseId) this.$emit('matched-rule-request', this.matchedAutoResponseId); }
  private async querySessions(event?: Event): Promise<void> {
    event?.preventDefault();
    const data = new FormData(this.filterForm);
    try {
      const page = await invoke<SessionPage>('query_sessions', {
        query: {
          cursor: null,
          latest: true,
          limit: Number(this.pageSize),
          terminal: null,
          method: optionalText(data.get('method')),
          host: optionalText(data.get('host')),
        },
      });
      if (!this.isConnected) return;
      this.renderSessions(page.sessions);
      const message = `Loaded ${page.sessions.length} sessions · evicted ${page.evicted} · gaps ${page.sequenceGaps} · subscriber lag ${page.subscriberLag}`;
      this.sessionText = message;
      this.sessionKind = 'success';
      this.diagnostic = message;
    } catch (error: unknown) {
      const message = `Session query failed: ${describeError(error)}`;
      this.sessionText = message;
      this.sessionKind = 'error';
      this.diagnostic = message;
    }
  }

  async watchSessions(): Promise<void> {
    if (this.watching) {
      this.sessionText = 'Live session refresh is already enabled.';
      this.sessionKind = 'success';
      return;
    }
    this.watching = true;
    this.sessionText = 'Enabling live session refresh…';
    this.sessionKind = 'progress';
    const onEvent = new Channel<SessionHint>();
    onEvent.onmessage = () => { if (this.isConnected) void this.refreshSessions(); };
    this.sessionUpdates = onEvent;
    try {
      await invoke<void>('watch_sessions', { onEvent });
      await this.refreshSessions();
      this.sessionText = 'Watching live traffic. Waiting for proxied requests.';
      this.sessionKind = 'success';
      this.diagnostic = 'Live session refresh enabled.';
    } catch (error: unknown) {
      this.watching = false;
      this.sessionUpdates = null;
      const message = `Live refresh failed: ${describeError(error)}`;
      this.sessionText = message;
      this.sessionKind = 'error';
      this.diagnostic = message;
    }
  }

  private async inspectSession(session: SessionSummary): Promise<void> {
    const inspection = ++this.inspectionGeneration;
    this.selectedSessionId = session.id;
    this.selectedSessionDetail = null;
    this.followLatest = false;
    this.renderFollowState();
    this.selectedMethodText = session.method;
    this.selectedUrlText = `${session.host}${session.path}`;
    this.selectedStatusText = session.status === null ? 'Pending' : `${session.status}`;
    this.selectedSuccess = session.status !== null && session.status >= 200 && session.status < 400;
    this.selectedAutoResponded = false;
    this.reuseDisabled = true;
    this.reuseTitle = 'Checking whether the completed response can be reused…';
    this.$emit('selection-changed', null);
    this.matchedAutoResponseId = null;
    this.matchedRuleHidden = true;
    this.imageHidden = true;
    this.imageSource = '';
    this.bodyBoundaries = [];
    this.bodyText = 'No body selected.';
    this.requestText = 'Loading bounded request evidence…';
    this.responseText = 'Loading bounded response evidence…';
    try {
      const detail = await loadSessionDetail(session.id, session.terminal === 'completed');
      if (inspection !== this.inspectionGeneration || !this.isConnected) return;
      this.selectedSessionId = session.id;
      this.selectedSessionDetail = detail;

      this.requestText = JSON.stringify({
        requests: detail.requests,
        hookEffects: detail.hookEffects,
        routeSelection: detail.routeSelection,
        routeAttempts: detail.routeAttempts,
      }, null, 2);
      const autoResponse = detail.autoResponse === null
        ? null
        : `AUTO-RESPONSE: “${detail.autoResponse.ruleName}” was the first matching rule and served its saved response without contacting the origin.`;
      this.selectedStatusText = detail.autoResponse === null
        ? (session.status === null ? 'Pending' : `${session.status}`)
        : `${detail.autoResponse.status} · AUTO`;
      this.selectedAutoResponded = detail.autoResponse !== null;
      this.matchedAutoResponseId = detail.autoResponse?.ruleId ?? null;
      this.matchedRuleHidden = detail.autoResponse === null;
      this.matchedRuleLabel = detail.autoResponse === null
        ? 'Show matched rule'
        : `Show “${detail.autoResponse.ruleName}”`;
      this.responseText = `${autoResponse === null ? '' : `${autoResponse}\n\n`}${JSON.stringify({
        responses: detail.responses,
        terminal: detail.terminal,
        websocket: detail.websocket,
        diagnostics: detail.diagnostics,
        sequenceLoss: detail.sequenceLoss,
      }, null, 2)}`;
      this.bodyBoundaries = detail.storedBodies.map((body) => ({ boundary: body.boundary, label: `${body.boundary} · ${body.availability} · ${body.retainedBytes} retained` }));
      this.bodyText = detail.storedBodies.length === 0
        ? 'No retained body boundaries are available.'
        : 'Choose a representation and inspect the selected boundary.';
      const reusable = clientResponseSource(detail) !== null;
      this.reuseDisabled = !reusable;
      this.reuseTitle = reusable
        ? 'Create an auto-response rule from this client-visible response'
        : autoResponseUnavailableReason(detail);
      this.$emit('selection-changed', { sessionId: session.id, detail, reusable });
    } catch (error: unknown) {
      if (inspection !== this.inspectionGeneration || !this.isConnected) return;
      const message = `Inspector unavailable: ${describeError(error)}`;
      this.requestText = message;
      this.responseText = message;
    }
  }

  resumeLatest(): void {
    this.inspectionGeneration += 1;
    this.selectedSessionId = null;
    this.selectedSessionDetail = null;
    this.matchedAutoResponseId = null;
    this.matchedRuleHidden = true;
    this.reuseDisabled = true;
    this.reuseTitle = '';
    this.$emit('selection-changed', null);
    this.followLatest = true;

    this.programmaticSessionScroll = true;
    this.sessionScroller.scrollTop = this.sessionScroller.scrollHeight;
    requestAnimationFrame(() => { this.programmaticSessionScroll = false; });
    this.renderFollowState();
  }

  private renderFollowState(): void {
    this.followText = this.followLatest
      ? 'Following latest traffic'
      : this.selectedSessionId === null
        ? 'Live updates continue · scroll position paused'
        : 'Live updates continue · selected request pinned';
  }

  async exportLiveCapture(): Promise<void> {
    this.sessionText = 'Exporting a sealed TMCap snapshot…';
    this.sessionKind = 'progress';
    try {
      const result = await invoke<{destination: string; records: number; bytes: number}>('export_live_capture');
      const message = `Exported ${result.records} records (${result.bytes} bytes) to ${result.destination}`;
      this.sessionText = message;
      this.sessionKind = 'success';
      this.diagnostic = message;
      this.showNotice('TMCap export complete', result.destination, null, null);
    } catch (error: unknown) {
      const message = `TMCap export failed: ${describeError(error)}`;
      this.sessionText = message;
      this.sessionKind = 'error';
      this.showNotice('TMCap export failed', message, null, null);
    }
  }

  async inspectBody(): Promise<void> {
    if (this.selectedSessionId === null || this.bodyBoundary.value.length === 0) {
      this.bodyText = 'Select a session with retained body metadata first.';
      return;
    }
    this.bodyText = 'Loading bounded body representation…';
    this.imageHidden = true;
    this.imageSource = '';
    try {
      const inspection = await invoke<BodyInspection>('inspect_body', {
        request: {
          sessionId: this.selectedSessionId,
          boundary: this.bodyBoundary.value,
          representation: this.bodyRepresentation.value,
          decodeContent: this.bodyDecoded.checked,
          offset: 0,
          maxBytes: Number(this.bodyMaxBytes.value),
        },
      });
      const summary = `${inspection.metadata.boundary} · ${inspection.representation} · ${inspection.displayBytes} bytes${inspection.decoded ? ' · decoded' : ' · encoded'}${inspection.truncated ? ' · truncated' : ''}`;
      if (inspection.previewHandle !== null) {
        this.imageSource = convertFileSrc(
          `preview/${inspection.previewHandle}`,
          'transmog-preview',
        );
        this.imageHidden = false;
        const safety = inspection.previewMimeType === 'image/svg+xml'
          ? 'Loaded as a sandboxed image; scripts and external resources are blocked.'
          : 'Normalized to PNG in an isolated decoder, then loaded as an image.';
        this.bodyText = `${summary}\n${safety}`;
      } else {
        this.bodyText = `${summary}${inspection.warning ? `\n${inspection.warning}` : ''}\n\n${inspection.display}`;
      }
    } catch (error: unknown) {
      this.bodyText = `Body inspector unavailable: ${describeError(error)}`;
    }
  }

}

TrafficWorkspace.define('traffic-workspace');
