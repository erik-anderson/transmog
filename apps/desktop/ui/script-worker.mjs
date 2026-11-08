import { TypeScriptWorker, ts } from "monaco-editor/language/typescript/ts.worker.js";
import { analyzeScript } from "./src/script-editor/analyze.ts";

// Extend the existing worker so authoring shares Monaco's parser and lifetime.
TypeScriptWorker.prototype.inspectScript = function (source, language) {
  return analyzeScript(ts.typescript, source, language);
};
