import assert from "node:assert/strict";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";

const pause = () => new Promise((resolve) => setTimeout(resolve, 40));
async function eventually(action) {
  const deadline = Date.now() + 15000;
  for (;;) {
    const value = await action();
    if (value) return value;
    if (Date.now() > deadline) throw new Error("Native session check timed out");
    await pause();
  }
}
async function targetCall(target, method, params) {
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve, { once: true });
    socket.addEventListener("error", reject, { once: true });
  });
  try {
    return await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("Native review command timed out")), 10000);
      socket.addEventListener("message", ({ data }) => {
        const message = JSON.parse(String(data));
        if (message.id === 1) {
          clearTimeout(timer);
          if (message.error) reject(new Error(message.error.message));
          else resolve(message.result);
        }
      });
      socket.send(JSON.stringify({ id: 1, method, params }));
    });
  } finally {
    socket.close();
  }
}
async function targetEvaluate(target, expression) {
  const result = await targetCall(target, "Runtime.evaluate", {
    expression,
    awaitPromise: true,
    returnByValue: true,
  });
  if (result.exceptionDetails)
    throw new Error(
      result.exceptionDetails.exception?.description ??
        result.exceptionDetails.exception?.value?.message ??
        result.exceptionDetails.exception?.value ??
        result.exceptionDetails.text,
    );
  return result.result.value;
}

export async function smokeMcpSessions(cli, invoke, key, grantId, instanceId, screenshotPath) {
  const directory = await mkdtemp(join(tmpdir(), "transmog-mcp-review-"));
  const portIndex = process.argv.indexOf("--port");
  const port = portIndex < 0 ? 9333 : process.argv[portIndex + 1];
  const targets = async () =>
    fetch(`http://127.0.0.1:${port}/json/list`).then((response) => response.json());
  const before = new Set((await targets()).map((target) => target.id));
  const args = { instanceId };
  let sessionId;
  try {
    const status = await invoke("mcp_status");
    const grant = status.grants.find((grant) => grant.id === grantId);
    assert.ok(grant, "Native session check requires its disposable agent grant");
    await invoke("mcp_update_key", {
      id: grant.id,
      revision: grant.revision,
      input: {
        name: grant.name,
        permissions: [...new Set([...grant.permissions, "load-captures", "prepare-changes"])],
        scope: { kind: "all" },
        omitSensitiveValues: false,
        expiresAt: null,
      },
    });
    const path = join(directory, "review.har");
    await writeFile(
      path,
      JSON.stringify({
        log: {
          version: "1.2",
          creator: { name: "Native session fixture", version: "1" },
          entries: [
            {
              startedDateTime: "2026-01-01T00:00:00Z",
              time: 1,
              request: {
                method: "GET",
                url: "http://example.test/private",
                httpVersion: "HTTP/1.1",
                headers: [],
                cookies: [],
                queryString: [],
                headersSize: -1,
                bodySize: 0,
              },
              response: {
                status: 200,
                statusText: "OK",
                httpVersion: "HTTP/1.1",
                headers: [],
                cookies: [],
                headersSize: -1,
                bodySize: 7,
                redirectURL: "",
                content: { size: 7, mimeType: "text/plain", text: "private" },
              },
              cache: {},
              timings: { send: 0, wait: 1, receive: 0 },
            },
          ],
        },
      }),
    );
    const mainBefore = await cli(key, "transmog_traffic_list", { ...args, sessionId: "main" });
    sessionId = (
      await cli(key, "transmog_session_create", { ...args, name: "Native private investigation" })
    ).sessionId;
    assert.ok(sessionId);
    const input = { ...args, sessionId };
    await cli(key, "transmog_capture_load", { ...input, path, operationId: "native-load" });
    const loaded = await eventually(async () => {
      const job = await cli(key, "transmog_operation_status", {
        ...args,
        operationId: "native-load",
      });
      assert.notEqual(job.state, "failed");
      return job.state === "completed" ? job : false;
    });
    const page = await cli(key, "transmog_traffic_list", input);
    assert.equal(page.totalMatched, 1);
    assert.deepEqual(
      (await cli(key, "transmog_traffic_list", { ...args, sessionId: "main" })).sessions,
      mainBefore.sessions,
    );
    const definition = {
      kind: "headers",
      name: "Agent private header",
      host: "example.test",
      phase: "request",
      operations: [{ operation: "set", name: "X-Debug", value: "one" }],
    };
    const rule = await cli(key, "transmog_rule_prepare", { ...input, rule: definition });
    assert.ok(rule.id);
    const label = await invoke("mcp_review_session", { sessionId });
    assert.equal(await invoke("mcp_review_session", { sessionId }), label);
    let target = await eventually(async () =>
      (await targets()).find((target) => target.type === "page" && !before.has(target.id)),
    );
    await eventually(async () =>
      targetEvaluate(
        target,
        "window.__TAURI_INTERNALS__?.invoke('mcp_review_state').then(state=>state.available)",
      ).catch(() => false),
    );
    assert.equal(
      (
        await targetEvaluate(
          target,
          "window.__TAURI_INTERNALS__.invoke('query_sessions',{query:{}})",
        )
      ).sessions.length,
      1,
    );
    assert.equal(
      await targetEvaluate(
        target,
        "window.__TAURI_INTERNALS__.invoke('mcp_set_enabled',{enabled:false}).then(()=>false,()=>true)",
      ),
      true,
    );
    await eventually(async () =>
      targetEvaluate(
        target,
        "typeof document.querySelector('app-shell')?.shadowRoot?.querySelector('session-review')?.open==='function' && document.querySelector('app-shell').shadowRoot.querySelector('session-review').sessionReviewAvailable",
      ).catch(() => false),
    );
    await targetEvaluate(
      target,
      `(async()=>{const review=document.querySelector('app-shell').shadowRoot.querySelector('session-review');review.open();await review.refresh();await review.headersEditor.selectRule(${JSON.stringify(rule.id)});const input=review.headersEditor.hrNameInput;input.value='Unsaved human private draft';input.dispatchEvent(new Event('input',{bubbles:true}));await review.dirty();})()`,
    );
    assert.equal((await cli(key, "transmog_session_close", input)).failed, true);
    await cli(key, "transmog_rule_prepare", {
      ...input,
      id: rule.id,
      expectedRevision: rule.revision,
      rule: { ...definition, name: "Agent newer revision" },
    });
    await eventually(async () =>
      targetEvaluate(
        target,
        "document.querySelector('app-shell').shadowRoot.querySelector('session-review').headersEditor.hrStale",
      ),
    );
    assert.equal(
      await targetEvaluate(
        target,
        "document.querySelector('app-shell').shadowRoot.querySelector('session-review').headersEditor.hrName",
      ),
      "Unsaved human private draft",
    );
    if (screenshotPath) {
      const image = await targetCall(target, "Page.captureScreenshot", { format: "png" });
      await writeFile(
        screenshotPath.replace(/\.png$/i, "-mcp-private-rules.png"),
        Buffer.from(image.data, "base64"),
      );
    }
    await targetEvaluate(
      target,
      "(async()=>{const review=document.querySelector('app-shell').shadowRoot.querySelector('session-review');await review.headersEditor.discard();await review.dirty();})()",
    );
    await targetEvaluate(target, "window.__TAURI_INTERNALS__.invoke('close_trace_window')").catch(
      () => undefined,
    );
    await eventually(async () => !(await targets()).some((entry) => entry.id === target.id));
    assert.equal((await cli(key, "transmog_traffic_list", input)).totalMatched, 1);
    await invoke("mcp_review_session", { sessionId });
    target = await eventually(async () =>
      (await targets()).find((target) => target.type === "page" && !before.has(target.id)),
    );
    await cli(key, "transmog_capture_unload", {
      ...input,
      captureId: loaded.result.captureId,
      operationId: "native-unload",
    });
    await eventually(
      async () =>
        (await cli(key, "transmog_operation_status", { ...args, operationId: "native-unload" }))
          .state === "completed",
    );
    assert.equal((await cli(key, "transmog_traffic_list", input)).totalMatched, 0);
    await targetEvaluate(target, "window.__TAURI_INTERNALS__.invoke('close_trace_window')").catch(
      () => undefined,
    );
    await cli(key, "transmog_session_close", input);
    sessionId = null;
    return {
      privateCaptureReview: true,
      reviewReusesBackend: true,
      reviewRoleEnforced: true,
      privateDraftConflicts: true,
      closeWindowPreservesSession: true,
      unloadReleasesEvidence: true,
    };
  } finally {
    if (sessionId) await cli(key, "transmog_session_close", { ...args, sessionId });
    await rm(directory, { recursive: true, force: true });
  }
}
