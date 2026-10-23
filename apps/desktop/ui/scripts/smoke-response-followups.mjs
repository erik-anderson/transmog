import assert from "node:assert/strict";
import { createServer } from "node:http";
import { connect } from "node:net";
import { createHash } from "node:crypto";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";
import { nativeResponseSetContext } from "./native-response-set-helpers.mjs";

export async function smokeResponseFollowups(context) {
  const { main, directory } = context;
  const { shell, a, sets, traffic, invoke, wait, button, pointer, key, openSet } =
    nativeResponseSetContext(context);
  const origin = createServer((request, response) => {
    if (request.url === "/download") {
      response.writeHead(200, {
        "Content-Type": "application/octet-stream",
        "Content-Length": "1000000",
      });
      response.write("part");
    } else {
      response.writeHead(200, { "Content-Type": "text/plain" });
      response.end("live");
    }
  });
  const originSockets = new Set(),
    clients = [];
  let releasePendingUpgrade;
  origin.on("connection", (socket) => {
    originSockets.add(socket);
    socket.on("close", () => originSockets.delete(socket));
  });
  origin.on("upgrade", (request, socket) => {
    const accept = createHash("sha1")
      .update(request.headers["sec-websocket-key"] + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11")
      .digest("base64");
    const upgrade = () =>
      socket.write(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: " +
          accept +
          "\r\n\r\n",
      );
    if (request.url === "/opening-socket") releasePendingUpgrade = upgrade;
    else upgrade();
  });
  await new Promise((resolve) => origin.listen(0, "127.0.0.1", resolve));
  const originAddress = origin.address(),
    base = `http://127.0.0.1:${originAddress.port}`;
  let started = false;
  try {
    const product = await invoke("product_state");
    product.preferences.configureSystemProxy = false;
    product.preferences.decryptHttps = false;
    product.preferences.allowRemoteClients = false;
    product.preferences.proxyPort = null;
    assert.equal(product.privacy.terminateWebsocketsOnClear, false);
    await invoke("save_product_state", { productState: product });
    const status = await invoke("start_proxy", {
      request: {
        listen: "127.0.0.1:0",
        route: "auto",
        allowRemoteClients: false,
        caCertificatePath: "",
        caPrivateKeyPath: "",
      },
    });
    started = true;
    const port = Number(status.listener.split(":").at(-1));
    const request = (path, websocket = false) =>
      new Promise((resolve, reject) => {
        const socket = connect({ host: "127.0.0.1", port });
        clients.push(socket);
        socket.on("error", () => {});
        let received = "";
        socket.on("data", (data) => {
          received += data.toString("latin1");
          if (
            received.includes("\r\n\r\n") &&
            (!path.includes("download") || received.includes("part"))
          )
            resolve(socket);
        });
        socket.once("connect", () =>
          socket.write(
            `GET ${base}${path} HTTP/1.1\r\nHost: 127.0.0.1:${originAddress.port}\r\n${websocket ? "Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n" : "Connection: close\r\n"}\r\n`,
          ),
        );
        socket.once("error", reject);
      });
    const find = async (path) =>
      (await invoke("query_sessions", { query: { limit: 100, offset: 0 } })).sessions.find(
        (row) => row.path === path,
      );
    const reveal = async (id) => {
      await main.evaluate(
        `(async()=>{await ${shell}.activateView('traffic');await ${traffic}.refreshSessions(undefined,true);await ${traffic}.revealSession(${JSON.stringify(id)});})()`,
      );
      await wait(`${traffic}.selectedDetail?.id===${JSON.stringify(id)}`);
    };
    const cancelInspected = async () => {
      await pointer(button(`${traffic}.querySelector('.selection-actions')`, "Actions"));
      await pointer(
        button(`${traffic}.querySelector('#selected-request-actions')`, "Cancel request"),
      );
    };
    const openOpaque = () =>
      new Promise((resolve, reject) => {
        const socket = connect({ host: "127.0.0.1", port });
        clients.push(socket);
        let headers = "";
        socket.on("error", () => {});
        socket.once("error", reject);
        socket.on("data", (chunk) => {
          headers += chunk.toString("latin1");
          if (headers.includes("\r\n\r\n")) {
            if (headers.startsWith("HTTP/1.1 200")) resolve(socket);
            else reject(new Error("Opaque CONNECT was not accepted"));
          }
        });
        socket.once("connect", () =>
          socket.write(
            `CONNECT 127.0.0.1:${originAddress.port} HTTP/1.1\r\nHost: 127.0.0.1:${originAddress.port}\r\n\r\n`,
          ),
        );
      });
    const beforeOpaque = new Set(originSockets),
      selectedOpaque = await openOpaque();
    await context.waitFor(() => [...originSockets].some((socket) => !beforeOpaque.has(socket)));
    const selectedOrigin = [...originSockets].find((socket) => !beforeOpaque.has(socket));
    assert.ok(selectedOrigin);
    const opaqueRow = async () =>
      (await invoke("query_sessions", { query: { limit: 100, offset: 0 } })).sessions.find(
        (row) => row.method === "CONNECT" && row.terminal === "active",
      );
    await context.waitFor(async () => Boolean(await opaqueRow()));
    const opaqueId = (await opaqueRow()).id;
    const survivingOpaque = await openOpaque();
    await reveal(opaqueId);
    await cancelInspected();
    await context.waitFor(
      () => selectedOpaque.destroyed && selectedOrigin.destroyed,
      "Opaque CONNECT cancellation left a transport open",
    );
    await context.waitFor(
      async () => (await invoke("session_detail", { id: opaqueId })).terminalState === "cancelled",
      "Opaque CONNECT cancellation lost terminal evidence",
    );
    const opaqueDetail = await invoke("session_detail", { id: opaqueId });
    assert.match(opaqueDetail.terminal, /Transmog/);
    assert.doesNotMatch(opaqueDetail.terminal, /by caller/);
    let survivingResponse = "";
    survivingOpaque.on("data", (chunk) => {
      survivingResponse += chunk.toString("latin1");
    });
    survivingOpaque.write(
      `GET /through-opaque HTTP/1.1\r\nHost: 127.0.0.1:${originAddress.port}\r\nConnection: keep-alive\r\n\r\n`,
    );
    await context.waitFor(
      () => survivingResponse.includes("live"),
      "Cancellation interrupted the other opaque tunnel",
    );
    await request("/after-opaque-cancel");
    assert.ok(
      await find("/after-opaque-cancel"),
      "Opaque cancellation stopped future HTTP requests",
    );
    survivingOpaque.destroy();
    const download = await request("/download"),
      websocket = await request("/socket", true);
    await context.waitFor(async () => Boolean((await find("/socket"))?.streamLifetime?.live));
    const downloadId = (await find("/download")).id,
      websocketId = (await find("/socket")).id;
    await reveal(downloadId);
    await cancelInspected();
    await context.waitFor(
      async () => (await find("/download"))?.terminal === "cancelled",
      "Download cancellation missing terminal evidence",
    );
    await context.waitFor(() => download.destroyed, "Canceled download stayed open");
    assert.equal((await find("/download")).id, downloadId);
    await reveal(websocketId);
    await pointer(button(traffic, "Clear all"));
    await wait(`!${traffic}.removingTraffic`);
    assert.equal(await main.evaluate(`${traffic}.selectedSessionId`), websocketId);
    assert.equal((await find("/socket")).id, websocketId);
    assert.equal(websocket.destroyed, false);
    assert.match(
      await main.evaluate(`${traffic}.trafficUndoText`),
      /1 open or connecting WebSocket was kept/,
    );
    await main.evaluate(`${shell}.activateView('settings')`);
    await wait(
      `${shell}.settings.workspaceLoaded!==false&&${shell}.settings.settingsForm?.elements.namedItem('terminateWebsocketsOnClear')`,
    );
    await pointer(
      `${shell}.settings.settingsForm.elements.namedItem('terminateWebsocketsOnClear')`,
    );
    await pointer(button(`${shell}.settings.settingsForm`, "Save settings"));
    await wait(`!${shell}.settings.settingsBusy&&!${shell}.settings.settingsDirty`);
    assert.equal((await invoke("product_state")).privacy.terminateWebsocketsOnClear, true);
    await main.evaluate(`${shell}.activateView('traffic')`);
    await pointer(button(traffic, "Clear all"));
    await wait(`!${traffic}.removingTraffic`);
    await context.waitFor(() => websocket.destroyed, "Clear did not terminate the WebSocket");
    assert.equal(await find("/socket"), undefined);
    const openingRequest = request("/opening-socket", true);
    await context.waitFor(
      () => Boolean(releasePendingUpgrade),
      "Origin did not receive pending upgrade",
    );
    await context.waitFor(
      async () => Boolean(await find("/opening-socket")),
      "Pending WebSocket row missing",
    );
    const openingId = (await find("/opening-socket")).id;
    await pointer(button(traffic, "Clear all"));
    await wait(`!${traffic}.removingTraffic`);
    assert.equal(
      (await find("/opening-socket")).id,
      openingId,
      "Clear hid a pending WebSocket before its upgrade",
    );
    const keptText = await main.evaluate(`${traffic}.trafficUndoText`);
    assert.match(keptText, /1 open or connecting WebSocket was kept/);
    assert.doesNotMatch(keptText, /Enable/);
    releasePendingUpgrade();
    const opened = await openingRequest;
    await context.waitFor(
      async () => Boolean((await find("/opening-socket"))?.streamLifetime?.live),
      "Cleared pending WebSocket did not remain inspectable after opening",
    );
    await pointer(button(traffic, "Clear all"));
    await wait(`!${traffic}.removingTraffic`);
    await context.waitFor(
      () => opened.destroyed,
      "A later Clear did not terminate the now-open WebSocket",
    );
    await request("/after-clear");
    assert.ok(await find("/after-clear"), "Clear stopped the proxy listener");
    const other = await request("/manual-socket", true);
    await context.waitFor(async () =>
      Boolean((await find("/manual-socket"))?.streamLifetime?.live),
    );
    const otherId = (await find("/manual-socket")).id;
    await reveal(otherId);
    await cancelInspected();
    await context.waitFor(
      () => other.destroyed,
      "Manual WebSocket cancellation did not close its transport",
    );
    assert.equal((await find("/manual-socket")).id, otherId);
    assert.equal((await find("/manual-socket")).terminal, "interrupted");
    // Historical response attribution and paused participation use the real engine.
    const asset = await invoke("create_response_asset", {
      input: {
        id: "followup-native-body",
        revision: 1,
        status: 200,
        headers: [],
        body: [108, 111, 99, 97, 108],
        mediaType: "text/plain",
      },
    });
    const action = { headers: [], replaceBody: null, discardBody: false, abortReason: null };
    const rule = {
      id: "followup-native-rule",
      displayName: "Native checkout response",
      revision: 1,
      priority: 0,
      enabled: true,
      matcher: {
        method: "GET",
        url: { kind: "exact", value: base + "/supplied" },
        requestHeaders: [],
        responseHeaders: [],
      },
      request: {
        ...action,
        responseAsset: asset.id + "@" + asset.revision,
        allowNonIdempotentBodyReplacement: false,
      },
      response: action,
    };
    let automation = await invoke("automation_status");
    automation = await invoke("change_response_set", {
      generation: automation.generation,
      change: {
        action: "create",
        id: "native-followups",
        name: "Native saved debugging work",
        notes: "Open this row by clicking its notes.",
        rules: [rule],
      },
    });
    automation = await invoke("change_response_set", {
      generation: automation.generation,
      change: {
        action: "apply",
        id: "native-followups",
        expectedRevision: 1,
        acknowledgeOverlaps: true,
        resume: false,
      },
    });
    await request("/supplied");
    await context.waitFor(async () => Boolean((await find("/supplied"))?.autoResponse));
    await reveal((await find("/supplied")).id);
    await main.call("Emulation.setDeviceMetricsOverride", {
      width: 800,
      height: 600,
      deviceScaleFactor: 1,
      mobile: false,
    });
    await main.evaluate(
      "new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))",
    );
    const inspectorUrl = await main.evaluate(
      `(()=>{const n=${traffic}.querySelector('#inspector-heading'),r=n.getBoundingClientRect();return {width:r.width,text:n.textContent.trim(),disabled:n.disabled};})()`,
    );
    assert.ok(
      inspectorUrl.width >= 60,
      `Native compact auto-response inspector hid its URL: ${JSON.stringify(inspectorUrl)}`,
    );
    assert.equal(inspectorUrl.disabled, false);
    assert.equal(inspectorUrl.text, base + "/supplied");
    await pointer(`${traffic}.querySelector('#inspector-heading')`);
    await wait(`${traffic}.querySelector('#selected-url-full').matches(':popover-open')`);
    assert.equal(
      await main.evaluate(
        `${traffic}.querySelector('#selected-url-full .full-value').textContent.trim()`,
      ),
      base + "/supplied",
    );
    await key("Escape");
    const inspectorShot = await main.call("Page.captureScreenshot", { format: "png" });
    await writeFile(
      join(directory, "native-auto-response-inspector-800.png"),
      Buffer.from(inspectorShot.data, "base64"),
    );
    await pointer(button(`${traffic}.querySelector('.selection-actions')`, "Details…"));
    await wait(`${traffic}.timingAutoResponse?.ruleName==='Native checkout response'`);
    await main.evaluate(
      "new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))",
    );
    const shot = await main.call("Page.captureScreenshot", { format: "png" });
    await writeFile(
      join(directory, "native-auto-response-details-800.png"),
      Buffer.from(shot.data, "base64"),
    );
    await key("Escape");
    await openSet(automation, "native-followups");
    await pointer(button(`${sets}.lifecycle`, "Pause"));
    await wait(`${sets}.status?.responseSets.find(set=>set.id==='native-followups')?.paused`);
    await main.evaluate(`${a}.showAutomationSection('rules')`);
    await wait(
      `${a}.querySelector('autoresponse-switch')?.autoresponsesLabel?.includes('(0 active)')`,
    );
    await main.evaluate(`${a}.showAutomationSection('experiments')`);
    if (await main.evaluate(`${a}.collectionOpen`)) await pointer(button(sets, "← Response sets"));
    await pointer(`${sets}.querySelector('.response-set-library tbody tr td:nth-child(2)')`);
    await wait(`${a}.collectionOpen&&${sets}.setId==='native-followups'`);
    assert.equal((await invoke("app_status")).lifecycle, "running");
    return {
      opaqueConnectCanceled: true,
      otherOpaqueTunnelPreserved: true,
      downloadCanceled: true,
      websocketProtected: true,
      pendingUpgradeClearPreservesEvidence: true,
      terminationSettingSaved: true,
      forceClearClosedTransport: true,
      manualWebsocketCancellationKeepsEvidence: true,
      autoResponseDetails: true,
      pausedActiveCount: true,
      fullRowOpen: true,
    };
  } finally {
    for (const socket of clients) socket.destroy();
    for (const socket of originSockets) socket.destroy();
    await new Promise((resolve) => origin.close(resolve));
    if (started) await invoke("stop_proxy").catch(() => undefined);
  }
}
