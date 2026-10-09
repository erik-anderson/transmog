import '@microsoft/webui-framework/lazy-hydration.js';
import './proxy-toggle/proxy-toggle.js';
import './pane-divider/pane-divider.js';
import './trace-drop-target/trace-drop-target.js';
import './app-shell/app-shell.js';
import './traffic-workspace/traffic-workspace.js';
import './message-inspector/message-inspector.js';
import './hex-viewer/hex-viewer.js';
import './settings-workspace/settings-workspace.js';
import { invoke } from '@tauri-apps/api/core';
import { describeError } from './utilities.js';

const report = (code: string, value: unknown): void => {
  void invoke<void>('record_frontend_diagnostic', { code, message: describeError(value) }).catch(() => undefined);
};
window.addEventListener('error', (event) => report('unhandled-error', event.error ?? event.message));
window.addEventListener('unhandledrejection', (event) => report('unhandled-rejection', event.reason));
