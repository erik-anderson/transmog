import * as monaco from 'monaco-editor/editor/editor.api';
import {
  ModuleKind,
  ModuleResolutionKind,
  ScriptTarget,
  typescriptDefaults,
} from 'monaco-editor/languages/features/typescript/register';

self.MonacoEnvironment = {
  getWorker(_moduleId: string, label: string): Worker {
    const source = label === 'typescript' || label === 'javascript'
      ? '/monaco-ts.worker.js'
      : '/monaco-editor.worker.js';
    return new Worker(source, { type: 'module', name: `transmog-${label}-worker` });
  },
};

export { ModuleKind, ModuleResolutionKind, ScriptTarget, monaco, typescriptDefaults };
