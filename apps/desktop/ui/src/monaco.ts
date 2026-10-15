import * as monaco from 'monaco-editor/editor/editor.api';
import 'monaco-editor/editor/editor.main';
import 'monaco-editor/languages/definitions/typescript/register';
import {
  ModuleKind,
  ModuleResolutionKind,
  ScriptTarget,
  getTypeScriptWorker,
  typescriptDefaults,
} from 'monaco-editor/languages/features/typescript/register';

export { ModuleKind, ModuleResolutionKind, ScriptTarget, getTypeScriptWorker, monaco, typescriptDefaults };
