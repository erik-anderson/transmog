import initialState from '../initial-state.json';
import { attr, observable } from '@microsoft/webui-framework';
import '../paused-exchange/paused-exchange.js';
import { invoke } from '@tauri-apps/api/core';
import { WorkspaceElement } from '../workspace-element.js';
import type { BreakpointPhase, PausedExchange, BreakpointStatus } from '../models.js';
import { describeError } from '../utilities.js';

export class BreakpointWorkspace extends WorkspaceElement {
  @attr view = 'traffic';
  @attr theme = 'system';
  @observable pausedExchanges: PausedExchange[] = initialState.pausedExchanges;
  @observable breakpointText = initialState.breakpointText;
  breakpointForm!: HTMLFormElement;
  protected hydratedCallback(): void { void this.refreshBreakpoints(); }
  private renderBreakpoints(status: BreakpointStatus): void {
    this.pausedExchanges = status.paused;
    this.breakpointText = status.enabled ? 'Controller attached; no exchanges are paused.' : 'Breakpoint controller disabled.';
  }
  onDecision(event: CustomEvent<{paused: PausedExchange; action: object}>): void { void this.submitBreakpoint(event.detail.paused,event.detail.action); }
  async enableBreakpoints(event: Event): Promise<void> {
    event.preventDefault();
    const data = new FormData(this.breakpointForm);
    const phases: BreakpointPhase[] = [];
    if (data.get('requestHead') === 'on') phases.push('request-head');
    if (data.get('requestBody') === 'on') phases.push('request-body');
    if (data.get('responseHead') === 'on') phases.push('response-head');
    if (data.get('responseBody') === 'on') phases.push('response-body');
    try {
      const status = await invoke<BreakpointStatus>('enable_breakpoints', {
        settings: {
          phases,
          bodyLimit: 4 * 1024 * 1024,
          maxPending: 64,
          timeoutMs: 30_000,
        },
      });
      this.renderBreakpoints(status);
    } catch (error: unknown) {
      this.diagnostic = `Breakpoint setup failed: ${describeError(error)}`;
    }
  }

  async disableBreakpoints(): Promise<void> {
    const status = await invoke<BreakpointStatus>('disable_breakpoints');
    this.renderBreakpoints(status);
  }

  async refreshBreakpoints(): Promise<void> {
    try {
      this.renderBreakpoints(await invoke<BreakpointStatus>('breakpoint_status'));
    } catch (error: unknown) {
      this.diagnostic = `Breakpoint refresh failed: ${describeError(error)}`;
    }
  }

  async submitBreakpoint(paused: PausedExchange, action: object): Promise<void> {
    try {
      const status = await invoke<BreakpointStatus>('decide_breakpoint', {
        decision: { decisionId: paused.decisionId, exchangeId: paused.exchangeId, action },
      });
      this.renderBreakpoints(status);
    } catch (error: unknown) {
      this.diagnostic = `Breakpoint decision failed: ${describeError(error)}`;
      await this.refreshBreakpoints();
    }
  }

}

BreakpointWorkspace.define('breakpoint-workspace');
