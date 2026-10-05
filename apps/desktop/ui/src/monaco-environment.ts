import type { TrustedTypesWindow } from 'trusted-types/lib';

const trustedTypesWindow = window as unknown as TrustedTypesWindow;
const monacoPolicy = trustedTypesWindow.trustedTypes?.createPolicy('monaco', {
  createHTML(value: string): string {
    return value;
  },
  createScriptURL(value: string): string {
    const url = new URL(value, window.location.origin);
    if (url.origin !== window.location.origin || !url.pathname.startsWith('/monaco-')) {
      throw new TypeError('Monaco worker URL must use an app-owned same-origin asset');
    }
    return value;
  },
});

self.MonacoEnvironment = {
  createTrustedTypesPolicy() {
    return monacoPolicy;
  },
  getWorker(_moduleId: string, label: string): Worker {
    const source = label === 'typescript' || label === 'javascript'
      ? '/monaco-ts.worker.js'
      : '/monaco-editor.worker.js';
    return new Worker(source, { type: 'module', name: `transmog-${label}-worker` });
  },
};
