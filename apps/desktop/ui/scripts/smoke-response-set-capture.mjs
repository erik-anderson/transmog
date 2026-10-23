import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import { nativeResponseSetContext } from "./native-response-set-helpers.mjs";

// Captured CSS and JS are saved as whole current collections. The separate
// lifecycle/intake/demos native probes cover their full dialogs and semantics.
export async function smokeResponseSetCapture(options) {
  const { main, documentId, targets, connect, waitFor, closeNative, screenshot } = options,
    h = nativeResponseSetContext(options),
    { shell, a, sets, traffic, invoke } = h;
  const before = await invoke("automation_status"),
    results = [];
  try {
    for (const kind of ["css", "script"]) {
      const id = "native-captured-" + kind,
        source = await h.captured(
          kind === "css" ? "/page.css" : "/experiment.js",
          id,
          "Native captured " + kind,
        ),
        original = await main.evaluate(a + ".autoResponseBody.value");
      const edited =
        kind === "css"
          ? "h1 { color: rgb(0, 0, 255); }"
          : original.replace('"original"', '"patched"');
      assert.notEqual(edited, original);
      await h.input(
        a + ".autoResponseForm.elements.namedItem('name')",
        "Captured " + kind + " response",
      );
      await main.evaluate(
        `(()=>{const a=${a};a.autoResponseEditBody.checked=true;a.updateAutoResponseBodyEditState();a.autoResponseBody.value=${JSON.stringify(edited)};a.autoResponseBody.dispatchEvent(new Event('input',{bubbles:true}));a.compareResponseDraft('source');})()`,
      );
      await h.wait(`${shell}.comparison.comparisonResult&&!${shell}.comparison.comparisonBusy`);
      assert.equal(
        await main.evaluate(
          `${shell}.comparison.comparisonResult.rows.some(row=>row.field==='Decoded bytes'&&row.state==='different')`,
        ),
        true,
      );
      await main.evaluate(shell + ".comparison.closeComparison()");
      const saved = await h.save(),
        set = saved.responseSets.find((set) => set.id === id),
        selection = await main.evaluate(sets + ".currentSelection()");
      assert.deepEqual(saved.rules, before.rules);
      assert.equal(set.applied, null);
      assert.equal(
        await main.evaluate("globalThis.experimentScriptValue??null"),
        null,
        "Captured script executed in the trusted workbench",
      );
      // The expanded editor preserves content and restores a visible opener.
      if (kind === "script") {
        await h.pointer(a + ".bodyDialogButton");
        await h.wait(a + ".bodyEditor.surface.open");
        await h.wait(`${a}.bodyEditor.bodyEditorReady||${a}.bodyEditor.bodyError`);
        // Browser zoom reduces the CSS viewport. Root CSS zoom scales dvh
        // twice in a native dialog and does not reproduce WebView2 zoom.
        try {
          for (const [width, height] of [
            [800, 600],
            [533, 400],
          ]) {
            await main.call("Emulation.setDeviceMetricsOverride", {
              width,
              height,
              deviceScaleFactor: 1,
              mobile: false,
            });
            await main.evaluate(
              "new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))",
            );
            for (const name of ["Apply to draft", "Save changes", "Cancel"]) {
              const expression = h.button(a + ".bodyEditor.surface", name),
                reachable = await main.evaluate(
                  `(()=>{const n=${expression};n.scrollIntoView({block:'center'});const r=n.getBoundingClientRect();return {name:${JSON.stringify(name)},top:r.top,bottom:r.bottom,height:innerHeight};})()`,
                );
              assert.ok(
                reachable.top >= 0 && reachable.bottom <= reachable.height,
                "Expanded action is reachable: " + JSON.stringify(reachable),
              );
            }
            if (screenshot) {
              const image = await main.call("Page.captureScreenshot", { format: "png" });
              await writeFile(
                screenshot.replace(".png", `-script-expanded-${width}.png`),
                Buffer.from(image.data, "base64"),
              );
            }
          }
        } finally {
          await main.call("Emulation.clearDeviceMetricsOverride");
        }
        await h.pointer(h.button(a + ".bodyEditor.surface", "Apply to draft"));
        await h.wait(`!${a}.bodyEditor.surface.open`);
        assert.equal(await main.evaluate(a + ".autoResponseBody.value"), edited);
        assert.equal(
          await main.evaluate(`${a}.getRootNode().activeElement===${a}.bodyDialogButton`),
          true,
        );
      }
      await h.associate(id, documentId);
      const prior = (await targets()).map((row) => row.id);
      await main.evaluate(sets + ".preview()");
      await h.wait(`${traffic}.previewPageDialog.open&&!${traffic}.previewRulesBusy`);
      assert.equal(await main.evaluate(traffic + ".selectedSessionId"), source);
      await main.evaluate(
        `(async()=>{${traffic}.previewPageScripts.checked=${kind === "script"};await ${traffic}.openCapturedPage();})()`,
      );
      const target = await waitFor(
          async () =>
            (await targets()).find(
              (row) =>
                !prior.includes(row.id) && row.url === "https://example.invalid/captured-page",
            ),
          "Current captured preview did not navigate",
        ),
        preview = await connect(target);
      try {
        if (kind === "css")
          await waitFor(
            () =>
              preview.evaluate(
                `getComputedStyle(document.getElementById('captured-heading')).color==='rgb(0, 0, 255)'`,
              ),
            "Saved current CSS did not render",
          );
        else {
          await waitFor(
            () => preview.evaluate("globalThis.experimentApi"),
            "Captured API dependency missing",
          );
          assert.equal(await preview.evaluate("globalThis.experimentScriptValue"), "patched");
          assert.deepEqual(await preview.evaluate("globalThis.experimentApi"), {
            token: "captured-token",
            value: 42,
          });
          assert.equal(
            await preview.evaluate(`document.getElementById('experiment-result').textContent`),
            "patched: 42",
          );
          assert.equal(
            await preview.evaluate(`document.getElementById('captured-image').naturalWidth`),
            20,
          );
          const policies = await preview.evaluate(
            `Promise.all(['/experiment.js','/experiment-api','/captured-page'].map(url=>fetch(url).then(r=>({cache:r.headers.get('cache-control'),policy:r.headers.get('content-security-policy')}))))`,
          );
          assert.deepEqual(
            policies.map((row) => row.cache),
            ["max-age=3600", "private, max-age=60", "max-age=60"],
          );
          assert.ok(policies.every((row) => row.policy.includes("worker-src 'none'")));
          assert.ok(
            policies[2].policy.includes("default-src 'self'; script-src 'self' 'unsafe-inline'"),
          );
        }
        const label = await main.evaluate(traffic + ".previewReportLabel"),
          report = await invoke("captured_page_report", { label }),
          supplied = report.requests.find(
            (row) => row.ruleId === set.rules[0].id && row.outcome === "auto-response",
          );
        assert.deepEqual(report.variant, selection);
        assert.ok(supplied?.sha256);
        assert.deepEqual(
          report.rules.map((rule) => rule.id),
          set.rules.map((rule) => rule.id),
        );
        if (kind === "script") {
          assert.ok(
            report.requests.some(
              (row) =>
                row.url === "https://example.invalid/experiment-api" &&
                row.outcome === "captured" &&
                row.entryId,
            ),
          );
          await main.evaluate(`${traffic}.showPreviewReport()`);
          await h.wait(
            `${traffic}.previewReportDialog.open&&!${traffic}.previewReportBusy&&${traffic}.previewReportRowsLabel===${JSON.stringify(label)}`,
          );
          await main.evaluate(
            `${traffic}.comparePreviewResponse(${JSON.stringify(String(supplied.id))},${JSON.stringify(source)},${JSON.stringify(supplied.ruleId)})`,
          );
          await h.wait(`${shell}.comparison.comparisonResult&&!${shell}.comparison.comparisonBusy`);
          assert.equal(
            await main.evaluate(
              `${shell}.comparison.comparisonResult.rows.some(row=>row.field==='Decoded bytes'&&row.state==='different')`,
            ),
            true,
          );
          await main.evaluate(shell + ".comparison.closeComparison()");
        }
        let state = await invoke("automation_status");
        state = await invoke("change_response_set", {
          generation: state.generation,
          change: {
            action: "save-version",
            id,
            expectedRevision: set.revision,
            versionId: "reviewed",
            name: "Reviewed captured response",
          },
        });
        state = await invoke("change_response_set", {
          generation: state.generation,
          change: {
            action: "apply",
            id,
            expectedRevision: set.revision,
            acknowledgeOverlaps: true,
          },
        });
        const applied = structuredClone(state.responseSets.find((set) => set.id === id).applied);
        state = await invoke("change_response_set", {
          generation: state.generation,
          change: {
            action: "save-current",
            id,
            expectedRevision: set.revision,
            name: set.name,
            notes: "Later saved current metadata",
            rules: set.rules,
          },
        });
        assert.deepEqual(state.responseSets.find((set) => set.id === id).applied, applied);
        assert.deepEqual(
          state.responseSets.find((set) => set.id === id).versions[0].rules,
          set.rules,
        );
        state = await invoke("change_response_set", {
          generation: state.generation,
          change: { action: "stop", id },
        });
        assert.deepEqual(state.rules, before.rules);
        await preview.call("Page.reload", { ignoreCache: false });
        await waitFor(
          () =>
            preview.evaluate(
              kind === "css"
                ? `getComputedStyle(document.getElementById('captured-heading')).color==='rgb(0, 0, 255)'`
                : `globalThis.experimentScriptValue==='patched'&&globalThis.experimentApi?.value===42`,
            ),
          "Saved current/live changes altered a frozen preview",
        );
        if (screenshot) {
          await writeFile(
            screenshot.replace(".png", `-${kind}-current-report.json`),
            JSON.stringify(report, null, 2),
          );
          const image = await preview.call("Page.captureScreenshot", { format: "png" });
          await writeFile(
            screenshot.replace(".png", `-${kind}-current.png`),
            Buffer.from(image.data, "base64"),
          );
        }
        results.push({
          kind,
          source,
          selection,
          exactPreview: true,
          namedAndLiveIndependent: true,
        });
      } finally {
        await closeNative(
          `Modified preview · Scripts ${kind === "script" ? "on" : "off"} — Transmog`,
        );
        preview.close();
        await waitFor(
          async () => !(await targets()).some((row) => row.id === target.id),
          "Owned captured preview did not close",
        );
      }
      await main.evaluate(
        `(async()=>{await ${a}.discardCollection();await ${a}.leaveResponseSet();})()`,
      );
    }
    process.stdout.write(
      "Native current response sets: captured CSS/JS, whole save, exact preview, captured API dependency, expanded editor, source comparison, named/live independence and frozen reload verified.\n",
    );
    return results;
  } finally {
    await h.cleanup(before);
  }
}
