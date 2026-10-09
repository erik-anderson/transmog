import * as monaco from 'monaco-editor/editor/editor.api';
import 'monaco-editor/languages/definitions/typescript/register';
import {
  ModuleKind,
  ModuleResolutionKind,
  ScriptTarget,
  typescriptDefaults,
} from 'monaco-editor/languages/features/typescript/register';

export { ModuleKind, ModuleResolutionKind, ScriptTarget, monaco, typescriptDefaults };
