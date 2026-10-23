import * as monaco from "monaco-editor/editor/editor.api";
import "monaco-editor/editor/editor.main";
import { requestFormatConverter } from "./converter-selection.js";
import "monaco-editor/languages/definitions/typescript/register";
import {
  ModuleKind,
  ModuleResolutionKind,
  ScriptTarget,
  getTypeScriptWorker,
  typescriptDefaults,
} from "monaco-editor/languages/features/typescript/register";

export {
  ModuleKind,
  ModuleResolutionKind,
  ScriptTarget,
  getTypeScriptWorker,
  monaco,
  typescriptDefaults,
};

monaco.editor.addEditorAction({
  id: "transmog.format-converter",
  label: "Open in Format converter",
  precondition: "editorHasSelection",
  contextMenuGroupId: "9_cutcopypaste",
  contextMenuOrder: 4,
  run(editor) {
    const selection = editor.getSelection(),
      model = editor.getModel(),
      element = editor.getDomNode(),
      source = element?.querySelector("textarea") ?? element;
    if (selection && model && source) {
      requestFormatConverter(source, model.getValueInRange(selection));
    }
  },
});
