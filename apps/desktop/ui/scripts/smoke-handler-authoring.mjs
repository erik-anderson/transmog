import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";

export async function smokeHandlerAuthoring(call, evaluate, editorPath, screenshotPath) {
  const prior = await evaluate(
    `(()=>{const e=${editorPath};return {draft:e.scriptDraft(),mode:e.testMode,body:e.scriptTestBody.value,request:e.scriptTestHeaders.value,response:e.scriptTestResponseHeaders.value,handler:e.scriptTestHandler.value,end:e.scriptTestEnd.checked,scenarioBody:e.scriptScenarioResponseBody.value,chunks:e.scriptScenarioChunkSizes.value,outcome:e.scriptScenarioOutcome.value};})()`,
  );
  const results = [];
  try {
    for (const language of ["typescript", "javascript"]) {
      for (const name of [
        "onRequestHead",
        "onRequestBody",
        "onResponseHead",
        "onResponseBody",
        "onResponseChunk",
        "onCompleted",
        "onFailed",
      ]) {
        const existingImports =
          name === "onRequestHead"
            ? 'import {pass as event} from "transmog:api";'
            : name === "onRequestBody"
              ? 'import * as text from "transmog:api";'
              : "";
        const off = await evaluate(`(async()=>{
          const e=${editorPath}; e.language=${JSON.stringify(language)}; e.replaceSource(${JSON.stringify(existingImports)},e.language);
          await e.chooseHandler(${JSON.stringify(name)}); await e.refreshSourceAnalysis(); e.$flushUpdates();
          e.testMode='callback'; e.scriptTestHandler.value=${JSON.stringify(name)};
          e.scriptForm.elements.namedItem('readBodies').checked=true;
          e.scriptForm.elements.namedItem('writeBody').checked=true;
          e.scriptForm.elements.namedItem('scriptHeaders').value='user-agent,x-sample,content-type';
          e.scriptForm.elements.namedItem('responseTypes').value='';
          e.scriptTestHeaders.value='Content-Type: text/plain'; e.scriptTestResponseHeaders.value='Content-Type: application/json';
          e.scriptTestBody.value=${JSON.stringify(name === "onResponseBody" ? '{"original":true}' : "before")}; e.scriptTestEnd.checked=true;
          e.markScriptChanged(); await e.testScript();
          return {summary:e.testResult,status:e.scriptText,kind:e.statusKind,evidence:e.testEvidence};
        })()`);
        assert.match(off.summary, /Callback completed.*Action: continue/, off.status);
        const offResult = JSON.parse(off.evidence.split("Returned result:\n")[1]);
        assert.equal(offResult.logs?.length ?? 0, 0);
        const on = await evaluate(
          `(async()=>{const e=${editorPath}; e.sourceEditor.setValue(e.sourceEditor.getValue().replace(' = false;',' = true;'));await e.testScript();return {summary:e.testResult,status:e.scriptText,evidence:e.testEvidence};})()`,
        );
        const expected =
          name === "onResponseChunk"
            ? "emit"
            : ["onCompleted", "onFailed"].includes(name)
              ? "continue"
              : "edit";
        assert.match(on.summary, new RegExp("Callback completed.*Action: " + expected), on.status);
        const onResult = JSON.parse(on.evidence.split("Returned result:\n")[1]);
        if (["onCompleted", "onFailed"].includes(name)) assert.ok(onResult.logs.length > 0);
        if (name === "onRequestHead")
          assert.equal(onResult.decision.edits.headers[0].value, "Transmog/1.0");
        if (name === "onResponseHead")
          assert.equal(onResult.decision.edits.headers[0].value, "preview");
        if (name === "onRequestBody" || name === "onResponseChunk")
          assert.equal(
            new TextDecoder().decode(
              Uint8Array.from(onResult.decision.edits?.bytes ?? onResult.decision.bytes),
            ),
            "after",
          );
        if (name === "onResponseBody")
          assert.equal(
            JSON.parse(new TextDecoder().decode(Uint8Array.from(onResult.decision.edits.bytes)))
              .inspected,
            true,
          );
        results.push({ language, handler: name, existingImports, off: "continue", on: expected });
        if (language === "typescript" && name === "onResponseChunk") {
          const stream = await evaluate(
            `(async()=>{const e=${editorPath};e.testMode='scenario';e.scriptScenarioOutcome.value='response';e.scriptTestResponseHeaders.value='Content-Type: text/plain';e.scriptScenarioResponseBody.value='before';e.scriptScenarioChunkSizes.value='2,2';e.markTestChanged();await e.testScript();return {summary:e.testResult,status:e.scriptText,evidence:e.testEvidence};})()`,
          );
          assert.match(stream.summary, /Scenario passed.*Exchange: completed/, stream.status);
          const scenario = JSON.parse(stream.evidence.split("Scenario evidence:\n")[1]);
          assert.equal(new TextDecoder().decode(Uint8Array.from(scenario.responseBody)), "after");
          assert.ok(scenario.streamCalls.some((call) => call.action === "need-more"));
          results[results.length - 1].splitStream = true;
        }
        if (language === "typescript" && name === "onRequestHead" && screenshotPath) {
          await evaluate(`(async()=>{const e=${editorPath};await e.toggleHandlers();})()`);
          await writeFile(
            screenshotPath.replace(/\.png$/i, "-handler-menu.png"),
            Buffer.from(
              (
                await call("Page.captureScreenshot", {
                  format: "png",
                  captureBeyondViewport: false,
                })
              ).data,
              "base64",
            ),
          );
          await evaluate(`${editorPath}.handlerMenu.hidePopover()`);
        }
      }
    }
    if (screenshotPath)
      await writeFile(
        screenshotPath.replace(/\.png$/i, "-handler-results.json"),
        JSON.stringify(results, null, 2),
      );
    return results;
  } finally {
    await evaluate(
      `(()=>{const e=${editorPath};const prior=${JSON.stringify(prior)};e.applyDraft(prior.draft);e.testMode=prior.mode;e.scriptTestBody.value=prior.body;e.scriptTestHeaders.value=prior.request;e.scriptTestResponseHeaders.value=prior.response;e.scriptTestHandler.value=prior.handler;e.scriptTestEnd.checked=prior.end;e.scriptScenarioResponseBody.value=prior.scenarioBody;e.scriptScenarioChunkSizes.value=prior.chunks;e.scriptScenarioOutcome.value=prior.outcome;})()`,
    );
  }
}
