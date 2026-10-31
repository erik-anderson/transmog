import { transformSync } from "esbuild";
import { checkCaptureTools } from "./check-capture-tools.mjs";
import { checkResponseSetFollowups } from "./check-response-set-followups.mjs";
import { checkFormatConverter } from "./check-format-converter.mjs";
import { checkRequestControls } from "./check-request-controls.mjs";
import { checkTrafficShortcuts } from "./check-traffic-shortcuts.mjs";
import {
  checkResponseSetWorkspaces,
  checkResponseSetDraftRegressions,
  checkResponseSetPendingDialogs,
} from "./check-response-set-workspaces.mjs";
import { checkSavedDemos } from "./check-saved-demos.mjs";
import { checkResponseSetLifecycle } from "./check-response-set-lifecycle.mjs";
import { checkResponseSetIntake } from "./check-response-set-intake.mjs";
import { checkResponseSetIntakeRefresh } from "./check-response-set-intake-refresh.mjs";
import { checkResponseSetIntakeMove } from "./check-response-set-intake-move.mjs";
import { checkResponseSetCollectionAdds } from "./check-response-set-collection-adds.mjs";
import { checkAutomationContracts } from "./check-automation-contracts.mjs";
import { checkUnifiedRules, newTrafficRule } from "./check-unified-rules.mjs";
import { checkActionMenus, openActionGroup } from "./check-action-menus.mjs";
import { checkPortableCases } from "./check-portable-cases.mjs";
import { checkResponseSnapshots } from "./check-response-snapshots.mjs";
import { checkResponseFiles } from "./check-response-files.mjs";
import { checkResponseSetGuide } from "./check-response-set-guide.mjs";
import { checkPreviewPrerequisites } from "./check-preview-prerequisites.mjs";
import { checkBodyRefresh } from "./check-body-refresh.mjs";
import {
  checkBodySource,
  checkJsonFields,
  checkBodySourceBounds,
  waitForBodySource,
  waitForBodyText,
  bodyText,
} from "./check-body-source.mjs";
import { checkBodyDecoding, checkXssiJson } from "./check-body-decoding.mjs";
import { checkResponseSetPreview } from "./check-response-set-preview.mjs";
import { checkTextEdits } from "./check-text-edits.mjs";
import { checkHeaderRules } from "./check-header-rules.mjs";
import { checkIncidents } from "./check-incidents.mjs";
import { checkResponseBodyEditor } from "./check-response-body-editor.mjs";
import { checkBinaryResponse } from "./check-binary-response.mjs";
import { checkMatchingTests, checkMatchingSelectionRace } from "./check-matching-tests.mjs";
import { checkScripts } from "./check-scripts.mjs";
import { checkMonacoIcons } from "./check-monaco-icons.mjs";
import { checkAutoResponseDragging } from "./check-autoresponse-dragging.mjs";
import { checkTrafficFilters } from "./check-traffic-filters.mjs";
import { checkTrafficAdvanced } from "./check-traffic-advanced.mjs";
import { checkStreams } from "./check-streams.mjs";
import { checkWindowShortcuts } from "./check-window-shortcuts.mjs";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { build, Protocol } from "@microsoft/webui";
import { checkHexViewer } from "./check-hex-viewer.mjs";
import { checkTrafficPresentation } from "./check-traffic-presentation.mjs";
import { checkTrafficClassifications } from "./check-traffic-classifications.mjs";
import { checkTrafficPerformance } from "./check-traffic-performance.mjs";
import { checkTrafficList } from "./check-traffic-list.mjs";
import { checkAppSelection } from "./check-app-selection.mjs";
import { checkTimingLabels } from "./check-timing-labels.mjs";
import { checkTrafficTimeline } from "./check-traffic-timeline.mjs";
import { checkRequestOrder, checkRequestOrderModel } from "./check-request-order.mjs";
import { checkTrafficNavigation } from "./check-traffic-navigation.mjs";
import { checkTraceImport } from "./check-trace-import.mjs";
import { checkTraceSave } from "./check-trace-save.mjs";
import { checkComparison } from "./check-comparison.mjs";
import { checkPagePreviewRules } from "./check-page-preview-rules.mjs";
import { checkTrafficFailures } from "./check-traffic-failures.mjs";
import { checkNetworkProfiles } from "./check-network-profiles.mjs";
import { checkClientCertificates } from "./check-client-certificates.mjs";
import { checkMcp } from "./check-mcp.mjs";
import { checkExtensions } from "./check-extensions.mjs";
import { checkVisualizers } from "./check-visualizers.mjs";
import { checkCopy } from "./check-copy.mjs";
import { checkAuth } from "./check-auth.mjs";
import { checkCertificates } from "./check-certificates.mjs";
import { checkConnectTunnels } from "./check-connect-tunnels.mjs";
import { checkUserAgentOverrides } from "./check-user-agent-overrides.mjs";
import { checkUserAgentPresets } from "./check-user-agent-presets.mjs";
import { checkLoopback } from "./check-loopback.mjs";

// Use the repository's existing, locked browser test installation.
const { chromium } = createRequire(
  new URL("../../../../e2e/playwright/package.json", import.meta.url),
)("playwright");
const root = fileURLToPath(new URL("../", import.meta.url));
await checkUserAgentPresets(root);
await checkRequestOrderModel(root);
const filterCatalog = JSON.parse(
  await readFile(resolve(root, "../../../crates/app/src/sessions/filter-catalog.json"), "utf8"),
);
const authCatalogSource = await readFile(
  resolve(root, "../../../crates/auth/src/catalog.rs"),
  "utf8",
);
const authConversions = [
  ...authCatalogSource.matchAll(
    /id:\s*"([^"]+)"[\s\S]*?label:\s*"([^"]+)"[\s\S]*?hint:\s*"([^"]+)"[\s\S]*?accepts_bare_base64:\s*(true|false)/g,
  ),
].map(([, id, label, hint, bare]) => ({ id, label, hint, acceptsBareBase64: bare === "true" }));
assert.equal(authConversions.length, 8);
const initial = JSON.parse(await readFile(resolve(root, "src/initial-state.json"), "utf8"));
const scriptHelp = JSON.parse(
  await readFile(resolve(root, "../../../crates/proxy-script/help.json"), "utf8"),
);
const scriptDeclarations = await readFile(
  resolve(root, "../../../crates/proxy-script/generated/transmog-script.d.ts"),
  "utf8",
);
const metadata = JSON.parse(await readFile(resolve(root, "dist/client-metafile.json"), "utf8"));
const assets = JSON.parse(await readFile(resolve(root, "dist/client-assets.json"), "utf8"));
const built = build({
  appDir: resolve(root, "src"),
  plugin: "webui",
  css: "link",
  cssPublicBase: "/",
  cssFileNameTemplate: "[name]-[hash].[ext]",
  projectionManifests: [resolve(root, "dist/webui-projection.json")],
});
assert.deepEqual(built.warnings, []);
const protocol = new Protocol(built.protocol, { plugin: "webui" });
const renderHtml = (viewerMode = false) =>
  protocol
    .render({
      ...initial,
      viewerMode,
      language: "en",
      pageTitle: "Transmog",
      heading: "Inspect traffic without losing the thread",
      lifecycleLabel: "Stopped",
      lifecycleKind: "stopped",
      listener: "Not listening",
    })
    .toString()
    .replace(/<script(?=[\s>])/g, '<script nonce="workspace-check"');
// Keep one build's bytes for the whole run, even if another workspace rebuilds dist.
const resources = new Map(
  await Promise.all(
    assets.map(async (asset) => [
      asset.path,
      { body: await readFile(resolve(root, "dist", asset.file)), contentType: asset.contentType },
    ]),
  ),
);
resources.set("/document.css", {
  body: await readFile(resolve(root, "dist/document.css")),
  contentType: "text/css",
});
resources.set("/transmog-icon.svg", {
  file: resolve(root, "../icons/icon.svg"),
  contentType: "image/svg+xml",
});
for (let index = 0; index < built.cssFiles.length; index += 2)
  resources.set("/" + built.cssFiles[index], {
    body: built.cssFiles[index + 1],
    contentType: "text/css",
  });
const browser = await chromium.launch({ headless: true });
const page = await browser.newPage({
  viewport: { width: 1280, height: 800 },
  timezoneId: "America/Los_Angeles",
});
await page
  .context()
  .grantPermissions(["clipboard-read", "clipboard-write"], { origin: "https://workspace.test" });
const errors = [];
const requests = [];
page.on("pageerror", (error) => errors.push(error.message));
page.on("console", (message) => {
  if (message.type() === "error") errors.push(message.text());
});
page.on("request", (request) => requests.push(new URL(request.url()).pathname));
await page.route("https://workspace.test/**", async (route) => {
  const path = new URL(route.request().url()).pathname;
  if (path === "/") {
    await route.fulfill({
      contentType: "text/html",
      body: renderHtml(new URL(route.request().url()).searchParams.has("viewer")),
      headers: {
        "Content-Security-Policy":
          "default-src 'none'; base-uri 'none'; object-src 'none'; script-src 'self' 'nonce-workspace-check'; worker-src 'self'; style-src 'self' 'unsafe-inline'; font-src data:; img-src 'self' data:; connect-src 'self'; require-trusted-types-for 'script'; trusted-types webui monaco",
      },
    });
  } else {
    if (path === "/preview/pixel.png") {
      await route.fulfill({
        contentType: "image/png",
        body: Buffer.from(
          "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==",
          "base64",
        ),
      });
      return;
    }
    if (process.argv.includes("--scripts-only") && path.startsWith("/chunks/tsMode-")) {
      // Exercise publishing while Monaco's first TypeScript registration is still pending.
      await new Promise((resolve) => setTimeout(resolve, 750));
    }
    const asset = resources.get(path);
    if (!asset) {
      await route.fulfill({ status: 404, body: "Not found" });
      return;
    }
    await route.fulfill({
      contentType: asset.contentType,
      body: asset.body ?? (await readFile(asset.file)),
    });
  }
});
await page.exposeFunction(
  "__compileFixtureScript",
  (source) => transformSync(source, { loader: "ts", target: "es2022" }).code,
);
await page.addInitScript(
  (configuration) => {
    const { workspace, networkProfiles, scriptDeclarations, scriptHelp, authConversions } =
      configuration;
    const caller = { kind: "local-process", processName: "Fixture", processId: 42 };
    const summary = (id, index = 0) => ({
      id,
      requestNumber: String(index + 1),
      caller,
      method: "GET",
      host: "example.test",
      path: "/" + id,
      url: "http://example.test/" + id,
      startedAt: 1000 + index,
      contentType:
        id === "second" ? "application/json" : id === "image" ? "image/webp" : "text/plain",
      topLevelNavigation: id === "first",
      fetchDestination: { first: "document", second: "script", image: "image" }[id] ?? null,
      protocol: "HTTP/1.1",
      status: id === "cached" ? 304 : 200,
      durationMs: index + 1,
      requestBytes: 0,
      responseBytes: id === "cached" ? 0 : 4,
      terminal: "completed",
      loss: false,
      capturing: false,
      autoResponse: null,
    });
    const detail = (id) => {
      if (state.composerResponseDetail?.id === id)
        return structuredClone(state.composerResponseDetail);
      const row = structuredClone(state.sessions.find((row) => row.id === id) ?? summary(id));
      if (row.streamLifetime?.live && row.quietStartedAt != null) {
        row.durationMs = Date.now() - row.quietStartedAt;
        row.streamLifetime.durationMs = row.durationMs;
      }
      const fullHeaders = state.fullHeaders ?? [
        {
          name: "Accept",
          value: "*/*",
          valueBytes: 3,
          fieldBytes: 13,
          sensitive: false,
          binary: false,
        },
        {
          name: "Authorization",
          value: "[redacted]",
          valueBytes: 5133,
          fieldBytes: 5150,
          sensitive: true,
          binary: false,
        },
      ];
      const headerSummary = {
        totalFields: fullHeaders.length,
        valueBytes: fullHeaders.reduce((total, header) => total + header.valueBytes, 0),
        serializedBytes: fullHeaders.reduce((total, header) => total + header.fieldBytes, 2),
        authorization: fullHeaders.some((header) => header.name === "Authorization")
          ? "present"
          : "absent",
        proxyAuthorization: fullHeaders.some((header) => header.name === "Proxy-Authorization")
          ? "present"
          : "absent",
      };
      return {
        id,
        requestNumber: row.requestNumber,
        originalId: row.originalId ?? null,
        originalRequestNumber: row.originalRequestNumber ?? null,
        originalRequestNumberProvenance: row.originalRequestNumberProvenance ?? null,
        stream: state.streams?.[id]
          ? {
              ...state.streams[id],
              lifetime: row.streamLifetime ?? state.streams[id].lifetime,
              httpDurationMs: row.httpDurationMs ?? state.streams[id].httpDurationMs,
              responseHeadersMs: row.responseHeadersMs ?? state.streams[id].responseHeadersMs,
            }
          : null,
        performance: state.performance,
        savedEvidence: state.savedTiming,
        traceId: row.traceId ?? null,
        startedAt: row.startedAt,
        caller,
        requests: [
          {
            boundary: "client-request",
            method: row.method,
            target: row.url,
            status: null,
            protocol: "HTTP/1.1",
            headers: fullHeaders.slice(0, 512),
            summary: headerSummary,
          },
        ],
        responses: [
          {
            boundary: "client-response",
            method: null,
            target: null,
            status: row.status,
            protocol: "HTTP/1.1",
            headers: [
              { name: "Content-Type", value: row.contentType, sensitive: false, binary: false },
            ],
          },
        ],
        bodies: [],
        storedBodies: row.storedBodies ?? [
          {
            exchangeId: id,
            boundary: "client-response",
            observedBytes: row.responseBytes,
            retainedBytes: row.responseBytes,
            availability: "complete",
            mediaType: row.contentType,
            charset: "utf-8",
            contentCodings: [],
            sha256: null,
            reason: null,
          },
        ],
        diagnostics: [],
        hookEffects: [],
        headerEffects: row.headerEffects ?? [],
        bodyEdits: row.bodyEdits ?? [],
        routeSelection: null,
        routeAttempts: [],
        terminal:
          row.terminalText ??
          (row.terminal === "failed"
            ? "failed at Upstream: Upstream; request_committed=true; response_committed=false; client error (Connect)"
            : row.terminal),
        terminalState: row.terminal,
        websocket: null,
        sequenceLoss: 0,
        autoResponse: row.autoResponse ?? null,
      };
    };
    const state = (globalThis.__workspaceFixture = {
      calls: {},
      workspace: JSON.parse(localStorage.getItem("workspace") ?? JSON.stringify(workspace)),
      lifecycle: "stopped",
      sessions: ["first", "second", "cached", "image"].map(summary),
      paused: [],
      queryDelay: 0,
      slowDetail: false,
      slowBody: false,
    });

    const canonicalProtocol = (value) => {
      const text = value.toLowerCase();
      return (
        configuration.filterCatalog.protocol.find(
          (option) => option.value === text || option.aliases?.includes(text),
        )?.value ?? text
      );
    };
    const otherValue = (dimension, candidate) =>
      candidate != null &&
      candidate !== "Unavailable" &&
      (dimension === "status"
        ? candidate < 100 || candidate >= 600
        : !!configuration.filterCatalog[dimension] &&
          !configuration.filterCatalog[dimension].some(
            (option) => option.value === String(candidate).toLowerCase(),
          ));
    const trafficKey = (row, column) =>
      ({
        "request-number": row.requestNumber ?? "0",
        method: row.method === "—" ? null : row.method,
        status: row.status,
        process: row.caller.processName,
        pid: row.caller.processId,
        host: row.host,
        path: row.path,
        url: row.url,
        protocol: row.protocol === "Unavailable" ? row.protocol : canonicalProtocol(row.protocol),
        duration: row.durationMs,
        "http-duration": row.httpDurationMs,
        "response-bytes": row.responseBytes,
        "request-bytes": row.requestBytes,
        state: row.terminal,
        "content-type": row.contentType,
        "started-at": row.startedAt > 0 ? row.startedAt : null,
        "stream-sent": row.streamSent,
        "stream-received": row.streamReceived,
        "traffic-type":
          row.streamProtocol === "web-socket"
            ? "websocket"
            : row.streamProtocol === "sse"
              ? "sse"
              : "http",
        "fetch-destination": row.fetchDestination,
      })[column];
    const numericColumns = new Set([
      "request-number",
      "status",
      "pid",
      "duration",
      "http-duration",
      "response-bytes",
      "request-bytes",
      "started-at",
      "stream-sent",
      "stream-received",
    ]);
    const classificationLabels = (id, key) =>
      state.classificationsById?.[id]?.[key] ?? state.classificationValues ?? ["MS-FSSHTTP"];
    const matchesClassifications = (row, query, exclude) =>
      (query.extensionFilters ?? [])
        .filter((filter) => filter.column !== exclude)
        .every(
          (filter) =>
            !filter.values.length ||
            filter.values.some((value) =>
              value.kind === "unavailable"
                ? classificationLabels(row.id, filter.column).length === 0
                : value.kind === "text" &&
                  classificationLabels(row.id, filter.column).some(
                    (label) => label.toLowerCase() === value.value.toLowerCase(),
                  ),
            ),
        );
    const trafficRows = (query = {}, exclude = null) => {
      const snapshot = query.searchResultId
        ? new Set(
            state.searchResults?.find((result) => result.id === query.searchResultId)?.ids ?? [],
          )
        : null;
      const dismissed = new Set(state.dismissedIds ?? []);
      const number = (value) => BigInt(value ?? 0);
      let rows = structuredClone(state.sessions)
        .map((row) => {
          if (row.streamLifetime?.live && row.quietStartedAt != null) {
            const duration = Date.now() - row.quietStartedAt;
            row.durationMs = duration;
            row.streamLifetime.durationMs = duration;
          }
          return row;
        })
        .filter(
          (row) =>
            matchesClassifications(row, query, exclude) &&
            !dismissed.has(row.id) &&
            (!snapshot || snapshot.has(row.id)) &&
            (!query.search ||
              (row.url + " " + row.caller.processName + " " + row.caller.processId)
                .toLowerCase()
                .includes(query.search.toLowerCase())),
        );
      if (query.incidentWindow) {
        const w = query.incidentWindow,
          intake = state.incidentIntakes?.[w.traceId],
          marker = intake?.markers.find((m) => m.id === w.markerId);
        rows = rows.filter(
          (row) =>
            row.traceId === w.traceId &&
            (!w.markerId ||
              (!!marker &&
                row.startedAt >= marker.unixMillis - w.beforeSeconds * 1000 &&
                row.startedAt <= marker.unixMillis + w.afterSeconds * 1000)) &&
            (!w.relevantHostsOnly ||
              intake?.profile?.relevantHosts.some(
                (h) =>
                  row.host === h.host || (h.includeSubdomains && row.host.endsWith("." + h.host)),
              )),
        );
      }
      if (exclude !== "fetch-destination" && query.fetchDestinations?.length)
        rows = rows.filter((row) => query.fetchDestinations.includes(row.fetchDestination));
      for (const filter of query.filters ?? []) {
        if (filter.column === exclude) continue;
        rows = rows.filter((row) => {
          const value = trafficKey(row, filter.column);
          return filter.operator === "minimum"
            ? value != null && number(value) >= number(filter.value)
            : filter.operator === "maximum"
              ? value != null && number(value) <= number(filter.value)
              : filter.operator === "equals"
                ? String(value).toLowerCase() === filter.value.toLowerCase()
                : String(value).toLowerCase().includes(filter.value.toLowerCase());
        });
      }
      for (const group of query.filterGroups ?? []) {
        if (group.dimension === exclude || !group.values.length) continue;
        rows = rows.filter((row) =>
          group.values.some((value) => {
            const candidate = trafficKey(row, group.dimension);
            return value.kind === "other"
              ? otherValue(group.dimension, candidate)
              : value.kind === "unavailable"
                ? candidate == null || candidate === "Unavailable"
                : value.kind === "status-family"
                  ? Math.floor(row.status / 100) === value.value
                  : value.kind === "number"
                    ? candidate != null && number(candidate) === number(value.value)
                    : String(candidate).toLowerCase() === value.value.toLowerCase();
          }),
        );
      }
      if (query.sort) {
        const { column, direction } = query.sort;
        rows.sort((a, b) => {
          const left = trafficKey(a, column),
            right = trafficKey(b, column);
          const compared = numericColumns.has(column)
            ? left == null
              ? right == null
                ? 0
                : 1
              : right == null
                ? -1
                : number(left) < number(right)
                  ? -1
                  : number(left) > number(right)
                    ? 1
                    : 0
            : String(left).localeCompare(String(right));
          return (direction === "descending" ? -compared : compared) || a.id.localeCompare(b.id);
        });
      }
      return rows;
    };
    const applyResponseFile = (asset, input) => {
      asset.linkedBody =
        input.bodyFileMode === "linked"
          ? {
              path: input.bodyPath,
              originalContentCodings: state.assetCodings ?? [],
              applyOriginalCompression: input.preserveContentEncoding ?? false,
            }
          : null;
      asset.copiedFrom = asset.linkedBody ? null : input.bodyPath;
      asset.display = asset.linkedBody ? "" : (state.bodyFileText ?? "file body");
      asset.rawBody = Array.from(new TextEncoder().encode(asset.display));
      asset.bodyBytes = state.bodyFileBytes ?? 100;
    };

    if (new URL(location.href).searchParams.has("empty")) {
      state.emptyExample = state.sessions[0];
      state.sessions = [];
    }
    const ruleDiagnostics = () => {
      const rules = state.automation?.rules ?? [];
      const earlier = new Map();
      const diagnostics = [];
      for (const rule of [...rules]
        .filter((rule) => rule.request.responseAsset)
        .sort((a, b) => a.priority - b.priority)) {
        const matcher = structuredClone(rule.matcher);
        delete matcher.examples;
        if (matcher.url?.kind === "pattern")
          matcher.url.value.address = matcher.url.value.address.replace(
            /\{[a-zA-Z0-9_]*(:digits|\.\.\.)?\}/g,
            (_, kind) => "{" + (kind ?? "") + "}",
          );
        const key = JSON.stringify(matcher);
        const winner = earlier.get(key);
        if (winner) {
          const asset = (reference) =>
            state.assets?.find((asset) => asset.id + "@" + asset.revision === reference);
          const left = asset(rule.request.responseAsset),
            right = asset(winner.request.responseAsset);
          diagnostics.push({
            ruleId: rule.id,
            supersededBy: winner.id,
            duplicateResponse: left?.display === right?.display && left?.status === right?.status,
          });
        } else if (rule.enabled !== false) earlier.set(key, rule);
      }
      if (state.automation) state.automation.diagnostics = diagnostics;
    };
    const intakeCandidate = (input) => {
      const document = structuredClone(
        state.automation ?? {
          generation: 0,
          rules: [],
          responseSets: [],
          autoresponsesEnabled: true,
          diagnostics: [],
          usage: [],
          candidateCount: 0,
          historyCount: 0,
        },
      );
      document.responseSets ??= [];
      if (document.generation !== input.generation)
        throw new Error("Automation changed. Refresh the collection review.");
      const ids = input.source.ids;
      if (!ids.length || ids.length > 1000 || new Set(ids).size !== ids.length)
        throw new Error("Select 1–1,000 unique responses.");
      const owner =
        input.destination.kind === "existing"
          ? document.responseSets.find((set) => set.id === input.destination.id)
          : null;
      if (
        input.destination.kind === "existing" &&
        (!owner ||
          owner.incarnation !== input.destination.incarnation ||
          owner.revision !== input.destination.revision)
      )
        throw new Error("The destination changed. Refresh the collection review.");
      if (
        input.destination.kind === "new" &&
        (!input.destination.name ||
          new TextEncoder().encode(input.destination.name).length > 128 ||
          new TextEncoder().encode(input.destination.notes).length > 16384)
      )
        throw new Error("Enter a bounded response set name and notes.");
      if ((owner?.rules.length ?? 0) + ids.length > 1000)
        throw new Error("A response set can contain at most 1,000 responses.");
      const owned = new Set(
        document.responseSets.flatMap((set) =>
          [
            ...set.rules,
            ...set.versions.flatMap((version) => version.rules),
            ...(set.applied?.rules ?? []),
            ...(set.undoApply?.applied?.rules ?? []),
          ].map((rule) => rule.id),
        ),
      );
      let sources;
      if (input.source.kind === "rules") {
        sources = ids
          .map((id) => {
            const rule = document.rules.find(
              (rule) => rule.id === id && rule.request.responseAsset && !owned.has(id),
            );
            if (!rule) throw new Error("Select only ordinary saved-response rules.");
            return { sourceId: id, rule };
          })
          .sort((a, b) => a.rule.priority - b.rule.priority || a.rule.id.localeCompare(b.rule.id));
      } else {
        if (input.mode === "move") throw new Error("Captured Traffic is copied.");
        sources = ids.map((id) => {
          const captured = detail(id),
            head = captured.requests[0],
            body = captured.storedBodies.find((body) => body.boundary === "client-response");
          if (state.evictedIds?.includes(id) || body?.availability !== "complete")
            throw new Error("Captured response unavailable.");
          const action = { headers: [], replaceBody: null, discardBody: false, abortReason: null };
          return {
            sourceId: id,
            rule: {
              id,
              displayName: head.method + " " + head.target,
              enabled: true,
              revision: 1,
              priority: 0,
              matcher: {
                method: head.method,
                url: { kind: "exact", value: head.target },
                requestHeaders: [],
                responseHeaders: [],
                scheme: null,
                host: null,
                port: null,
                pathPrefix: null,
                query: null,
                responseStatus: null,
                responseStatusClass: null,
              },
              request: { ...action, responseAsset: null, allowNonIdempotentBodyReplacement: false },
              response: { ...action },
            },
          };
        });
      }
      const rows = sources.map(({ sourceId, rule }) => {
        const asset = state.assets?.find(
          (asset) => asset.id + "@" + asset.revision === rule.request.responseAsset,
        );
        return {
          id: sourceId,
          name: rule.displayName,
          method: rule.matcher.method ?? "ANY",
          url: rule.matcher.url?.value ?? rule.matcher.host ?? "Any URL",
          response: asset
            ? asset.status + " · " + asset.bodyBytes + " bytes"
            : "Complete captured response",
          bodySource: asset?.linkedBody
            ? "Linked file: " + asset.linkedBody.path
            : asset?.copiedFrom
              ? "Copied file: " + asset.copiedFrom
              : input.source.kind === "traffic"
                ? "Copied from retained Traffic"
                : "Saved response bytes",
          included: rule.enabled,
        };
      });
      const warnings = [];
      for (let index = 0; index < sources.length; index++) {
        const rule = sources[index].rule;
        for (const other of [
          ...(owner?.rules ?? []),
          ...sources.slice(0, index).map((source) => source.rule),
        ])
          if (
            JSON.stringify(rule.matcher.url) === JSON.stringify(other.matcher.url) &&
            rule.matcher.method === other.matcher.method
          )
            warnings.push(
              rule.displayName +
                " may overlap " +
                other.displayName +
                ". Both responses will be kept.",
            );
      }
      return {
        document,
        owner,
        sources,
        review: {
          rows,
          warnings,
          activeCount:
            input.source.kind === "rules" && document.autoresponsesEnabled !== false
              ? sources.filter((source) => source.rule.enabled).length
              : 0,
          destinationCount: owner?.rules.length ?? 0,
        },
      };
    };
    const callbacks = new Map();
    const nativeEvents = new Map();
    let nativeEventId = 0;
    state.extensionListenerCount = () =>
      Array.from(nativeEvents.values()).filter((entry) => entry.event === "extensions-changed")
        .length;
    state.emitNative = (event, payload = null) => {
      for (const [id, entry] of nativeEvents)
        if (entry.event === event) callbacks.get(entry.handler)?.({ event, id, payload });
    };
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener(_event, id) {
        const entry = nativeEvents.get(id);
        if (entry) callbacks.delete(entry.handler);
        nativeEvents.delete(id);
      },
    };
    state.callbackCount = () => callbacks.size;
    const finishVisualizerDocument = (view) => {
      if (view.document.updates === "final") {
        view.live.available = false;
        view.live.enabled = false;
      }
      if (view.live.missedHints !== "0" && view.document.status === "complete")
        view.document.status = "partial";
    };
    let callbackId = 0;
    window.__TAURI_INTERNALS__ = {
      transformCallback(callback) {
        const id = ++callbackId;
        callbacks.set(id, callback);
        return id;
      },
      unregisterCallback(id) {
        callbacks.delete(id);
      },
      convertFileSrc(path) {
        return "https://workspace.test/" + path;
      },
      async invoke(command, args) {
        if (state.holdCommand === command) {
          state.holdCommand = null;
          await new Promise((resolve) => (state.finishHeldCommand = resolve));
        }
        state.calls[command] = (state.calls[command] ?? 0) + 1;
        switch (command) {
          case "mcp_summary": {
            const summary = {
              enabled: state.mcp?.enabled ?? false,
              controls: state.mcp?.controls.length ?? 0,
              sensitiveRequests: structuredClone(state.mcpSensitive ?? []),
              activeAgents: structuredClone(state.mcpActiveAgents ?? []),
              sessions: structuredClone(state.mcpInspectionSessions ?? []),
              capturePasswords: structuredClone(state.mcpCapturePasswords ?? []),
            };
            if (state.deferMcpSummary) {
              state.deferMcpSummary = false;
              await new Promise((resolve) => {
                state.finishMcpSummary = () => {
                  state.finishMcpSummary = null;
                  resolve();
                };
              });
            }
            return summary;
          }
          case "mcp_sensitive_decide": {
            if (state.mcpSensitiveFailure) throw new Error(state.mcpSensitiveFailure);
            const approval = state.mcpSensitive.find((entry) => entry.id === args.id);
            if (!approval || approval.state !== "pending")
              throw new Error("This access request expired or changed.");
            if (args.choice === "minute") {
              approval.state = "temporary";
              approval.remainingSeconds = 60;
            } else {
              if (args.choice === "always") {
                const grant = state.mcp.grants.find((grant) => grant.id === approval.grantId);
                grant.omitSensitiveValues = false;
                grant.permissions = [
                  ...new Set([...grant.permissions, "read-bodies", "read-credentials"]),
                ];
                grant.revision++;
              }
              state.mcpSensitive = state.mcpSensitive.filter((entry) => entry.id !== args.id);
            }
            state.emitNative("mcp-access-changed");
            return {
              enabled: state.mcp.enabled,
              controls: 0,
              sensitiveRequests: structuredClone(state.mcpSensitive),
              activeAgents: structuredClone(state.mcpActiveAgents ?? []),
            };
          }
          case "mcp_sensitive_revoke": {
            state.mcpSensitive = state.mcpSensitive.filter((entry) => entry.id !== args.id);
            state.emitNative("mcp-access-changed");
            return {
              enabled: state.mcp.enabled,
              controls: 0,
              sensitiveRequests: structuredClone(state.mcpSensitive),
              activeAgents: structuredClone(state.mcpActiveAgents ?? []),
            };
          }
          case "mcp_review_state":
            return {
              available: location.search.includes("viewer"),
              name: "Agent investigation",
              accessEnded: state.reviewAccessEnded ?? false,
            };
          case "mcp_review_dirty":
            state.reviewDirty = args.dirty;
            return;
          case "mcp_review_publish":
            state.publishedPrivateRule = structuredClone(args);
            return { id: "published", enabled: false, revision: 1 };
          case "mcp_review_session":
            state.reviewedSession = args.sessionId;
            return "viewer-agent";
          case "mcp_end_session":
            state.mcpInspectionSessions = state.mcpInspectionSessions.filter(
              (session) => session.sessionId !== args.sessionId,
            );
            return {
              enabled: true,
              controls: 0,
              sessions: structuredClone(state.mcpInspectionSessions),
            };
          case "mcp_capture_password":
            state.capturePasswordSubmitted = args.password.length > 0;
            state.mcpCapturePasswords = [];
            return {
              enabled: true,
              controls: 0,
              sessions: structuredClone(state.mcpInspectionSessions),
              capturePasswords: [],
            };
          case "mcp_configuration_key": {
            const grant = state.mcp.grants.find(
              (item) => item.id === args.id && item.revision === args.revision && !item.revoked,
            );
            if (!grant || state.mcpConfigurationUnavailable)
              throw new Error(
                "This entry has no recoverable configuration. Replace its key, then copy the new configuration to the agent.",
              );
            return { key: state.mcpSavedKey ?? "tmk_fixture_secret_shown_once" };
          }
          case "mcp_status":
          case "mcp_set_enabled":
          case "mcp_create_key":
          case "mcp_update_key":
          case "mcp_replace_key":
          case "mcp_revoke_key":
          case "mcp_remove_key": {
            state.mcp ??= {
              enabled: false,
              grants: [],
              instanceId: "a".repeat(32),
              endpoint: null,
              cliCommand: "C:\\Transmog\\transmog-cli.exe",
              cliEnvironment: { LOCALAPPDATA: "C:\\Users\\Fixture\\AppData\\Local" },
              controls: [],
              temporaryRules: [],
              activity: [],
              error: null,
            };
            const current = state.mcp;
            if (state.mcpFailure && command !== "mcp_status") throw new Error(state.mcpFailure);
            if (command === "mcp_set_enabled") {
              current.enabled = args.enabled;
              current.endpoint = args.enabled ? "https://127.0.0.1:45678" : null;
            }
            if (command === "mcp_create_key") {
              current.grants.push({
                ...args.input,
                id: `key-${current.grants.length + 1}`,
                revision: 1,
                createdAt: 1700000000,
                lastUsedAt: null,
                revoked: false,
              });
              return { key: "tmk_fixture_secret_shown_once", status: structuredClone(current) };
            }
            if (command === "mcp_update_key") {
              const grant = current.grants.find((grant) => grant.id === args.id);
              Object.assign(grant, args.input, { revision: grant.revision + 1 });
            }
            if (command === "mcp_replace_key") {
              current.grants.find((grant) => grant.id === args.id).revision++;
              state.mcpSavedKey = "tmk_fixture_replacement";
              return { key: "tmk_fixture_replacement", status: structuredClone(current) };
            }
            if (command === "mcp_revoke_key")
              for (const grant of current.grants)
                if (!args.id || grant.id === args.id) {
                  grant.revoked = true;
                  grant.revision++;
                }
            if (command === "mcp_remove_key")
              current.grants = current.grants.filter((grant) => grant.id !== args.id);
            return structuredClone(current);
          }
          case "plugin:event|listen": {
            const id = ++nativeEventId;
            nativeEvents.set(id, { event: args.event, handler: args.handler });
            return id;
          }
          case "plugin:event|unlisten":
            nativeEvents.delete(args.eventId);
            return;
          case "visualizer_choices":
            return structuredClone(
              (state.visualizerChoicesBySession?.[args.selection.sessionId] ??
                state.visualizerChoices)?.[args.selection.surface] ?? [],
            );
          case "extension_traffic_columns": {
            const choices = structuredClone(state.extensionColumnChoices ?? []);
            if (state.deferExtensionColumns) {
              state.deferExtensionColumns = false;
              await new Promise(
                (resolve) =>
                  (state.finishExtensionColumns = () => {
                    state.finishExtensionColumns = null;
                    resolve();
                  }),
              );
            }
            return choices;
          }
          case "classify_traffic_rows": {
            state.classificationRequests ??= [];
            state.classificationRequests.push(structuredClone(args));
            const rows = args.ids.map((id) => ({
              id,
              values: Object.fromEntries(
                args.columns.map((key) => [key, state.classificationValues ?? ["MS-FSSHTTP"]]),
              ),
              limited: !!state.classificationLimited,
            }));
            if (state.deferClassifications) {
              state.deferClassifications = false;
              await new Promise((resolve) => (state.finishClassifications = resolve));
            }
            return rows;
          }
          case "visualizer_page":
            return "1";
          case "inspect_auth": {
            const request = structuredClone(args.request);
            state.authRequests ??= [];
            state.authRequests.push(request);
            if (state.failAuth) throw new Error("Synthetic native inspection failure");
            const inspection = structuredClone(
              state.authBodyInspections?.[request.interpretation] ??
                state.authInspection ?? {
                  findings: [],
                  coverage: [
                    { source: "headers", status: "complete", detail: "All retained occurrences." },
                    {
                      source: "body",
                      status: request.scanBody ? "complete" : "skipped",
                      detail: "Selected-message body only.",
                    },
                  ],
                  diagnostics: [],
                },
            );
            const view = {
              handle: {
                owner: "fixture-owner",
                slot: request.slot,
                generation: request.generation,
                pageId: request.pageId,
                revision: request.sessionId + ":" + request.boundary,
              },
              inspection,
            };
            if (state.deferAuth) {
              state.deferAuth = false;
              await new Promise((resolve) => (state.finishAuth = resolve));
            }
            return view;
          }
          case "cancel_auth":
            state.authCancels = (state.authCancels ?? 0) + 1;
            return;
          case "auth_values":
            return args.fields.map((field) => state.authValues?.[field] ?? "synthetic-secret");
          case "auth_value": {
            const value = state.authValues?.[args.field] ?? "synthetic-secret";
            if (state.deferAuthValue) {
              state.deferAuthValue = false;
              await new Promise((resolve) => (state.finishAuthValue = resolve));
            }
            return value;
          }
          case "auth_converter_input":
            return structuredClone(
              state.authConverterInputs?.[args.finding] ?? {
                text: "private-bearer-value",
                operation: "auth-decode",
              },
            );
          case "auth_source":
            return structuredClone(
              state.authSource ?? {
                sessionId: "first",
                boundary: "client-response",
                source: {
                  kind: "header",
                  label: "WWW-Authenticate · field 1",
                  occurrence: 0,
                  offset: 0,
                  length: 0,
                  transforms: [],
                },
              },
            );
          case "related_auth":
            return structuredClone(
              state.authRelated ?? {
                entries: [],
                searched: 2,
                limited: false,
                detail:
                  "Same capture and authority; other bodies were not read. Connection context does not prove authentication.",
              },
            );
          case "activate_visualizer": {
            const request = structuredClone(args.request);
            state.visualizerStarts ??= [];
            state.visualizerStarts.push(request);
            if (state.visualizerError) throw new Error("Synthetic parser failed");
            if (state.holdVisualizer) {
              state.holdVisualizer = false;
              await new Promise((resolve) => (state.finishVisualizer = resolve));
            }
            state.visualizerClosed ??= {};
            if (BigInt(state.visualizerClosed[request.slot] ?? 0) >= BigInt(request.generation))
              throw new Error("Visualizer closed");
            const handle = {
              slot: request.slot,
              generation: request.generation,
              pageId: request.pageId,
              instanceId: "worker-" + state.visualizerStarts.length,
            };
            const supported = state.visualizerLiveSupported ?? false,
              available = supported && state.visualizerLiveAvailable !== false;
            if (request.live && !available) throw new Error("Live updates unavailable");
            const view = {
              handle,
              instance: {
                id: handle.instanceId,
                extensionId: request.extensionId,
                visualizerId: request.visualizerId,
                digest: "a".repeat(64),
                development: !!state.visualizerDevelopment,
                declaration: {
                  liveUpdates: supported ? { events: ["response"], bodyProgress: false } : null,
                },
              },
              document: structuredClone(
                state.visualizerDocuments?.[request.visualizerId] ?? state.visualizerDocument,
              ),
              sources: structuredClone(state.visualizerSources ?? []),
              live: { available, enabled: !!request.live, missedHints: "0" },
              reloadEpoch: state.reloadEpochs?.[request.extensionId] ?? 0,
              sourceEpoch: state.sourceEpoch ?? 0,
            };
            finishVisualizerDocument(view);
            state.visualizerActive ??= {};
            state.visualizerActive[handle.slot] = view;
            return structuredClone(view);
          }
          case "deactivate_visualizer": {
            state.visualizerClosed ??= {};
            const previous = BigInt(state.visualizerClosed[args.slot] ?? 0);
            if (BigInt(args.generation) > previous)
              state.visualizerClosed[args.slot] = args.generation;
            if (
              state.visualizerActive?.[args.slot] &&
              BigInt(state.visualizerActive[args.slot].handle.generation) <= BigInt(args.generation)
            )
              delete state.visualizerActive[args.slot];
            return;
          }
          case "visualizer_reload_status": {
            const view = state.visualizerActive?.[args.handle.slot];
            if (view?.handle.instanceId !== args.handle.instanceId)
              throw new Error("Visualizer closed");
            return {
              available: !!view.instance.development,
              enabled: !!state.visualizerHotReload,
              dirty: !!state.visualizerHotReload && (state.sourceEpoch ?? 0) > view.sourceEpoch,
              requested:
                (state.reloadEpochs?.[view.instance.extensionId] ?? 0) > view.reloadEpoch
                  ? "1"
                  : null,
              error: null,
            };
          }
          case "reload_extension": {
            state.reloadEpochs ??= {};
            state.reloadEpochs[args.id] = (state.reloadEpochs[args.id] ?? 0) + 1;
            const activeInstances = Object.values(state.visualizerActive ?? {}).filter(
              (view) => view.instance.extensionId === args.id,
            ).length;
            state.emitNative("extension-reload-requested", args.id);
            return { activeInstances };
          }
          case "reload_visualizer": {
            const { handle, generation } = args.request,
              previous = state.visualizerActive?.[handle.slot];
            if (previous?.handle.instanceId !== handle.instanceId)
              throw new Error("Visualizer closed");
            state.visualizerReloads ??= [];
            state.visualizerReloads.push(structuredClone(args.request));
            const epoch = state.reloadEpochs?.[previous.instance.extensionId] ?? 0,
              sourceEpoch = state.sourceEpoch ?? 0;
            previous.reloadEpoch = epoch;
            previous.sourceEpoch = sourceEpoch;
            if (
              state.holdReload &&
              (!state.holdReloadSlot || state.holdReloadSlot === handle.slot)
            ) {
              state.holdReload = false;
              await new Promise((resolve) => (state.finishReload = resolve));
            }
            if (
              state.visualizerActive?.[handle.slot]?.handle.instanceId !== handle.instanceId ||
              BigInt(state.visualizerClosed?.[handle.slot] ?? 0) >= BigInt(generation)
            )
              throw new Error("Visualizer closed");
            if (state.reloadError) throw new Error("Synthetic reload failed");
            const view = structuredClone(previous);
            view.handle = {
              ...handle,
              generation,
              instanceId: "reload-worker-" + state.visualizerReloads.length,
            };
            view.instance.id = view.handle.instanceId;
            view.instance.digest = "b".repeat(64);
            view.reloadEpoch = epoch;
            view.sourceEpoch = sourceEpoch;
            view.document = structuredClone(state.reloadDocument ?? previous.document);
            if (state.reloadDropsLive) view.live.enabled = false;
            state.visualizerActive[handle.slot] = view;
            if (state.reloadEmitsMetadata) state.emitNative("extensions-changed");
            return structuredClone(view);
          }
          case "visualizer_active":
            return (
              state.visualizerActive?.[args.handle.slot]?.handle.instanceId ===
              args.handle.instanceId
            );
          case "expand_visualizer": {
            const view = state.visualizerActive[args.handle.slot];
            if (!view || view.handle.instanceId !== args.handle.instanceId)
              throw new Error("Visualizer closed");
            if (state.holdVisualizerExpansion) {
              state.holdVisualizerExpansion = false;
              await new Promise((resolve) => (state.finishVisualizerExpansion = resolve));
            }
            view.document = structuredClone(
              state.nextExpandedDocument ?? state.expandedVisualizerDocument,
            );
            delete state.nextExpandedDocument;
            finishVisualizerDocument(view);
            return structuredClone(view);
          }
          case "stop_visualizer_live": {
            const view = state.visualizerActive[args.handle.slot];
            if (!view || view.handle.instanceId !== args.handle.instanceId)
              throw new Error("Visualizer closed");
            view.live.enabled = false;
            state.liveStops ??= [];
            state.liveStops.push(structuredClone(args.handle));
            const status = structuredClone(view.live);
            if (state.holdLiveStop) {
              state.holdLiveStop = false;
              await new Promise((resolve) => (state.finishLiveStop = resolve));
            }
            return status;
          }
          case "poll_visualizer_live": {
            const view = state.visualizerActive[args.handle.slot];
            if (!view || view.handle.instanceId !== args.handle.instanceId)
              throw new Error("Visualizer closed");
            if (state.holdLivePoll) {
              state.holdLivePoll = false;
              await new Promise((resolve) => (state.finishLivePoll = resolve));
            }
            if (state.liveError) {
              delete state.visualizerActive[args.handle.slot];
              throw new Error("Synthetic live parser failed");
            }
            view.live.missedHints = state.liveMissedHints ?? "0";
            if (!state.nextLiveDocument)
              return { view: null, status: structuredClone(view.live), busy: false };
            view.document = structuredClone(state.nextLiveDocument);
            delete state.nextLiveDocument;
            finishVisualizerDocument(view);
            return { view: structuredClone(view), status: structuredClone(view.live), busy: false };
          }
          case "visualizer_source": {
            if (
              state.visualizerActive?.[args.handle.slot]?.handle.instanceId !==
              args.handle.instanceId
            )
              throw new Error("Visualizer closed");
            state.visualizerRanges ??= [];
            state.visualizerRanges.push(structuredClone(args));
            const source = state.visualizerSources.find(
              (source) => source.id === args.request.sourceId,
            );
            if (!source) throw new Error("Unknown source");
            const offset = BigInt(args.request.offset),
              count = args.request.maxBytes;
            if (offset + BigInt(count) > BigInt(source.length))
              throw new Error("Read outside source");
            const bytes = Array.from({ length: count }, (_, i) =>
                Number((offset + BigInt(i)) % 256n),
              ),
              chunks = [];
            for (let i = 0; i < bytes.length; i += 8192)
              chunks.push(String.fromCharCode(...bytes.slice(i, i + 8192)));
            return {
              sourceId: source.id,
              totalBytes: source.length,
              offset: args.request.offset,
              bytesBase64: btoa(chunks.join("")),
            };
          }
          case "extension_status": {
            const status = structuredClone(state.extensionStatus ?? { entries: [], warning: null });
            if (state.deferExtensionStatus) {
              state.deferExtensionStatus = false;
              await new Promise((resolve) => (state.finishExtensionStatus = resolve));
            }
            return status;
          }
          case "install_extension":
          case "load_unpacked_extension": {
            if (state.cancelExtensionPicker) {
              state.cancelExtensionPicker = false;
              return null;
            }
            const status = (state.extensionStatus ??= { entries: [], warning: null });
            status.entries.push(structuredClone(state.nextExtension));
            return structuredClone(status);
          }
          case "enable_extension": {
            if (state.extensionSaveError) throw new Error("Extension settings could not be saved.");
            state.extensionStatus.entries.find(
              (entry) => entry.catalog.manifest.id === args.id,
            ).catalog.enabled = args.enabled;
            return structuredClone(state.extensionStatus);
          }
          case "extension_hot_reload": {
            state.extensionStatus.entries.find(
              (entry) => entry.catalog.manifest.id === args.id,
            ).hotReload = args.enabled;
            return structuredClone(state.extensionStatus);
          }
          case "package_extension": {
            if (state.cancelExtensionPicker) {
              state.cancelExtensionPicker = false;
              return null;
            }
            if (state.extensionPackageError) throw new Error("A declared source file is missing.");
            return { path: "C:/extensions/exported.tmext", digest: "b".repeat(64), bytes: 1536 };
          }
          case "export_extension": {
            if (state.cancelExtensionPicker) {
              state.cancelExtensionPicker = false;
              return null;
            }
            if (state.extensionExportError)
              throw new Error("This folder already exists. Choose another destination.");
            return { path: "C:/extensions/" + args.id, files: 2 };
          }
          case "uninstall_extension": {
            state.extensionStatus.entries = state.extensionStatus.entries.filter(
              (entry) => entry.catalog.manifest.id !== args.id,
            );
            if (state.extensionCleanupError)
              throw new Error(
                "The extension was removed but its old package file could not be deleted.",
              );
            return structuredClone(state.extensionStatus);
          }
          case "auth_conversion_catalog":
            return authConversions;
          case "cancel_format_conversion":
            return;
          case "convert_format": {
            const input = structuredClone(args.input);
            if (state.deferConversion) {
              state.deferConversion = false;
              state.conversionPending = true;
              await new Promise((resolve) => (state.finishConversion = resolve));
            }
            if (input.input === "invalid") throw new Error("Invalid conversion input.");
            let bytes =
              input.representation === "hex"
                ? Uint8Array.from(input.input.match(/../g) ?? [], (pair) => parseInt(pair, 16))
                : new TextEncoder().encode(input.input);
            if (input.operation === "base64-encode")
              bytes = new TextEncoder().encode(btoa(String.fromCharCode(...bytes)));
            if (input.operation === "base64-decode")
              bytes = Uint8Array.from(atob(input.input), (character) => character.charCodeAt(0));
            if (input.operation === "jwt-decode") {
              const result = state.jwtConversions?.[input.input];
              if (!result) throw new Error("Expected a complete compact JWT.");
              bytes = new TextEncoder().encode(result);
            }
            if (
              authConversions.some((entry) => entry.id === input.operation) &&
              input.operation !== "jwt-decode"
            ) {
              const result = state.authConversionOutputs?.[input.operation]?.[input.input];
              if (!result) throw new Error("Authentication structure is malformed or unsupported.");
              bytes = new TextEncoder().encode(result);
            }
            let text = null;
            try {
              text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
            } catch {}
            const representation =
              input.outputRepresentation === "hex" || text === null ? "hex" : "text";
            return {
              requiresNextConversion: authConversions.some((entry) => entry.id === input.operation),
              display:
                representation === "hex"
                  ? Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("")
                  : text,
              representation,
              textAvailable: text !== null,
              byteLength: bytes.length,
              encoding: "utf8",
            };
          }
          case "open_converter_window":
            state.converterDraft = structuredClone(args.draft);
            if (state.converterWindowError)
              throw new Error("Converter window could not be opened.");
            return;
          case "save_converter_output":
            state.converterSaved = structuredClone(args.input);
            if (state.converterSaveError) throw new Error("Output could not be saved.");
            return false;
          case "copy_header_field": {
            const header = (state.fullHeaders ?? [
              { name: "Accept", value: "*/*" },
              { name: "Authorization", value: "[redacted]", sensitive: true },
            ])[args.index];
            return {
              name: header.name,
              value: header.sensitive ? null : header.value,
              binary: false,
            };
          }
          case "converter_header_value":
          case "converter_header_input": {
            const header = (state.fullHeaders ?? [
              { name: "Accept", value: "*/*" },
              { name: "Authorization", value: "[redacted]", sensitive: true },
            ])[args.index];
            if (header?.sensitive) throw new Error("This header value was redacted.");
            if (command === "converter_header_value") return header?.value ?? "";
            return {
              text: header?.value ?? "",
              operation: /authentication-info/i.test(header?.name ?? "")
                ? "digest-parse"
                : /auth|access|token/i.test(header?.name ?? "")
                  ? "auth-decode"
                  : null,
            };
          }
          case "open_product_website":
            return;
          case "open_response_set_live_page":
            state.livePageArgs = structuredClone(args);
            if (state.livePageError) throw new Error(state.livePageError);
            return;
          case "app_containers": {
            if (state.loopbackReadError) throw new Error("Fixture Windows Firewall unavailable");
            if (state.deferLoopbackRead) {
              state.loopbackReadPending = true;
              await new Promise((resolve) => (state.finishLoopbackRead = resolve));
              state.deferLoopbackRead = false;
            }
            return structuredClone(state.appContainers ?? []);
          }
          case "set_loopback_exemptions": {
            state.loopbackChange = structuredClone(args.change);
            if (state.deferLoopbackChange) {
              state.loopbackChangePending = true;
              await new Promise((resolve) => (state.finishLoopbackChange = resolve));
              state.deferLoopbackChange = false;
            }
            if (state.loopbackChangeError)
              throw new Error(
                "Windows approval was canceled. No loopback exemptions were changed.",
              );
            for (const row of state.appContainers)
              if (args.change.sids.includes(row.sid)) row.exempt = args.change.enabled;
            return structuredClone(state.appContainers);
          }
          case "watch_update_close":
            state.updateCloseChannel = args.channel;
            return;
          case "set_automatic_update_checks":
            if (state.updatePreferenceError) throw new Error("Fixture preferences cannot be saved");
            state.update.automaticChecksEnabled = args.enabled;
            return structuredClone(state.update);
          case "update_status":
            return structuredClone(state.update);
          case "check_updates": {
            state.update ??= {
              phase: sessionStorage.getItem("startup-update-tested") ? "idle" : "available",
              channel: "Release",
              currentVersion: "0.1.0",
              version: sessionStorage.getItem("startup-update-tested") ? null : "0.2.0",
              notes: "Startup release",
              message: "Release check complete.",
              downloadedBytes: 0,
              totalBytes: null,
              scheduled: false,
              suppressed: false,
              remindAfterUnixMs: 0,
            };
            if (state.deferUpdateCheck) {
              state.updateCheckPending = true;
              await new Promise((resolve) => (state.releaseUpdateCheck = resolve));
              state.deferUpdateCheck = false;
            }
            if (state.updateNetworkError) {
              state.update.phase = "error";
              state.update.message = "Fixture GitHub unavailable";
              throw new Error(state.update.message);
            }
            state.update.lastCheckedUnixMs = Date.now();
            state.update.suppressed = !args.manual && state.update.remindAfterUnixMs > Date.now();
            return structuredClone(state.update);
          }
          case "remind_update":
            state.update.remindAfterUnixMs = Date.now() + 30 * 24 * 60 * 60 * 1000;
            state.update.suppressed = true;
            return structuredClone(state.update);
          case "download_update": {
            state.updateCancelled = false;
            if (state.deferUpdateDownload) {
              state.updateDownloadPending = true;
              await new Promise((resolve) => (state.releaseUpdateDownload = resolve));
            }
            if (state.updateCancelled) return structuredClone(state.update);
            if (state.updateSignatureError)
              throw new Error("Fixture signature verification failed");
            state.update.phase = "ready";
            state.update.scheduled = args.afterSession;
            state.update.message = args.afterSession
              ? "The verified update will install when you quit Transmog. It will not reopen the app."
              : "The verified update is ready to install.";
            return structuredClone(state.update);
          }
          case "cancel_update":
            state.updateCancelled = true;
            state.update.phase = "available";
            state.update.scheduled = false;
            state.releaseUpdateDownload?.();
            return structuredClone(state.update);
          case "install_update": {
            state.updateInstallCalls = (state.updateInstallCalls ?? 0) + 1;
            if (state.updateRestoreError) {
              state.update.phase = "ready";
              throw new Error("Fixture Windows proxy restoration failed");
            }
            state.updateInstallerStarted = true;
            state.updateReopen = args.reopen;
            return;
          }
          case "record_frontend_diagnostic":
            throw new Error("Unexpected frontend diagnostic: " + args.message);
          case "take_opened_traces": {
            const paths = state.openedTraces ?? [];
            state.openedTraces = [];
            return paths;
          }
          case "pick_trace_path":
            if (state.deferTracePicker)
              await new Promise((resolve) => (state.releaseTracePicker = resolve));
            return state.pickedTrace ?? null;
          case "open_trace_viewer":
            state.openedViewer = structuredClone(args.paths);
            return "viewer-fixture";
          case "open_main_window":
            return;
          case "compare_evidence": {
            for (const key of Object.keys(args.input))
              if (!["operationId", "left", "right", "options"].includes(key))
                throw new Error("Unexpected comparison input field: " + key);
            state.comparisonInput = structuredClone(args.input);
            const generation = (state.comparisonGeneration ?? 0) + 1;
            state.comparisonGeneration = generation;
            if (state.deferComparison) {
              state.deferComparison = false;
              await new Promise((resolve) => (state.releaseComparison = resolve));
            }
            const rows = Array.from({ length: 70 }, (_, index) => ({
              index,
              section: "Response",
              field: "Evidence " + index,
              state:
                index === 2 || index === 67 ? "different" : index === 3 ? "incomplete" : "equal",
              left: index === 3 ? "No source body retained" : "<script>inert source</script>",
              right: index === 2 ? "Changed header" : "Same evidence",
              note: index === 3 ? "Incomplete is distinct from empty" : "",
            }));
            return {
              id: "comparison-" + generation,
              left: {
                label: "Request #1",
                source: "fixture-left.saz",
                entryId: "first",
                acquiredAt: 1800000000000,
                boundary: "client-request / client-response",
                notes: [],
              },
              right: {
                label: "Request #2",
                source: "fixture-right.saz",
                entryId: "second",
                acquiredAt: 1800000000001,
                boundary: "client-request / client-response",
                notes: [],
              },
              different: true,
              complete: false,
              changed: 2,
              unknown: 1,
              excluded: 0,
              totalRows: 70,
              moreChanges: false,
              rows: rows.slice(0, 64),
              allRows: rows,
              bodySections: ["Response"],
              options: args.input.options,
            };
          }
          case "comparison_page": {
            const rows = Array.from({ length: 70 }, (_, index) => ({
              index,
              section: "Response",
              field: "Evidence " + index,
              state:
                index === 2 || index === 67 ? "different" : index === 3 ? "incomplete" : "equal",
              left: index === 3 ? "No source body retained" : "Left",
              right: "Right",
              note: "",
            }));
            return {
              rows: rows.slice(args.offset, args.offset + 64),
              nextOffset: args.offset + 64 < rows.length ? args.offset + 64 : null,
            };
          }
          case "comparison_stale":
            return false;
          case "comparison_fields": {
            state.fieldInput = structuredClone(args);
            if (state.deferFields) {
              state.deferFields = false;
              await new Promise((resolve) => (state.releaseFields = resolve));
            }
            return {
              id: args.operationId,
              document: {
                title: "Response · MessagePack field comparison",
                status: "complete",
                updates: "final",
                presentation: state.fieldPresentation ?? "tree",
                summary: "13 field findings · 3 equal parsed fields. Both bodies fully parsed.",
                sources: [],
                diagnostics: [],
                roots: [
                  {
                    id: "field.0",
                    name: "greeting [1] · changed",
                    spans: [],
                    children: [
                      {
                        id: "field.0.left",
                        name: "Left",
                        value: { kind: "string", value: "hello" },
                        spans: [],
                        children: [],
                      },
                      {
                        id: "field.0.right",
                        name: "Right",
                        value: { kind: "string", value: "<script>inert</script>" },
                        spans: [],
                        children: [],
                      },
                      {
                        id: "field.0.evidence",
                        name: "Evidence",
                        spans: [],
                        children: [
                          {
                            id: "field.0.path",
                            name: "Field path",
                            value: {
                              kind: "string",
                              value: '/Value 1 [1]/Key "greeting" [1]/Value [1]',
                            },
                            spans: [],
                            children: [],
                          },
                        ],
                      },
                    ],
                  },
                  ...Array.from({ length: 12 }, (_, index) => ({
                    id: "extra." + index,
                    name: "Additional field " + index + " · changed",
                    value: { kind: "string", value: "Changed value" },
                    spans: [],
                    children: [],
                  })),
                ],
              },
            };
          }
          case "close_comparison":
          case "cancel_comparison":
          case "close_capture_comparison":
            return;
          case "comparison_bytes":
            return {
              left: "00 01",
              right: "00 02",
              offset: args.offset,
              nextOffset: null,
              note: "Frozen original bytes",
            };
          case "reopen_case_capture_comparison":
          case "compare_captures":
          case "capture_comparison_page":
            state.lastPairAction = args.action;
            return {
              id: "capture-comparison",
              leftLabel: "Left fixture snapshot",
              rightLabel: "Right fixture snapshot",
              leftDetails: "Recorded starts unavailable. Stored formats: saz: 2.",
              rightDetails: "Recorded starts unavailable. Stored formats: har: 1.",
              canUndo: true,
              leftCount: 2,
              rightCount: 1,
              changed: 0,
              equalMetadata: 0,
              unresolved: 1,
              leftOnly: 1,
              rightOnly: 0,
              unavailable: 0,
              total: 1,
              rows: [
                {
                  index: 0,
                  left: "first",
                  right: "second",
                  leftLabel: "GET /retry (left)",
                  rightLabel: "GET /retry (right)",
                  reason: "Proposed occurrence order; missing middle request",
                  state: "unresolved",
                  group: "GET /retry",
                },
              ],
              options: args.options ?? {
                excludedQueryNames: [],
                requestHeaders: [],
                requestBodyDigest: false,
              },
              note: "Bodies not compared; only observed within selected scopes.",
            };
          case "capture_comparison_entries":
            return [
              { id: "first", label: "Counterpart 1" },
              { id: "second", label: "Counterpart 2" },
            ];
          case "captured_page_choices": {
            if (state.deferPreviewContext) {
              state.deferPreviewContext = false;
              await new Promise((resolve) => (state.releasePreviewContext = resolve));
            }
            if (state.previewContextError) throw new Error(state.previewContextError);
            const source = state.sessions.find((row) => row.id === args.sourceId);
            return {
              pages: state.sessions
                .filter((row) => row.traceId === source?.traceId && row.contentType === "text/html")
                .map((row) => ({
                  id: row.id,
                  url: row.url,
                  startedAt: row.startedAt,
                  recordedReferrer: false,
                })),
              more: false,
            };
          }
          case "experiment_preview_pages": {
            if (state.deferSetPages && args.includePages !== false) {
              state.deferSetPages = false;
              await new Promise((resolve) => (state.releaseSetPages = resolve));
            }
            const set = state.automation.responseSets?.find((set) => set.id === args.id);
            if (!set) throw new Error("Response set unavailable");
            const pages = state.sessions
              .filter((row) => row.contentType === "text/html")
              .map((row, index) => ({
                capture: state.captureNonce ?? "fixture-capture",
                entryId: row.id,
                requestNumber: row.requestNumber ?? String(index + 1),
                traceId: row.traceId ?? null,
                url: row.url,
                method: row.method ?? "GET",
                startedAt: row.startedAt ?? 1800000000000,
                source: row.traceId ?? "Live traffic",
              }));
            const available =
              !!set.previewPage &&
              pages.some((page) => JSON.stringify(page) === JSON.stringify(set.previewPage));
            const sourceLabel =
              !available &&
              state.retainedSetPage &&
              JSON.stringify(state.retainedSetPage) === JSON.stringify(set.previewPage)
                ? "captured-fixture"
                : null;
            const included = pages.slice(0, 100),
              explicit = pages.find((page) => page.entryId === args.selectedId);
            if (explicit && !included.some((page) => page.entryId === explicit.entryId))
              included.push(explicit);
            return {
              pages: args.includePages === false ? [] : included,
              page: set.previewPage ?? null,
              revision: set.previewPageRevision ?? 0,
              available: available || !!sourceLabel,
              sourceLabel,
              more: pages.length > 100,
            };
          }
          case "set_experiment_preview_page": {
            if (state.deferSetPageSave) {
              state.deferSetPageSave = false;
              await new Promise((resolve) => (state.releaseSetPageSave = resolve));
            }
            const set = state.automation.responseSets?.find((set) => set.id === args.id);
            if (!set) throw new Error("Response set unavailable");
            if ((set.previewPageRevision ?? 0) !== args.revision)
              throw new Error(
                "The page association changed. Review its current page before choosing again.",
              );
            set.previewPage = structuredClone(args.page);
            set.previewPageRevision = args.revision + 1;
            return structuredClone(state.automation);
          }
          case "captured_page_report": {
            if (state.deferPreviewReportLabel === args.label) {
              state.deferPreviewReportLabel = null;
              await new Promise((resolve) => (state.releasePreviewReport = resolve));
            }
            if (state.previewReportFailure === args.label)
              throw new Error("Fixture preview report unavailable");
            if (state.previewReports?.[args.label])
              return structuredClone(state.previewReports[args.label]);
            return {
              caseOrigin: state.previewArgs?.options?.casePreview
                ? "fixture-case@1 / frozen/blue/7"
                : null,
              variant: state.previewArgs?.options?.variant ?? null,
              url: "http://example.test/page",
              scope: "all-loaded",
              source: "fixture.saz",
              scriptsEnabled: true,
              available: 3,
              skipped: 1,
              bytes: 1000,
              hits: 4,
              misses: 2,
              resources: [
                {
                  entryId: "second",
                  source: "fixture.saz",
                  sourceAvailable: true,
                  method: "POST",
                  url: "http://example.test/api",
                  unixMillis: 1800000000000,
                  bytes: 20,
                  decision: "Nearest matching body and Vary variant",
                },
              ],
              requests: [
                {
                  id: 1,
                  method: "GET",
                  url: "http://example.test/missing",
                  outcome: "missing",
                  entryId: null,
                  reason: "No captured response for this method and URL",
                },
              ],
            };
          }
          case "saved_demos": {
            const rows = structuredClone(
                (state.savedDemos ?? []).filter((demo) => demo.ownerId === args.ownerId),
              ),
              error = state.demoListError;
            if (state.deferDemoList) {
              state.deferDemoList = false;
              await new Promise((resolve) => (state.finishDemoList = resolve));
            }
            if (error) throw new Error(error);
            return rows;
          }
          case "prepare_demo_save": {
            state.demoPrepareCount = (state.demoPrepareCount ?? 0) + 1;
            state.demoSaveInput = structuredClone(args.input);
            const row = state.sessions.find((row) => row.id === args.input.entryId);
            if (!row) throw new Error("Captured page is unavailable");
            const report = {
              url: row.url,
              scope: args.input.options.scope,
              source: "fixture.saz",
              sourceTraceId: null,
              scriptsEnabled: null,
              selectedEntryId: row.id,
              runtime: "WebView2",
              bytesAvailable: true,
              rules: [],
              variant: args.input.options.variant,
              caseOrigin: null,
              savedDemo: null,
              available: 2,
              skipped: 3,
              bytes: 512,
              hits: 0,
              misses: 0,
              failures: 0,
              requests: [],
              resources: [
                "304 has no representation body",
                "Response bytes were evicted",
                "Resource exceeds the 32 MiB preview limit",
              ].map((decision, index) => ({
                entryId: "missing-" + index,
                source: "fixture.saz",
                sourceAvailable: false,
                method: "GET",
                url: "https://example.test/unavailable/" + index,
                unixMillis: null,
                bytes: null,
                sha256: null,
                decision,
              })),
            };
            const prepared = {
              token: "demo-token-" + state.demoPrepareCount,
              name: args.input.name,
              replace: args.input.replace ?? null,
              report,
            };
            (state.demoPrepared ??= {})[prepared.token] = {
              prepared: structuredClone(prepared),
              input: structuredClone(args.input),
              byteVersion: state.demoByteVersion ?? "original",
            };
            if (state.deferDemoPrepare) {
              state.deferDemoPrepare = false;
              await new Promise((resolve) => (state.finishDemoPrepare = resolve));
            }
            return prepared;
          }
          case "commit_demo_save": {
            state.demoCommitArgs = structuredClone(args);
            if (state.demoCommitError) throw new Error(state.demoCommitError);
            const staged = state.demoPrepared?.[args.token];
            if (!staged) throw new Error("Prepared demo expired");
            const old = staged.input.replace
              ? (state.savedDemos ?? []).find((demo) => demo.id === staged.input.replace.id)
              : null;
            if (staged.input.replace && old?.revision !== staged.input.replace.revision)
              throw new Error("This saved demo changed or was deleted; reopen Saved demos");
            const demo = {
              id: old?.id ?? "saved-demo-" + state.demoPrepareCount,
              revision: (old?.revision ?? 0) + 1,
              ownerId: staged.input.ownerId,
              ownerIncarnation: staged.input.options.variant.incarnation,
              name: staged.prepared.name,
              createdAt: old?.createdAt ?? Date.now(),
              modifiedAt: Date.now(),
              selection: staged.input.options.variant,
              entryId: staged.input.entryId,
              url: staged.prepared.report.url,
              scope: staged.prepared.report.scope,
              available: 2,
              skipped: 3,
              bytes: 512,
            };
            state.savedDemos = [
              ...(state.savedDemos ?? []).filter((item) => item.id !== demo.id),
              demo,
            ];
            (state.savedDemoReports ??= {})[demo.id] = staged.prepared.report;
            state.demoSavedByteVersion = staged.byteVersion;
            delete state.demoPrepared[args.token];
            return structuredClone(demo);
          }
          case "cancel_demo_save":
            (state.demoCanceledTokens ??= []).push(args.token);
            delete (state.demoPrepared ?? {})[args.token];
            return;
          case "cancel_demo_operation":
            state.demoOperationCanceled = args.operationId;
            return;
          case "delete_saved_demo": {
            state.demoDeleteArgs = structuredClone(args);
            if (state.deferDemoDelete) {
              state.deferDemoDelete = false;
              await new Promise((resolve) => (state.finishDemoDelete = resolve));
            }
            const old = state.savedDemos?.find((item) => item.id === args.demo.id);
            if (old?.revision !== args.demo.revision)
              throw new Error("This saved demo changed or was deleted; reopen Saved demos");
            state.savedDemos = state.savedDemos.filter((item) => item.id !== args.demo.id);
            return;
          }
          case "open_saved_demo": {
            state.openSavedDemoArgs = structuredClone(args);
            const demo = state.savedDemos?.find(
              (item) =>
                item.id === args.demo.id &&
                item.ownerId === args.ownerId &&
                item.revision === args.demo.revision,
            );
            if (!demo) throw new Error("Saved demo unavailable");
            const label = "saved-demo-run-" + (state.demoRunCount = (state.demoRunCount ?? 0) + 1);
            (state.previewReports ??= {})[label] = {
              ...structuredClone(state.savedDemoReports[demo.id]),
              savedDemo: {
                id: demo.id,
                name: demo.name,
                revision: demo.revision,
                ownerId: demo.ownerId,
              },
              scriptsEnabled: args.enableScripts,
            };
            return label;
          }
          case "open_captured_page":
            state.previewArgs = structuredClone(args);
            if (state.deferPagePreview)
              await new Promise((resolve) => (state.releasePagePreview = resolve));
            if (state.pagePreviewCanceled && !state.ignorePreviewCancellation)
              throw new Error("Captured page preview canceled");
            return state.previewRunLabel ?? "captured-fixture";
          case "cancel_captured_page":
            state.pagePreviewCanceled = true;
            if (!state.ignorePreviewCancellation) state.releasePagePreview?.();
            return;
          case "case_preview_choices":
            if (state.deferCaseChoices) {
              state.deferCaseChoices = false;
              await new Promise((resolve) => (state.releaseCaseChoices = resolve));
            }
            return { pages: [{ id: "first", url: "https://example.test/page" }] };
          case "resolve_case_comparison":
            if (state.caseResolveError) throw new Error(state.caseResolveError);
            return {
              kind: args.id === "saved-scopes" ? "captures" : "exchange",
              left: { kind: "exchange", id: "first" },
              right: { kind: "exchange", id: "second" },
              options: { excludedHeaders: ["date", "x-request-id"], normalizeNewlines: true },
            };
          case "case_export_choices": {
            state.caseChoiceScopes ??= [];
            state.caseChoiceScopes.push(structuredClone(args.scope));
            const response = structuredClone(state.caseChoices ?? { choices: [], comparisons: [] });
            response.evidenceSelected = args.scope !== null || args.comparisonIds.length > 0;
            response.requestCount = response.evidenceSelected ? 99 : 0;
            state.caseChoiceComparisons = structuredClone(args.comparisonIds);
            if (!response.evidenceSelected)
              response.choices = response.choices.map((c) => ({ ...c, relevance: "not-assessed" }));
            if (state.deferCaseRelevance) {
              state.deferCaseRelevance = false;
              await new Promise((resolve) => (state.releaseCaseRelevance = resolve));
            }
            if (state.caseRelevanceError) throw new Error(state.caseRelevanceError);
            return response;
          }
          case "prepare_case_export":
            state.caseExportInput = structuredClone(args.input);
            return structuredClone(state.caseReview);
          case "save_open_case":
          case "save_case_export":
            if (state.deferCaseSave) {
              state.deferCaseSave = false;
              await new Promise((resolve) => (state.releaseCaseSave = resolve));
            }
            if (state.caseCanceled) {
              state.caseCanceled = false;
              throw { category: "canceled", message: "Case operation canceled" };
            }
            return "C:/cases/investigation.tmcase.tmcap";
          case "cancel_case_operation":
            state.caseCanceled = true;
            state.releaseCaseSave?.();
            return;
          case "read_case_context":
            return "Original config output <script>inert</script>\r\nDNS=value";
          case "saved_file_is_case":
            return /\.(tmcase|tmrules)\.tmcap$/i.test(args.path);
          case "choose_case":
            return "portable-fixture.tmcase.tmcap";
          case "open_case":
            if (state.caseLocked && args.password !== "fixture")
              throw {
                category: args.password ? "invalid-password" : "password-required",
                message: args.password
                  ? "The package password is incorrect"
                  : "This package requires a password",
              };
            return structuredClone(state.caseReview);
          case "inspect_case_source":
            return {
              links: [
                {
                  reference: {
                    artifactSha256: "fixture-digest",
                    sourceKey: "root",
                    exchangeId: "original",
                    occurrence: "0",
                  },
                  exchangeId: "first",
                },
              ],
              imported: { trace: state.traces?.[0], issues: [] },
            };
          case "review_case_rules":
            return {
              source: {
                rules: [{ id: "rule-a", label: "Keep the exact encoded response" }],
                experiments: [],
              },
              items: [
                {
                  kind: "rule",
                  portableId: "rule-a",
                  state: "conflicting",
                  localId: "fresh-disabled-rule",
                },
              ],
              activation: "disabled",
            };
          case "import_case_rules":
            state.caseImported = true;
            return {};
          case "close_case":
            return;
          case "save_case_working_copy":
            return "C:/cases/plaintext-working-copy";
          case "save_traffic_trace": {
            state.savedTraceArgs = structuredClone({ options: args.options });
            state.saveTraceChannel = args.onProgress;
            if (state.deferTraceSave) {
              state.traceSavePending = true;
              await new Promise((resolve) => (state.releaseTraceSave = resolve));
              state.traceSavePending = false;
            }
            if (state.saveTraceError) throw new Error("Fixture trace write failed");
            if (
              !state.cancelTraceSave &&
              args.options.includeMarkers &&
              ["native", "saz-extended"].includes(args.options.format)
            )
              for (const r of Object.values(state.markerReviews ?? {})) r.dirty = false;
            return state.cancelTraceSave
              ? null
              : {
                  destination: "C:/captures/shared.tmcap",
                  entries: state.sessions.length,
                  bytes: 1234,
                  incompleteBodies: 0,
                  placeholderEntries: state.tracePlaceholders ?? 0,
                  connectionEntries: state.traceConnections ?? 0,
                };
          }
          case "close_trace_window":
            state.traceCloseRequested = true;
            return;
          case "set_marker_draft_dirty":
            state.markerDraftDirty = args.dirty;
            return;
          case "discard_marker_changes":
            for (const r of Object.values(state.markerReviews ?? {})) r.dirty = false;
            return;
          case "trace_marker_reviews": {
            state.markerReviews ??= {};
            return (state.traces ?? []).map((source) => {
              let r = state.markerReviews[source.id];
              if (!r)
                r = state.markerReviews[source.id] = {
                  traceId: source.id,
                  name: source.name,
                  revision: 0,
                  dirty: false,
                  canUndo: false,
                  markers: structuredClone(
                    state.incidentIntakes?.[source.id]?.markers ??
                      source.context?.incidentMarkers ??
                      [],
                  ),
                  editedMarkerIds: [],
                  editedNotes: 0,
                  deletedMarkers: 0,
                };
              return structuredClone(r);
            });
          }
          case "add_trace_marker": {
            state.markerReviews ??= {};
            const row = state.sessions.find((r) => r.id === args.exchangeId),
              traceId = row.traceId ?? "live";
            if (traceId === "live" && !state.traces.some((t) => t.id === traceId))
              state.traces.push({ id: traceId, name: "Live traffic", context: {}, sessions: 1 });
            const r = (state.markerReviews[traceId] ??= {
              traceId,
              name: state.traces.find((t) => t.id === traceId).name,
              revision: 0,
              dirty: false,
              canUndo: false,
              markers: [],
              editedMarkerIds: [],
              editedNotes: 0,
              deletedMarkers: 0,
            });
            state.markerUndo ??= {};
            (state.markerUndo[traceId] ??= []).push(structuredClone(r));
            const marker = {
              id: "analyst-" + r.revision,
              runId: "analyst-run",
              unixMillis: row.startedAt,
              offsetMillis: null,
              origin: "analyst",
              creationKind: "later-annotation",
              note: args.note,
            };
            r.markers.push(marker);
            r.revision++;
            r.dirty = true;
            r.canUndo = true;
            r.links ??= [];
            r.links.push({
              markerId: marker.id,
              anchorId: "anchor-" + row.id,
              boundary: args.boundary,
              timeBasis: "request-start",
            });
            r.markerEntries ??= {};
            r.markerEntries[marker.id] = row.id;
            state.lastAddedMarker = structuredClone(args);
            return structuredClone(r);
          }
          case "change_trace_marker": {
            const r = state.markerReviews[args.traceId];
            if (r.revision !== args.revision)
              throw new Error("Markers changed. Reload them and try again.");
            state.markerUndo ??= {};
            const history = (state.markerUndo[args.traceId] ??= []);
            if (args.change.action === "undo") {
              const previous = history.pop();
              if (previous) {
                const revision = r.revision + 1;
                Object.assign(r, previous, { revision });
              }
            } else {
              history.push(structuredClone(r));
              if (args.change.action === "delete") {
                r.markers = r.markers.filter((m) => m.id !== args.change.id);
                r.deletedMarkers++;
              } else {
                r.markers.find((m) => m.id === args.change.id).note = args.change.note;
                r.editedMarkerIds.push(args.change.id);
                r.editedNotes++;
              }
              r.revision++;
              r.dirty = true;
            }
            r.canUndo = history.length > 0;
            const source = state.traces.find((t) => t.id === args.traceId);
            source.context = { ...source.context, incidentMarkers: structuredClone(r.markers) };
            return structuredClone(r);
          }
          case "incident_intake": {
            if (state.deferIntake) {
              state.deferIntake = false;
              await new Promise((resolve) => (state.releaseIntake = resolve));
            }
            const source = state.traces?.find((t) => t.id === args.traceId);
            const intake = structuredClone(
              state.incidentIntakes?.[args.traceId] ?? {
                traceId: args.traceId,
                source: source?.name ?? "Fixture",
                profile: null,
                markers: [],
                expectedBehavior: "",
                actualBehavior: "",
                enteredClientVersion: "",
                collector: "Fixture collector",
                observedClients: [],
                observedUserAgents: [],
                entries: state.sessions.filter((r) => r.traceId === args.traceId).length,
                incompleteBodies: 0,
                note: "Original source context; clocks do not establish cross-source chronology.",
              },
            );
            const review = state.markerReviews?.[args.traceId];
            if (review) {
              intake.review = structuredClone(review);
              intake.markers = structuredClone(review.markers);
            }
            return intake;
          }
          case "review_traffic_export": {
            const id =
              "export-review-" + (state.reviewGeneration = (state.reviewGeneration ?? 0) + 1);
            let rows = state.sessions;
            if (args.selection?.kind === "selected")
              rows = rows.filter((r) => args.selection.ids.includes(r.id));
            if (args.selection?.kind === "source")
              rows = rows.filter((r) => (r.traceId ?? null) === args.selection.traceId);
            if (args.selection?.kind === "filtered") rows = trafficRows(args.selection.query);
            const result = {
              id,
              entries: rows.length,
              retainedEntries: state.sessions.length,
              sources: ["Customer fixture"],
              sourceCount: 1,
              hosts: ["example.test"],
              firstMillis: 1800000000000,
              lastMillis: 1800000003000,
              incompleteBodies: 1,
              retainedBodyBytes: 512,
              streamRecords: 3,
              retainedStreamBytes: 1024,
              incidentMarkers: 1,
              note: "Entry membership is frozen. Header redaction is not complete anonymization.",
            };
            if (state.deferExportReview) {
              state.deferExportReview = false;
              await new Promise((resolve) => (state.releaseExportReview = resolve));
            }
            state.lastReviewSelection = structuredClone(args.selection);
            return result;
          }
          case "mark_incident":
            state.lastIncidentNote = args.note;
            if (state.deferMarker) {
              state.deferMarker = false;
              await new Promise((resolve) => (state.releaseMarker = resolve));
            }
            return { id: "marker-fixture", unixMillis: 1800000000000 };
          case "trace_metadata_list":
            return structuredClone(state.traces ?? []);
          case "cancel_trace_import":
            state.importCanceled = true;
            return;
          case "import_trace": {
            state.lastImport = structuredClone(args.request);
            state.importCanceled = false;
            if (state.importError) throw new Error(state.importError);
            if (state.importPassword && args.request.password !== state.importPassword)
              throw {
                category: args.request.password ? "invalid-password" : "password-required",
                message: args.request.password
                  ? "The capture password is incorrect"
                  : "This capture requires a password",
              };
            args.onProgress.onmessage({
              operationId: args.request.operationId,
              completed: 1,
              total: 2,
            });
            if (state.deferImport) await new Promise((resolve) => (state.releaseImport = resolve));
            if (state.importCanceled) throw new Error("Trace import canceled");
            const trace = {
              id: "trace-" + (state.traces?.length ?? 0),
              name: args.request.path.split(/[\\/]/).pop(),
              format: "saz",
              path: args.request.path,
              sessions: 1,
              importedAt: Date.now(),
              context: { networkContext: "Captured machine IP configuration" },
              notes: ["Original trace note"],
            };
            const row = {
              ...summary("imported-" + trace.id),
              traceId: trace.id,
              durationMs: null,
              startedAt: Date.now(),
            };
            state.sessions.push(row);
            (state.traces ??= []).push(trace);
            return { trace, issues: [] };
          }
          case "desktop_bootstrap":
            return {
              viewerMode: location.search.includes("viewer"),
              windows: true,
              caCertificatePath: "fixture.pem",
              caPrivateKeyPath: "fixture.key",
              caFilesPresent: true,
              caFilesExist: true,
              caKeyError: null,
              ownedCaSha256: "0".repeat(64),
              ownedCaTrusted: true,
              hostRestorePending: false,
              diagnosticsPath: "fixture.jsonl",
              ...state.caBootstrap,
            };
          case "reset_ca": {
            state.caEvents ??= [];
            state.caEvents.push("reset");
            state.resetCaArgs = structuredClone(args);
            if (state.deferCaReset) {
              state.caResetPending = true;
              await new Promise((resolve) => (state.releaseCaReset = resolve));
              state.deferCaReset = false;
            }
            if (state.caResetError) throw new Error("Fixture CA file deletion failed");
            state.caBootstrap = {
              caFilesPresent: false,
              caFilesExist: false,
              ownedCaSha256: null,
              ownedCaTrusted: false,
            };
            return;
          }
          case "create_ca": {
            state.caEvents ??= [];
            state.caEvents.push("create");
            state.lastCaCreate = structuredClone(args.request);
            if (state.caBootstrap?.caFilesExist || state.caBootstrap?.ownedCaSha256)
              throw new Error("Fixture CA destination or identity already exists");
            state.caBootstrap = {
              caFilesPresent: true,
              caFilesExist: true,
              ownedCaSha256: "1".repeat(64),
              ownedCaTrusted: false,
            };
            return { sha256: "1".repeat(64), certificatePath: args.request.certificatePath };
          }
          case "install_certificate": {
            state.caEvents ??= [];
            state.caEvents.push("install");
            state.lastCaInstall = structuredClone(args);
            if (state.caTrustError) throw new Error("Fixture Windows trust canceled");
            state.caBootstrap = { ...state.caBootstrap, ownedCaTrusted: true };
            return;
          }
          case "client_certificate_status":
            return { supported: true, pending: structuredClone(state.certificatePending ?? []) };
          case "client_certificate_identities":
            return structuredClone(
              (state.certificateIdentities ??= [
                {
                  id: "windows:first",
                  fingerprint: "1".repeat(64),
                  subject: "CN=Client one",
                  issuer: "CN=Fixture issuer",
                  validUntil: "Oct 10 2027 GMT",
                },
                {
                  id: "windows:second",
                  fingerprint: "2".repeat(64),
                  subject: "CN=Client two",
                  issuer: "CN=Fixture issuer",
                  validUntil: "Oct 10 2027 GMT",
                },
              ]),
            );
          case "client_certificate_request": {
            const pending = state.certificatePending?.find((row) => row.id === args.id);
            if (!pending)
              throw new Error(
                "This connection is no longer waiting. It may have closed or timed out.",
              );
            return {
              pending: structuredClone(pending),
              choices: pending.choiceCount
                ? [
                    {
                      id: "windows:first",
                      fingerprint: "1".repeat(64),
                      subject: "CN=Client one",
                      issuer: "CN=Fixture issuer",
                      validUntil: "Oct 10 2027 GMT",
                    },
                    {
                      id: "windows:second",
                      fingerprint: "2".repeat(64),
                      subject: "CN=Client two",
                      issuer: "CN=Fixture issuer",
                      validUntil: "Oct 10 2027 GMT",
                    },
                  ]
                : [],
              tlsVersion: "TLS 1.3",
              acceptedIssuers: 1,
              signatureAlgorithms: ["0x0804"],
            };
          }
          case "decide_client_certificate": {
            if (!state.certificatePending?.some((row) => row.id === args.id))
              throw new Error("This connection is no longer waiting.");
            state.certificateDecision = structuredClone(args);
            state.certificatePending = state.certificatePending.filter((row) => row.id !== args.id);
            return { supported: true, pending: structuredClone(state.certificatePending) };
          }
          case "save_client_certificate_policy": {
            if (state.certificateSaveError)
              throw {
                category: "unavailable",
                message: "Fixture certificate policy persistence failed",
                retryable: true,
              };
            state.certificatePolicy = structuredClone(args.policy);
            localStorage.setItem("certificatePolicy", JSON.stringify(state.certificatePolicy));
            return structuredClone(state.certificatePolicy);
          }
          case "buffer_status":
            return {
              installedRam: 34359738368,
              maxBytes: 17179869184,
              storage: "memory",
              retainedBytes: 0,
            };
          case "product_state": {
            const product = state.savedProduct
              ? {
                  ...structuredClone(state.savedProduct),
                  workspace: structuredClone(state.workspace),
                }
              : {
                  schemaVersion: 6,
                  workspace: structuredClone(state.workspace),
                  preferences: {
                    theme: "system",
                    sessionPageSize: 100,
                    configureSystemProxy: state.configureSystemProxy ?? true,
                  },
                  privacy: {
                    terminateWebsocketsOnClear: false,
                    maxLiveEntries: null,
                    bufferLimit: { mode: "automatic" },
                    retainRequestBodies: true,
                    requestBodyLimit: 25000000,
                    redactSensitiveHeaders: false,
                    retainResponseBodies: true,
                    retainBodySamples: state.recordingDefaultBodies ?? false,
                    rememberRecentArtifacts: false,
                    includePathsInSupportBundles: false,
                  },
                  window: {},
                  recentArtifacts: [],
                };
            product.preferences.networkProfiles = structuredClone(
              state.networkProfiles ?? networkProfiles,
            );
            product.preferences.clientCertificatePolicy = structuredClone(
              state.certificatePolicy ??
                JSON.parse(
                  localStorage.getItem("certificatePolicy") ??
                    '{"default":"noCertificate","rules":[]}',
                ),
            );
            return product;
          }
          case "save_workspace_preferences":
            state.workspace = {
              ...structuredClone(args.preferences),
              responseSetGuide: state.workspace.responseSetGuide ?? "not-offered",
            };
            localStorage.setItem("workspace", JSON.stringify(state.workspace));
            return state.workspace;
          case "update_response_set_guide": {
            if (state.deferGuide) await new Promise((resolve) => (state.finishGuide = resolve));
            if (state.guideFailure) throw new Error("Fixture preference save failed");
            const previous = state.workspace.responseSetGuide ?? "not-offered",
              offered = args.action === "offer" && previous === "not-offered";
            state.workspace.responseSetGuide =
              args.action === "start" ? "started" : offered ? "offered" : previous;
            localStorage.setItem("workspace", JSON.stringify(state.workspace));
            return { history: state.workspace.responseSetGuide, offered };
          }
          case "app_status":
            if (state.statusError) throw new Error("Fixture status unavailable");
            return {
              lifecycle: state.lifecycle,
              listener: state.lifecycle === "running" ? "127.0.0.1:8888" : null,
              summary: "Proxy " + state.lifecycle,
              hostRestorePending: false,
            };
          case "start_proxy":
            if (state.holdProxyStart)
              await new Promise((resolve) => (state.releaseProxyStart = resolve));
            else await new Promise((resolve) => setTimeout(resolve, 80));
            state.lifecycle = "running";
            return {
              lifecycle: "running",
              listener: "127.0.0.1:8888",
              summary: "Proxy running",
              hostRestorePending: false,
            };
          case "stop_application":
            if (state.holdProxyStop)
              await new Promise((resolve) => (state.releaseProxyStop = resolve));
            else await new Promise((resolve) => setTimeout(resolve, 80));
            if (state.testDrain) {
              state.lifecycle = "draining";
              return {
                lifecycle: "draining",
                listener: "127.0.0.1:8888",
                summary: "Finishing 1 active request/connection",
                hostRestorePending: false,
              };
            }
            state.lifecycle = "stopped";
            return {
              lifecycle: "stopped",
              listener: null,
              summary: "Proxy stopped",
              hostRestorePending: false,
            };
          case "resume_application":
            state.lifecycle = "running";
            return {
              lifecycle: "running",
              listener: "127.0.0.1:8888",
              summary: "Proxy resumed",
              hostRestorePending: false,
            };
          case "search_traffic": {
            const request = args.request;
            state.lastContentSearch = structuredClone(request);
            state.searchCanceled = false;
            args.onProgress.onmessage({
              operationId: request.operationId,
              completed: 0,
              total: state.sessions.length,
            });
            if (state.deferSearch) await new Promise((resolve) => (state.releaseSearch = resolve));
            if (state.searchCanceled && !state.ignoreSearchCancel)
              throw new Error("Traffic search canceled");
            const normalize = (text) => {
              text = String(text);
              if (request.ignoreDiacritics) text = text.normalize("NFD").replace(/\p{M}/gu, "");
              return request.caseSensitive ? text : text.toLowerCase();
            };
            const regex =
              request.mode === "regex"
                ? new RegExp(request.pattern, request.caseSensitive ? "u" : "iu")
                : null;
            const match = (text) =>
              regex
                ? regex.test(String(text))
                : normalize(text).includes(normalize(request.pattern));
            const ids = state.sessions
              .filter((row) => !state.dismissedIds?.includes(row.id))
              .filter(
                (row) =>
                  (request.metadata &&
                    match(
                      row.url +
                        " " +
                        row.method +
                        " " +
                        row.caller.processName +
                        " " +
                        row.caller.processId +
                        " " +
                        row.status,
                    )) ||
                  (request.headers && match("X-Support: HeaderNeedle " + row.id)) ||
                  (request.responseBodies &&
                    row.contentType !== "image/webp" &&
                    match(row.searchBody ?? "body for " + row.id)),
              )
              .map((row) => row.id);
            const id = "search-" + (state.searchResults?.length ?? 0);
            (state.searchResults ??= []).push({ id, ids });
            return {
              id,
              operationId: request.operationId,
              ids,
              examined: state.sessions.length,
              binaryBodies: 1,
              unavailableBodies: 0,
            };
          }
          case "traffic_search_entry":
            return {
              entryId: args.id,
              moreMatches: false,
              matches: [
                {
                  boundary: "client-response",
                  field: "Decoded text body",
                  startUtf16: 3,
                  endUtf16: 7,
                  before: "😀 ",
                  matched: "Café",
                  after: " marker <script>globalThis.matchUnsafe=true</script>",
                  shortened: false,
                },
                {
                  boundary: "client-response",
                  field: "Decoded text body",
                  startUtf16: 10,
                  endUtf16: 15,
                  before: "Café / ",
                  matched: "Café",
                  after: " again",
                  shortened: false,
                },
              ],
            };
          case "cancel_traffic_search":
            state.searchCanceled = true;
            return;
          case "matching_traffic_ids":
            return trafficRows(args.query).map((row) => row.id);
          case "extension_traffic_facets": {
            const { query, column } = args.request,
              choice = state.extensionColumnChoices.find((choice) => choice.key === column);
            if (!choice) throw new Error("Extension column unavailable");
            const rows = trafficRows(query, column),
              labels = new Map((choice.values ?? []).map((label) => [label.toLowerCase(), label]));
            const options = Array.from(labels, ([value, label]) => ({
              key: "text:" + value,
              label,
              value: { kind: "text", value },
              count: rows.filter((row) =>
                classificationLabels(row.id, column).some((label) => label.toLowerCase() === value),
              ).length,
            }));
            options.push({
              key: "unavailable",
              label: "No match",
              value: { kind: "unavailable" },
              count: rows.filter((row) => !classificationLabels(row.id, column).length).length,
            });
            state.extensionFacetRequests ??= [];
            state.extensionFacetRequests.push(structuredClone(args.request));
            return { options, matchingEntries: rows.length, omittedEntries: 0 };
          }
          case "traffic_facets": {
            const request = structuredClone(args.request);
            if (state.deferFacets) {
              state.deferFacets = false;
              state.facetPending = true;
              await new Promise((resolve) => (state.releaseFacets = resolve));
              state.facetPending = false;
            }
            if (state.facetError) throw new Error("Fixture facet failure");
            const rows = trafficRows(request.query, request.dimension),
              counts = new Map();
            const add = (key, value, label) => {
              const current = counts.get(key);
              if (current) current.count++;
              else counts.set(key, { key, value, label, count: 1 });
            };
            let omittedEntries = 0;
            for (const row of rows) {
              const candidate = trafficKey(row, request.dimension);
              if (otherValue(request.dimension, candidate))
                add("other", { kind: "other" }, "Other");
              if (candidate == null || candidate === "Unavailable")
                add("unavailable", { kind: "unavailable" }, "Unavailable");
              else if (String(candidate).length > 255) {
                omittedEntries++;
              } else if (numericColumns.has(request.dimension))
                add(
                  "number:" + candidate,
                  { kind: "number", value: String(candidate) },
                  String(candidate),
                );
              else {
                const value = String(candidate).toLowerCase();
                add(
                  "text:" + value,
                  { kind: "text", value },
                  request.dimension === "method" || request.dimension === "protocol"
                    ? value.toUpperCase()
                    : value,
                );
              }
              if (request.dimension === "status" && row.status >= 100 && row.status < 600) {
                const family = Math.floor(row.status / 100);
                add(
                  "status-family:" + family,
                  { kind: "status-family", value: family },
                  family + "xx",
                );
              }
            }
            state.facetRequests ??= [];
            state.facetRequests.push(request);
            const options = [...counts.values()]
                .filter(
                  (option) =>
                    !request.search ||
                    option.label.toLowerCase().includes(request.search.toLowerCase()),
                )
                .sort((a, b) => a.key.localeCompare(b.key)),
              offset = request.offset ?? 0,
              limit = request.limit ?? 100;
            const includedOptions = (request.includeValues ?? []).map((value) => {
              const key =
                value.kind === "unavailable" || value.kind === "other"
                  ? value.kind
                  : value.kind + ":" + String(value.value).toLowerCase();
              return (
                counts.get(key) ?? {
                  key,
                  label: value.kind === "unavailable" ? "Unavailable" : String(value.value),
                  value,
                  count: 0,
                }
              );
            });
            return {
              dimension: request.dimension,
              includedOptions,
              options: options.slice(offset, offset + limit),
              matchingEntries: rows.length,
              omittedEntries,
              totalOptions: options.length,
              nextOffset: offset + limit < options.length ? offset + limit : null,
            };
          }
          case "remove_unselected_traffic_entries": {
            state.dismissedIds ??= [];
            const changed = state.sessions
              .map((row) => row.id)
              .filter((id) => !args.ids.includes(id) && !state.dismissedIds.includes(id));
            state.dismissedIds.push(...changed);
            return changed;
          }
          case "watch_sessions":
            state.channel = args.onEvent;
            return;
          case "query_sessions":
          case "query_traffic_view": {
            if (state.deferResultQuery && args.query.searchResultId) {
              state.deferResultQuery = false;
              state.resultQueryPending = true;
              await new Promise((resolve) => (state.releaseResultQuery = resolve));
            }
            if (
              state.searchQueryFailure &&
              args.query.searchResultId === state.searchResults?.at(-1)?.id
            )
              throw new Error("Fixture result paging failed");
            if (state.queryError) throw new Error("Fixture query failure");
            if (state.deferFocus && args.query.focusId) {
              state.deferFocus = false;
              state.focusPending = true;
              await new Promise((resolve) => (state.releaseFocus = resolve));
            }
            if (state.deferRefresh && !args.query.focusId) {
              state.deferRefresh = false;
              state.refreshPending = true;
              await new Promise((resolve) => (state.releaseRefresh = resolve));
            }
            await new Promise((resolve) => setTimeout(resolve, state.queryDelay));
            state.lastQuery = args.query;
            let rows = trafficRows(args.query);
            const { sort, limit = 100 } = args.query;
            let offset = args.query.offset ?? 0;
            if (args.query.focusId) {
              const index = rows.findIndex((row) => row.id === args.query.focusId);
              if (index < 0) throw new Error("Source no longer in Traffic.");
              offset = Math.floor(index / limit) * limit;
            }
            const page = {
              sessions: rows.slice(offset, offset + limit),
              focusOffset: args.query.focusId ? offset : null,
              totalMatched: rows.length,
              retainedCount: state.sessions.length,
              nextCursor: null,
              evicted: 0,
              sequenceGaps: 0,
              subscriberLag: 0,
            };
            if (command === "query_sessions") return page;
            const id = "view-" + (state.viewSequence = (state.viewSequence ?? 0) + 1),
              view = { id, ids: rows.map((row) => row.id), page };
            (state.trafficViews ??= new Map()).set(id, view);
            state.rowMap = new Map(state.sessions.map((row) => [row.id, row]));
            return view;
          }
          case "release_traffic_view":
            state.trafficViews?.delete(args.viewId);
            return;
          case "traffic_view_rows": {
            const snapshot = state.trafficViews?.get(args.viewId);
            if (!snapshot) throw new Error("Traffic view expired");
            const dismissed = new Set(state.dismissedIds ?? []);
            return snapshot.ids
              .slice(args.offset, args.offset + args.limit)
              .filter((id) => !dismissed.has(id))
              .map((id) => state.rowMap.get(id))
              .filter(Boolean);
          }
          case "traffic_timeline_rows": {
            const view = state.trafficViews?.get(args.viewId);
            if (!view) throw new Error("Timeline view expired");
            if (state.timelineFailure) throw new Error("Fixture timeline failure");
            if (state.deferTimeline) {
              state.deferTimeline = false;
              state.timelinePending = true;
              await new Promise((resolve) => (state.releaseTimeline = resolve));
              state.timelinePending = false;
            }
            const dismissed = new Set(state.dismissedIds ?? []);
            return view.ids
              .slice(args.offset, args.offset + args.limit)
              .filter((id) => !dismissed.has(id))
              .map((id) => state.rowMap.get(id))
              .filter(Boolean)
              .map((row) => ({
                id: row.id,
                requestNumber: row.requestNumber,
                method: row.method,
                url: row.url,
                traceId: row.traceId ?? null,
                startMillis: row.startedAt || null,
                durationMicros:
                  (row.httpDurationMs ?? row.durationMs) == null
                    ? null
                    : (row.httpDurationMs ?? row.durationMs) * 1000,
                responseMicros: row.responseOffset ?? null,
                basis: "Fixture recorded interval",
                terminal: row.terminal,
              }));
          }
          case "composer_source": {
            if (state.sourceDelay)
              await new Promise((resolve) => setTimeout(resolve, state.sourceDelay));
            if (state.composerSourceOverride) return structuredClone(state.composerSourceOverride);
            const request = detail(args.id).requests[0];
            return {
              method: request.method,
              url: request.target,
              headers: request.headers.filter((header) => !header.sensitive),
              body: "",
              bodyAvailable: true,
              notices: [],
            };
          }
          case "copy_message_headers": {
            const head = [...detail(args.id).requests, ...detail(args.id).responses].find(
              (head) => head.boundary === args.boundary,
            );
            return (
              args.boundary === "client-request" && state.fullHeaders
                ? state.fullHeaders
                : head.headers
            )
              .map(
                (header) => header.name + ": " + (header.sensitive ? "[redacted]" : header.value),
              )
              .join("\r\n");
          }
          case "inspect_headers": {
            const head = [...detail(args.id).requests, ...detail(args.id).responses].find(
              (head) => head.boundary === args.boundary,
            );
            let headers = (
              args.boundary === "client-request" && state.fullHeaders
                ? state.fullHeaders
                : head.headers
            ).map((header, index) => ({ ...header, index }));
            if (args.largestFirst)
              headers.sort((a, b) => (b.fieldBytes ?? 0) - (a.fieldBytes ?? 0));
            return {
              summary: head.summary,
              headers: headers.slice(args.offset, args.offset + 512),
              offset: args.offset,
              nextOffset: args.offset + 512 < headers.length ? args.offset + 512 : null,
            };
          }
          case "request_command":
            state.lastRequestCommand = args;
            return state.directCommand
              ? {
                  text: "fixture direct " + args.format,
                  notices: [],
                  bodyFileRequired: false,
                  bodyFileAvailable: true,
                }
              : {
                  text: "fixture generated " + args.format,
                  notices: ["The command needs a request body file."],
                  bodyFileRequired: true,
                  bodyFileAvailable: true,
                };
          case "save_request_body":
            state.savedRequestBody = args;
            return {
              text: "fixture saved body command",
              notices: [],
              bodyFileRequired: true,
              bodyFileAvailable: true,
            };
          case "copy_message_url":
            state.lastCopiedUrl = structuredClone(args);
            return state.completeCopiedUrl ?? detail(args.id).requests[0].target;
          case "copy_all_headers":
            return "GET http://example.test/second HTTP/1.1\r\nAccept: */*\r\n\r\n\r\nHTTP/1.1 200 OK";
          case "inspect_certificates": {
            if (state.deferCertificates)
              await new Promise((resolve) => (state.releaseCertificates = resolve));
            return state.certificateInspection ?? { connections: [], certificates: [] };
          }
          case "stream_page": {
            const query = args.query;
            let rows = (state.streamItems ?? []).filter(
              (row) =>
                (!query.sender || row.message.sender === query.sender) &&
                (query.controls || ["text", "binary", "event"].includes(row.message.kind)) &&
                (!query.search ||
                  new TextDecoder()
                    .decode(new Uint8Array(row.message.payload))
                    .includes(query.search)),
            );
            const total = rows.length;
            const offset = query.tail
              ? Math.max(0, total - query.limit)
              : Math.min(query.offset, Math.max(0, total - 1));
            return {
              total,
              offset,
              rows: rows
                .slice(offset, offset + query.limit)
                .map((row) => ({ ...row, message: { ...row.message, payload: [], raw: null } })),
              summary: detail(query.sessionId).stream,
            };
          }
          case "stream_message": {
            const message = structuredClone(
              state.streamItems.find((row) => row.sequence === args.sequence)?.message,
            );
            if (state.deferStream) await new Promise((resolve) => (state.releaseStream = resolve));
            if (!message) throw new Error("Message was evicted");
            return message;
          }
          case "save_stream_message":
            state.savedStream = structuredClone(args);
            return true;
          case "session_detail":
            if (state.slowDetail && args.id === "first")
              await new Promise((resolve) => setTimeout(resolve, 100));
            if (state.evictedIds?.includes(args.id) || state.dismissedIds?.includes(args.id))
              throw new Error("Session is unavailable or has been evicted");
            return detail(args.id);
          case "save_response_body": {
            state.lastSave = structuredClone(args);
            if (state.holdSaveBody) {
              state.holdSaveBody = false;
              await new Promise((resolve) => (state.finishSaveBody = resolve));
            } else await new Promise((resolve) => setTimeout(resolve, 80));
            if (state.saveError) throw new Error("Fixture file write failed");
            return state.saveCancelled ? null : { fileName: "original.webp", bytes: 4 };
          }
          case "read_body_range": {
            const request = args.request;
            state.lastRangeRequest = structuredClone(request);
            const composer = state.composerResponseDetail?.id === request.sessionId;
            const retained = composer
              ? state.composerBytes
              : (state.hexBytes ?? [0, 0x41, 0xff, 0x20]);
            state.rangeRequests ??= [];
            state.rangeRequests.push(structuredClone(request));
            const total = BigInt(
              !composer && state.hexLargeTotal ? state.hexLargeTotal : retained.length,
            );
            const offset = BigInt(request.offset),
              count = Number(
                total - offset < BigInt(request.maxBytes)
                  ? total - offset
                  : BigInt(request.maxBytes),
              );
            const bytes =
              !composer && state.hexLargeTotal
                ? Array.from({ length: count }, (_, index) =>
                    Number((offset + BigInt(index)) % 256n),
                  )
                : retained.slice(Number(offset), Number(offset) + count);
            if (state.deferRangeOffset === request.offset) {
              state.rangePending = true;
              await new Promise((resolve) => (state.releaseRange = resolve));
            }
            const metadata = {
              lengthKnown: true,
              wireBodyBytes: null,
              ...detail(request.sessionId).storedBodies[0],
            };
            if (state.bodyOverride && !composer) Object.assign(metadata, state.bodyOverride);
            metadata.retainedBytes = Number(total);
            const header = new TextEncoder().encode(
              JSON.stringify({
                metadata,
                totalBytes: total.toString(),
                offset: request.offset,
                readLimit: 16777216,
              }),
            );
            const frame = new Uint8Array(4 + header.length + bytes.length);
            new DataView(frame.buffer).setUint32(0, header.length, true);
            frame.set(header, 4);
            frame.set(bytes, 4 + header.length);
            return frame.buffer;
          }
          case "inspect_body": {
            const composer = state.composerResponseDetail?.id === args.request.sessionId;
            if (
              state.deferBody === args.request.sessionId &&
              ["original-text", "editable"].includes(args.request.representation)
            ) {
              state.bodyPending = true;
              await new Promise((resolve) => (state.releaseBody = resolve));
            }
            if (state.slowBody && args.request.sessionId === "first")
              await new Promise((resolve) => setTimeout(resolve, 100));
            const metadata = detail(args.request.sessionId).storedBodies[0];
            if (state.bodyOverride && !composer) Object.assign(metadata, state.bodyOverride);
            state.lastBodyRequest = structuredClone(args.request);
            if (
              metadata.contentCodings.length &&
              metadata.availability !== "complete" &&
              args.request.decodeContent
            )
              throw new Error("Content decoding needs a complete body");
            if (args.request.representation === "editable") {
              const bytes =
                state.editableBodyBytes ??
                Array.from(new TextEncoder().encode("body for " + args.request.sessionId));
              let display = "",
                textEncoding = null;
              try {
                display = new TextDecoder("utf-8", { fatal: true }).decode(Uint8Array.from(bytes));
                if (!/[\u0000-\u0008\u000b\u000e-\u001f\u007f-\u009f]/u.test(display))
                  textEncoding = "utf-8";
              } catch {}
              return {
                metadata,
                representation: textEncoding ? "original-text" : "bytes",
                decoded: args.request.decodeContent && metadata.contentCodings.length > 0,
                textEncoding,
                display,
                displayBytes: bytes.length,
                bytesBase64: btoa(String.fromCharCode(...bytes)),
                byteOffset: 0,
                truncated: state.editableBodyTruncated ?? false,
                nextOffset: null,
                warning: null,
                previewHandle: null,
                previewMimeType: null,
              };
            }
            const representation =
              args.request.representation === "auto"
                ? metadata.mediaType === "application/json"
                  ? "formatted-json"
                  : metadata.mediaType.startsWith("image/")
                    ? "image"
                    : metadata.mediaType === "application/octet-stream"
                      ? "bytes"
                      : "original-text"
                : args.request.representation === "formatted"
                  ? "formatted-json"
                  : args.request.representation;
            const bytes = composer
              ? state.composerBytes
              : (state.hexBytes ?? [0, 0x41, 0xff, 0x20]);
            const chunks = [];
            if (representation === "bytes")
              for (let start = 0; start < bytes.length; start += 8192)
                chunks.push(String.fromCharCode(...bytes.slice(start, start + 8192)));
            return {
              metadata,
              representation,
              decoded: args.request.decodeContent && metadata.contentCodings.length > 0,
              textEncoding: "utf-8",
              originalText: representation === "formatted-json" ? '{"fixture":true}' : null,
              display:
                representation === "formatted-json"
                  ? '{\n  "fixture": true\n}'
                  : composer
                    ? new TextDecoder().decode(Uint8Array.from(bytes))
                    : "body for " + args.request.sessionId,
              displayBytes: representation === "bytes" || composer ? bytes.length : 4,
              bytesBase64: representation === "bytes" ? btoa(chunks.join("")) : null,
              byteOffset: 0,
              truncated: composer ? false : (state.hexTruncated ?? false),
              nextOffset: null,
              warning: null,
              previewHandle: representation === "image" ? "pixel.png" : null,
              previewMimeType: representation === "image" ? "image/png" : null,
            };
          }
          case "automation_status":
            if (state.deferAutomationStatus) {
              state.deferAutomationStatus = false;
              await new Promise((resolve) => (state.releaseAutomationStatus = resolve));
            }
            return structuredClone(
              state.automation ?? {
                generation: 0,
                rules: [],
                autoresponsesEnabled: true,
                diagnostics: [],
                usage: [],
                candidateCount: 0,
                historyCount: 0,
              },
            );
          case "review_response_set_intake":
            return structuredClone(intakeCandidate(args.input).review);
          case "collect_response_set": {
            const input = args.input,
              { document, owner, sources, review } = intakeCandidate(input);
            if (state.intakeReject)
              throw new Error("Destination save failed. Originals were preserved.");
            if (review.warnings.length && !input.acknowledgeOverlaps)
              throw new Error("Review overlapping matches before adding responses.");
            const set = owner ?? {
              id: input.destination.id,
              incarnation: crypto.randomUUID(),
              name: input.destination.name,
              notes: input.destination.notes,
              revision: 1,
              modifiedAt: Date.now(),
              rules: [],
              versions: [],
              applied: null,
              undoApply: null,
              paused: false,
              previewPage: null,
              previewPageRevision: 0,
            };
            const additions = sources.map(({ sourceId, rule }, index) => {
              const next = {
                ...structuredClone(rule),
                id: "intake-" + crypto.randomUUID(),
                revision: 1,
                priority: -1000000 + set.rules.length + index,
              };
              if (input.source.kind === "traffic") {
                const captured = detail(sourceId),
                  body = new TextEncoder().encode("body for " + sourceId),
                  asset = {
                    id: "intake-asset-" + crypto.randomUUID(),
                    revision: 1,
                    status: captured.responses[0].status,
                    bodyBytes: body.length,
                    sha256: "a".repeat(64),
                    mediaType: captured.storedBodies[0].mediaType,
                    provenance: {
                      kind: "session",
                      exchange_id: sourceId,
                      boundary: "client-response",
                    },
                    headers: [],
                    display: new TextDecoder().decode(body),
                    rawBody: [...body],
                    linkedBody: null,
                    copiedFrom: null,
                  };
                state.assets ??= [];
                state.assets.push(asset);
                next.request.responseAsset = asset.id + "@1";
              }
              return next;
            });
            set.rules.push(...additions);
            set.modifiedAt = Date.now();
            if (owner) set.revision++;
            else document.responseSets.push(set);
            if (input.mode === "move")
              document.rules = document.rules.filter((rule) => !input.source.ids.includes(rule.id));
            for (const owner of document.responseSets) owner.undoApply = null;
            document.generation++;
            state.automation = document;
            return {
              status: structuredClone(document),
              setId: set.id,
              setName: set.name,
              addedCount: additions.length,
            };
          }
          case "set_autoresponses_enabled": {
            state.automation ??= {
              generation: 0,
              rules: [],
              diagnostics: [],
              usage: [],
              built: state.scriptBuilds ?? [],
              paused: state.pausedScripts ?? [],
              runtimes: (state.activeScripts ?? []).map((item) => ({
                scriptId: item.manifest.id,
                revision: item.manifest.revision,
                available: !state.scriptRuntimeError,
                error: state.scriptRuntimeError ?? null,
              })),
              candidateCount: 0,
              historyCount: 0,
            };
            if (state.automation.generation !== args.generation)
              throw new Error("Stale generation");
            state.automation = {
              ...state.automation,
              generation: state.automation.generation + 1,
              autoresponsesEnabled: args.enabled,
            };
            return structuredClone(state.automation);
          }
          case "test_autoresponse_match": {
            state.lastMatcherTest = structuredClone(args.input);
            const evaluate = (input) => {
              const { matcher, url } = input;
              const request = new URL(url);
              const condition = matcher.url;
              let matched = true;
              const checks = [];
              const captures = [];
              if (condition?.kind === "exact")
                matched = new URL(condition.value).href === request.href;
              else if (condition?.kind === "pattern") {
                let previous = 0;
                let expression = "";
                const labels = [];
                for (const token of condition.value.address.matchAll(/\{([^{}]*)\}/g)) {
                  expression += condition.value.address
                    .slice(previous, token.index)
                    .replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
                  expression += token[1].endsWith(":digits")
                    ? "([0-9]+)"
                    : token[1].endsWith("...")
                      ? "([^/?#]+(?:/[^/?#]+)*)"
                      : "([^/?#]+)";
                  labels.push(
                    token[1].replace(/:digits$|\.\.\.$/g, "") || "Part " + (labels.length + 1),
                  );
                  previous = token.index + token[0].length;
                }
                expression += condition.value.address
                  .slice(previous)
                  .replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
                const result = new RegExp(
                  "^" + expression + "$",
                  condition.value.caseSensitive ? "" : "i",
                ).exec(request.origin + request.pathname);
                matched = Boolean(result);
                if (result)
                  labels.forEach((label, index) =>
                    captures.push({ label, value: result[index + 1] }),
                  );
                const query = condition.value.query;
                if (query.kind === "exact")
                  matched &&=
                    query.value === null
                      ? !request.search
                      : query.value === request.search.slice(1);
                if (query.kind === "parameters")
                  matched &&= query.value.every((item) =>
                    request.searchParams.getAll(item.name).includes(item.value),
                  );
              } else if (condition?.kind === "regex") {
                const { pattern, whole, caseSensitive, scope } = condition.value;
                matched = new RegExp(
                  whole ? "^(?:" + pattern + ")$" : pattern,
                  caseSensitive ? "" : "i",
                ).test(scope === "path" ? request.pathname : request.href);
              }
              checks.push({ label: "Address", matched, detail: "Fixture runtime condition" });
              if (matcher.method) {
                const method = matcher.method === input.method;
                checks.push({ label: "Method", matched: method, detail: matcher.method });
                matched &&= method;
              }
              for (const header of matcher.requestHeaders ?? []) {
                const value = input.headers?.find(
                  (item) => item.name.toLowerCase() === header.name.toLowerCase(),
                )?.value;
                const expected =
                  header.condition.kind === "exact"
                    ? new TextDecoder().decode(new Uint8Array(header.condition.value))
                    : null;
                if (expected !== null) matched &&= value === expected;
              }
              return {
                test: { matched, normalizedUrl: request.href, checks, captures },
                wouldServe:
                  matched && input.enabled && state.automation?.autoresponsesEnabled !== false,
                explanation: matched
                  ? input.enabled
                    ? "This rule would serve the saved response."
                    : "The request matches, but this rule is disabled."
                  : "This request does not match the draft rule.",
                winningRule: null,
                examples: [],
              };
            };
            const result = evaluate(args.input);
            result.examples = (args.input.matcher.examples ?? []).map((item, index) => {
              const actual = evaluate({ ...args.input, ...item }).test.matched;
              return {
                index,
                method: item.method,
                url: item.url,
                expected: item.expected,
                matched: actual,
                passed: actual === item.expected,
              };
            });
            if (state.deferNextMatcher && args.input.url !== "https://example.test/") {
              state.deferNextMatcher = false;
              state.matcherPending = true;
              await new Promise((resolve) => (state.releaseMatcher = resolve));
            }
            return result;
          }
          case "autoresponse_source_headers":
            if (state.deferSourceHeaders) {
              state.deferSourceHeaders = false;
              await new Promise((resolve) => (state.finishSourceHeaders = resolve));
            }
            return structuredClone(
              state.rawResponseHeaders ??
                detail(args.id).responses[0].headers.map((header) => ({
                  name: Array.from(new TextEncoder().encode(header.name)),
                  value: Array.from(new TextEncoder().encode(header.value)),
                })),
            );
          case "response_assets":
            return structuredClone(state.assets ?? []);
          case "inspect_response_asset": {
            if (state.deferAssetInspection) {
              state.deferAssetInspection = false;
              await new Promise((resolve) => (state.releaseAssetInspection = resolve));
            }
            const asset = state.assets.find(
              (asset) => asset.id + "@" + asset.revision === args.reference,
            );
            if (!asset) throw new Error("Saved response missing");
            const charset = asset.mediaType?.match(/charset=([^;\s]+)/i)?.[1] ?? "utf-8";
            const display = asset.display ?? "saved body",
              bytes =
                asset.rawBody ??
                (charset === "utf-16le"
                  ? Array.from(display).flatMap((char) => [
                      char.charCodeAt(0) & 255,
                      char.charCodeAt(0) >> 8,
                    ])
                  : Array.from(new TextEncoder().encode(display)));
            const unavailable = asset.linkedBody || state.assetBodyUnavailable;
            return {
              asset: structuredClone(asset),
              display: unavailable ? "" : display,
              textEncoding: unavailable ? null : state.assetTextEncoding === null ? null : charset,
              bytesBase64: unavailable ? null : btoa(String.fromCharCode(...bytes)),
              contentCodings: state.assetCodings ?? asset.linkedBody?.originalContentCodings ?? [],
              explanation: "Edits create a new saved response revision.",
            };
          }
          case "edit_response_asset": {
            if (state.deferAssetEdit) {
              state.assetEditPending = true;
              await new Promise((resolve) => (state.finishAssetEdit = resolve));
            }
            if (state.assetError) throw new Error("Fixture asset write failed");
            const source = state.assets.find(
              (asset) => asset.id + "@" + asset.revision === args.input.assetReference,
            );
            const revision =
              Math.max(
                ...state.assets
                  .filter((asset) => asset.id === source.id)
                  .map((asset) => asset.revision),
              ) + 1;
            const charset = args.input.mediaType?.match(/charset=([^;\s]+)/i)?.[1] ?? "utf-8";
            const asset = {
              ...source,
              revision,
              status: args.input.status,
              headers: args.input.headers,
              mediaType: args.input.mediaType,
              display: args.input.decodedBody
                ? new TextDecoder(charset).decode(new Uint8Array(args.input.decodedBody))
                : source.display,
            };
            if (args.input.bodyPath) {
              applyResponseFile(asset, args.input);
            } else if (args.input.decodedBody !== null) {
              asset.linkedBody = null;
              asset.copiedFrom = null;
              asset.rawBody = structuredClone(args.input.decodedBody);
              asset.bodyBytes = asset.rawBody.length;
            }
            state.assets.push(asset);
            state.lastAssetEdit = structuredClone(args.input);
            return structuredClone(asset);
          }
          case "pick_response_body":
            return state.bodyFile ?? null;
          case "response_body_file_info":
            if (
              state.deferFileInfo ||
              (state.deferFileInfoAfterSetSave &&
                state.calls.change_response_set >= state.deferFileInfoAfterSetSave)
            ) {
              state.deferFileInfo = false;
              state.deferFileInfoAfterSetSave = null;
              await new Promise((resolve) => (state.finishFileInfo = resolve));
            }
            if (state.bodyFileError)
              throw new Error(
                "Response file is missing or unreadable. Restore access or choose another file.",
              );
            return {
              path: args.path,
              bytes: state.bodyFileBytes ?? 100,
              error: state.lastFileFailure ?? null,
            };
          case "cancel_requests": {
            state.lastCanceledIds = structuredClone(args.ids);
            if (state.cancelRequestError) throw new Error("Fixture cancellation failed");
            const canceled = state.sessions.filter(
              (row) => args.ids.includes(row.id) && row.terminal === "active" && !row.traceId,
            );
            for (const row of canceled) {
              row.terminal = row.streamProtocol ? "interrupted" : "cancelled";
              if (state.streams?.[row.id]) state.streams[row.id].state.status = "interrupted";
            }
            return canceled.map((row) => row.id);
          }
          case "clear_traffic": {
            const keep = (row) =>
              row.terminal === "active" &&
              row.streamProtocol === "web-socket" &&
              !row.traceId &&
              !state.savedProduct?.privacy.terminateWebsocketsOnClear;
            const ids = state.sessions.filter((row) => !keep(row)).map((row) => row.id);
            state.dismissedIds = [...ids];
            return {
              ids,
              preservedWebSockets: state.sessions.filter(keep).length,
              bytes: state.clearBytes ?? 0,
              undoable: (state.clearBytes ?? 0) < 1000000000,
              undoSeconds:
                (state.clearBytes ?? 0) >= 100000000 && (state.clearBytes ?? 0) < 1000000000
                  ? 300
                  : null,
            };
          }
          case "remove_traffic_entries": {
            const dismissed = new Set(state.dismissedIds ?? []);
            const changed = [];
            const retained = new Set(
              state.sessions
                .filter(
                  (row) =>
                    args.restore ||
                    !(
                      row.terminal === "active" &&
                      row.streamProtocol === "web-socket" &&
                      !row.traceId &&
                      !state.savedProduct?.privacy.terminateWebsocketsOnClear
                    ),
                )
                .map((row) => row.id),
            );
            for (const id of args.ids)
              if (retained.has(id) && (args.restore ? dismissed.has(id) : !dismissed.has(id))) {
                if (args.restore) dismissed.delete(id);
                else dismissed.add(id);
                changed.push(id);
              }
            state.dismissedIds = [...dismissed];
            return changed;
          }
          case "create_autoresponse_batch": {
            if (state.batchError) throw new Error("Fixture batch storage failed");
            if (state.automation.generation !== args.input.generation)
              throw new Error("Rules changed during batch review");
            const rules = [],
              createdIds = [];
            state.assets ??= [];
            for (const [index, id] of args.input.ids.entries()) {
              if (state.evictedIds?.includes(id) || state.dismissedIds?.includes(id))
                throw new Error("Response was evicted during review");
              const assetId = "batch-asset-" + state.assets.length;
              const asset = {
                id: assetId,
                revision: 1,
                status: detail(id).responses[0].status,
                headers: [],
                display: "body for " + id,
                bodyBytes: 4,
                sha256: "0".repeat(64),
                mediaType: "application/json",
                provenance: { kind: "session", exchange_id: id, boundary: "client-response" },
              };
              state.assets.push(asset);
              const ruleId = "batch-rule-" + assetId;
              createdIds.push(ruleId);
              const matcher = {
                method: "GET",
                url: { kind: "exact", value: detail(id).requests[0].target },
                scheme: null,
                host: null,
                port: null,
                pathPrefix: null,
                query: null,
                requestHeaders: [],
                responseHeaders: [],
                responseStatus: null,
                responseStatusClass: null,
              };
              rules.push({
                id: ruleId,
                displayName: "GET " + detail(id).requests[0].target,
                enabled: true,
                revision: 1,
                priority: -1000000 + index,
                matcher,
                request: {
                  headers: [],
                  replaceBody: null,
                  discardBody: false,
                  abortReason: null,
                  responseAsset: assetId + "@1",
                  allowNonIdempotentBodyReplacement: false,
                },
                response: { headers: [], replaceBody: null, discardBody: false, abortReason: null },
              });
            }
            const existing = [...state.automation.rules]
              .sort((a, b) => a.priority - b.priority)
              .map((rule, index) => ({
                ...rule,
                priority: -1000000 + rules.length + index,
                revision: rule.revision + 1,
              }));
            state.automation = {
              ...state.automation,
              generation: state.automation.generation + 1,
              rules: [...rules, ...existing],
            };
            ruleDiagnostics();
            return { status: structuredClone(state.automation), createdIds };
          }
          case "import_response_asset":
          case "create_response_asset":
          case "create_response_asset_from_session": {
            if (state.assetDelay)
              await new Promise((resolve) => setTimeout(resolve, state.assetDelay));
            if (state.assetError) throw new Error("Fixture asset write failed");
            const input = args.input;
            const sourceHeaders =
              command === "create_response_asset_from_session"
                ? (state.rawResponseHeaders ??
                  detail(input.exchangeId).responses[0].headers.map((header) => ({
                    name: Array.from(new TextEncoder().encode(header.name)),
                    value: Array.from(new TextEncoder().encode(header.value)),
                  })))
                : [];
            const asset = {
              id: input.id,
              revision: input.revision,
              status: input.status ?? 200,
              headers: input.headers ?? sourceHeaders,
              display: input.body
                ? new TextDecoder().decode(new Uint8Array(input.body))
                : "body for " + input.exchangeId,
              bodyBytes: input.body?.length ?? 4,
              sha256: "0".repeat(64),
              mediaType: input.mediaType ?? "application/json",
              provenance:
                command === "import_response_asset"
                  ? { kind: "imported" }
                  : command === "create_response_asset"
                    ? { kind: "authored" }
                    : { kind: "session", exchange_id: input.exchangeId, boundary: input.boundary },
            };
            asset.rawBody = structuredClone(
              input.body ??
                input.decodedBody ??
                state.editableBodyBytes ??
                Array.from(new TextEncoder().encode(asset.display)),
            );
            if (input.decodedBody) {
              asset.display = new TextDecoder().decode(new Uint8Array(input.decodedBody));
              asset.bodyBytes = input.decodedBody.length;
            }
            if (input.bodyPath) applyResponseFile(asset, input);
            (state.assets ??= []).push(asset);
            state.lastAsset = structuredClone(input);
            return asset;
          }
          case "validate_automation":
            if (state.uaRejectValidation) throw new Error("Fixture validation failed");
            state.candidate = structuredClone(args.document);
            return { candidateId: "fixture-candidate" };
          case "test_response_transform": {
            if (state.holdHeaderTest) {
              state.holdHeaderTest = false;
              await new Promise((resolve) => (state.finishHeaderTest = resolve));
            }
            const { sample, body } = args.input,
              rule = sample.rule,
              op = rule.response.textTransform;
            state.lastTextTest = structuredClone(args.input);
            if (!op.find) throw new Error("Find text must not be empty");
            const decoded = (h) => new TextDecoder().decode(new Uint8Array(h)),
              types = sample.responseHeaders
                .filter((h) => decoded(h.name).toLowerCase() === "content-type")
                .map((h) => decoded(h.value));
            const matched =
              (!rule.matcher.method || rule.matcher.method === sample.method) &&
              (!rule.matcher.url ||
                rule.matcher.url.kind !== "exact" ||
                rule.matcher.url.value === sample.url) &&
              (!rule.matcher.responseStatus ||
                rule.matcher.responseStatus === sample.responseStatus) &&
              types.some((t) => t.split(";")[0] === op.mediaType);
            let afterText = body,
              report = null;
            if (matched) {
              const matches = body.split(op.find).length - 1,
                replacements = op.occurrences === "first" ? Math.min(1, matches) : matches;
              let outcome = matches ? "applied" : "not-found";
              if (op.expectedMatches !== null && op.expectedMatches !== matches)
                outcome = "count-mismatch";
              else if (matches)
                afterText =
                  op.occurrences === "all"
                    ? body.split(op.find).join(op.replacement)
                    : body.slice(0, body.indexOf(op.find)) +
                      op.replacement +
                      body.slice(body.indexOf(op.find) + op.find.length);
              const bytes = (s) => new TextEncoder().encode(s),
                hash = async (s) =>
                  [...new Uint8Array(await crypto.subtle.digest("SHA-256", bytes(s)))]
                    .map((b) => b.toString(16).padStart(2, "0"))
                    .join("");
              report = {
                outcome,
                matches,
                replacements: outcome === "count-mismatch" ? 0 : replacements,
                beforeBytes: bytes(body).length,
                afterBytes: outcome === "count-mismatch" ? null : bytes(afterText).length,
                beforeSha256: await hash(body),
                afterSha256: outcome === "count-mismatch" ? null : await hash(afterText),
              };
            }
            return {
              test: {
                matched,
                normalizedUrl: sample.url,
                checks: [{ label: "Draft conditions", matched, detail: "Fixture sample" }],
                captures: [],
              },
              report,
              beforeText: body,
              afterText,
            };
          }
          case "test_header_rule": {
            state.lastHeaderTest = structuredClone(args.input);
            if (state.holdHeaderTest) {
              state.holdHeaderTest = false;
              await new Promise((resolve) => (state.finishHeaderTest = resolve));
            }
            const { rule, method, url, requestHeaders, responseStatus, responseHeaders } =
                args.input,
              phase = rule.request.headers.length ? "request" : "response";
            if (
              rule.request.headers
                .concat(rule.response.headers)
                .some((op) =>
                  [
                    "host",
                    "content-length",
                    "transfer-encoding",
                    "content-encoding",
                    "connection",
                  ].includes(
                    op.operation === "remove"
                      ? op.field.toLowerCase()
                      : new TextDecoder().decode(new Uint8Array(op.field.name)).toLowerCase(),
                  ),
                )
            )
              throw new Error(
                "Protected header: routing, framing and body coding are managed by the proxy",
              );
            const matched =
              (!rule.matcher.method || rule.matcher.method === method) &&
              (!rule.matcher.url ||
                rule.matcher.url.kind !== "exact" ||
                rule.matcher.url.value === url) &&
              (phase === "request" ||
                !rule.matcher.responseStatus ||
                rule.matcher.responseStatus === responseStatus);
            const before = structuredClone(phase === "request" ? requestHeaders : responseHeaders);
            let after = structuredClone(before);
            const operations = [];
            if (matched)
              for (const [index, op] of (phase === "request"
                ? rule.request.headers
                : rule.response.headers
              ).entries()) {
                const name =
                    op.operation === "remove"
                      ? op.field
                      : new TextDecoder().decode(new Uint8Array(op.field.name)),
                  same = (f) =>
                    new TextDecoder().decode(new Uint8Array(f.name)).toLowerCase() ===
                    name.toLowerCase(),
                  previous = JSON.stringify(after);
                if (op.operation === "append") after.push(structuredClone(op.field));
                else {
                  const first = after.findIndex(same);
                  after = after.filter((f) => !same(f));
                  if (
                    op.operation === "set" ||
                    (op.operation === "override-if-present" && first >= 0)
                  )
                    after.splice(first < 0 ? after.length : first, 0, structuredClone(op.field));
                }
                operations.push({
                  index: index + 1,
                  operation:
                    op.operation === "override-if-present"
                      ? "Override if present"
                      : op.operation[0].toUpperCase() + op.operation.slice(1),
                  name,
                  changed: JSON.stringify(after) !== previous,
                });
              }
            return {
              test: {
                matched,
                normalizedUrl: url,
                checks: [{ label: "Draft conditions", matched, detail: "Fixture sample" }],
                captures: [],
              },
              before,
              after,
              operations,
            };
          }
          case "change_response_transform":
          case "change_header_rule": {
            state.lastHeaderChange = structuredClone(args.change);
            if (state.holdHeaderSave) {
              state.holdHeaderSave = false;
              await new Promise((resolve) => (state.finishHeaderSave = resolve));
            }
            if (state.headerReject)
              throw new Error("Fixture validation failed. Previous live rules were kept.");
            if (args.generation !== state.automation.generation)
              throw new Error("Automation changed. Refresh rules; the draft is preserved.");
            const status = structuredClone(state.automation),
              c = args.change;
            if (c.action === "save") {
              const old = status.rules.find((r) => r.id === c.rule.id);
              if ((old?.revision ?? null) !== c.expectedRevision)
                throw new Error(
                  "This header rule changed while you were editing. Your draft is preserved.",
                );
              const saved = {
                ...structuredClone(c.rule),
                revision: (old?.revision ?? 0) + 1,
                priority:
                  old?.priority ??
                  Math.max(
                    -1,
                    ...status.rules
                      .filter((r) => r.id.startsWith("header-"))
                      .map((r) => r.priority),
                  ) + 1,
              };
              if (c.placement === "before")
                saved.priority =
                  Math.min(
                    0,
                    ...status.rules.filter((r) => r.id !== saved.id).map((r) => r.priority),
                  ) - 1;
              if (c.placement === "after")
                saved.priority =
                  Math.max(
                    0,
                    ...status.rules.filter((r) => r.id !== saved.id).map((r) => r.priority),
                  ) + 1;
              if (old) status.rules[status.rules.indexOf(old)] = saved;
              else status.rules.push(saved);
            } else if (c.action === "remove") {
              const old = status.rules.find((r) => r.id === c.id);
              if (old?.revision !== c.expectedRevision) throw new Error("This rule changed");
              status.rules = status.rules.filter((r) => r.id !== c.id);
            } else if (c.action === "restore") {
              if (status.rules.some((r) => r.id === c.rule.id)) throw new Error("Identity in use");
              status.rules.push({ ...structuredClone(c.rule), revision: c.rule.revision + 1 });
            } else if (c.action === "move") {
              const ordered = status.rules
                  .filter((r) => r.id.startsWith("header-"))
                  .sort((a, b) => a.priority - b.priority),
                index = ordered.findIndex((r) => r.id === c.id),
                other = ordered[index + (c.earlier ? -1 : 1)];
              if (other) {
                [ordered[index].priority, other.priority] = [
                  other.priority,
                  ordered[index].priority,
                ];
                ordered[index].revision++;
                other.revision++;
              }
            }
            status.generation++;
            state.automation = status;
            return structuredClone(status);
          }
          case "response_set_overlaps":
            return structuredClone(state.variantOverlaps ?? []);
          case "change_response_set": {
            if (state.setReject) throw new Error("Fixture response set save failed");
            if (state.deferSetSave && args.change.action === "save-current") {
              state.deferSetSave = false;
              state.setSavePending = true;
              await new Promise((resolve) => (state.finishSetSave = resolve));
            }
            if (args.generation !== state.automation.generation)
              throw new Error("Automation changed. Your draft has been preserved.");
            const next = structuredClone(state.automation),
              c = args.change;
            next.responseSets ??= [];
            const set = next.responseSets.find((set) => set.id === c.id);
            if (c.action === "create")
              next.responseSets.push({
                id: c.id,
                incarnation: crypto.randomUUID(),
                name: c.name,
                notes: c.notes,
                revision: 1,
                modifiedAt: Date.now(),
                rules: structuredClone(c.rules),
                versions: [],
                applied: null,
                undoApply: null,
                paused: false,
                previewPage: null,
                previewPageRevision: 0,
              });
            else if (c.action === "save-current") {
              if (!set || set.revision !== c.expectedRevision)
                throw new Error("Response set changed.");
              Object.assign(set, {
                name: c.name,
                notes: c.notes,
                rules: structuredClone(c.rules),
                revision: set.revision + 1,
                modifiedAt: Date.now(),
              });
            } else if (c.action === "save-version") {
              if (!set || set.revision !== c.expectedRevision)
                throw new Error("Response set changed.");
              set.versions.push({
                id: c.versionId,
                name: c.name,
                revision: set.revision,
                rules: structuredClone(set.rules),
              });
            } else if (c.action === "restore-version") {
              if (!set || set.revision !== c.expectedRevision)
                throw new Error("Response set changed.");
              const version = set.versions.find((version) => version.id === c.versionId);
              if (!version) throw new Error("Version unavailable");
              set.rules = structuredClone(version.rules);
              set.revision++;
              set.modifiedAt = Date.now();
            } else if (c.action === "delete-version") {
              if (!set?.versions.some((version) => version.id === c.versionId))
                throw new Error("Version unavailable");
              set.versions = set.versions.filter((version) => version.id !== c.versionId);
            } else if (c.action === "apply") {
              if (!set || set.revision !== c.expectedRevision)
                throw new Error("Response set changed.");
              if (state.variantOverlaps?.length && !c.acknowledgeOverlaps)
                throw new Error("Review overlapping rules");
              set.undoApply = {
                generation: next.generation + 1,
                applied: structuredClone(set.applied),
                paused: set.paused,
              };
              const owned = new Set(set.applied?.rules.map((rule) => rule.id) ?? []);
              next.rules = next.rules.filter((rule) => !owned.has(rule.id));
              set.applied = { revision: set.revision, rules: structuredClone(set.rules) };
              next.rules.push(...structuredClone(set.rules));
              if (c.resume) set.paused = false;
            } else if (c.action === "pause") {
              if (!set) throw new Error("Set unavailable");
              set.paused = c.paused;
            } else if (c.action === "undo-apply") {
              if (!set?.undoApply || set.undoApply.generation !== next.generation)
                throw new Error("Undo is no longer available");
              const undo = set.undoApply,
                owned = new Set(set.applied?.rules.map((rule) => rule.id) ?? []);
              next.rules = next.rules.filter((rule) => !owned.has(rule.id));
              set.applied = structuredClone(undo.applied);
              set.paused = undo.paused;
              next.rules.push(...structuredClone(set.applied?.rules ?? []));
              set.undoApply = null;
            } else if (c.action === "delete") {
              if (!set) throw new Error("Set unavailable");
              const owned = new Set(set.applied?.rules.map((rule) => rule.id) ?? []);
              next.rules = next.rules.filter((rule) => !owned.has(rule.id));
              next.responseSets = next.responseSets.filter((item) => item.id !== set.id);
              state.savedDemos = (state.savedDemos ?? []).filter((demo) => demo.ownerId !== set.id);
            } else throw new Error("Unknown response set action");
            for (const item of next.responseSets)
              if (item.undoApply?.generation !== next.generation + 1) item.undoApply = null;
            next.generation++;
            state.automation = next;
            state.lastSetChange = structuredClone(c);
            ruleDiagnostics();
            return structuredClone(next);
          }
          case "activate_automation":
            if (state.deferExperimentActivation) {
              state.deferExperimentActivation = false;
              await new Promise((resolve) => (state.finishExperimentActivation = resolve));
            }
            state.automation = {
              ...structuredClone(state.candidate),
              responseSets: state.automation?.responseSets ?? [],
              generation: (state.automation?.generation ?? 0) + 1,
              built: state.scriptBuilds ?? [],
              paused: state.pausedScripts ?? [],
              runtimes: (state.activeScripts ?? []).map((item) => ({
                scriptId: item.manifest.id,
                revision: item.manifest.revision,
                available: !state.scriptRuntimeError,
                error: state.scriptRuntimeError ?? null,
              })),
              candidateCount: 0,
              historyCount: 0,
            };
            ruleDiagnostics();
            return structuredClone(state.automation);
          case "save_network_profiles": {
            if (state.networkSaveError) throw new Error(state.networkSaveError);
            if (state.delayNetworkSave)
              await new Promise((resolve) => {
                state.resolveNetworkSave = resolve;
              });
            const catalog = state.networkProfiles ?? structuredClone(networkProfiles);
            if (args.expectedRevision !== catalog.revision)
              throw new Error(
                "Network profiles changed in another window. Reload profiles before saving; your draft is still available.",
              );
            const profiles = args.profiles.map((profile) => {
              const old = catalog.customProfiles.find((item) => item.id === profile.id);
              return {
                ...structuredClone(profile),
                revision: old
                  ? old.revision +
                    (JSON.stringify(old.settings) !== JSON.stringify(profile.settings) ||
                    old.name !== profile.name
                      ? 1
                      : 0)
                  : 1,
              };
            });
            const chromium = {
              ...structuredClone(networkProfiles.activeProfile),
              id: "chromium-154",
              name: "Chromium 154",
              revision: 5,
              settings: {
                ...structuredClone(networkProfiles.activeProfile.settings),
                rootStore: "chrome-installed",
              },
            };
            const active = args.useProfile
              ? [networkProfiles.activeProfile, chromium, ...profiles].find(
                  (profile) => profile.id === args.useProfile,
                )
              : catalog.activeProfile;
            state.networkProfiles = {
              revision: catalog.revision + 1,
              customProfiles: profiles,
              activeProfile: structuredClone(active),
            };
            return structuredClone(state.networkProfiles);
          }
          case "save_product_state":
            state.savedProduct = structuredClone(args.productState);
            state.savedProduct.preferences.clientCertificatePolicy = structuredClone(
              state.certificatePolicy ?? { default: "noCertificate", rules: [] },
            );
            return structuredClone(state.savedProduct);
          case "pick_support_path":
            return state.supportPick ?? null;
          case "diagnostics_report":
            return {
              applicationVersion: "0.1.0",
              runtime: {
                operatingSystem: "Windows fixture",
                architecture: "x64",
                webviewVersion: "test",
              },
              events: [{ level: "warning", message: "safe fixture" }],
              privacyNotice: "Traffic bodies and credentials are omitted",
            };
          case "create_support_bundle":
            state.supportInput = structuredClone(args);
            return {
              destination: args.destination,
              bytes: 4096,
              includedRecentPaths: args.includeRecentPaths,
            };
          case "prepare_update_handoff":
            state.lifecycle = "stopped";
            return {
              lifecycle: "stopped",
              listener: null,
              summary: "Proxy stopped",
              hostRestorePending: false,
            };
          case "capture_status": {
            const status = structuredClone(state.capture ?? { state: "idle" });
            if (state.deferCaptureStatus) {
              state.captureStatusPending = true;
              await new Promise((resolve) => (state.releaseCaptureStatus = resolve));
              delete state.deferCaptureStatus;
            }
            if (state.captureStatusError) throw new Error("Fixture capture status failed");
            return status;
          }
          case "start_capture":
            state.lastCaptureRequest = structuredClone(args.request);
            state.capture = { state: "active", path: args.request.path, bytesWritten: 2048 };
            return structuredClone(state.capture);
          case "stop_capture":
            state.capture = { ...state.capture, state: "sealed" };
            return structuredClone(state.capture);
          case "pick_capture_path":
            state.lastCapturePick = structuredClone(args);
            return state.capturePick ?? null;
          case "import_capture":
            state.lastCaptureImport = structuredClone(args.request);
            if (state.captureInspectionError) throw new Error("Fixture inspection failed");
            return {
              records: 12,
              exchanges: 3,
              lossMarkers: 1,
              retainedBodyBytes: 2048,
              sealed: false,
              truncatedTail: true,
              validBytes: 4096,
            };
          case "inspect_trace_metadata":
            state.lastCaptureImport = structuredClone(args.request);
            if (state.captureInspectionError) throw new Error("Fixture inspection failed");
            return {
              path: /^[A-Za-z]:/.test(args.request.path)
                ? args.request.path
                : "C:/captures/" + args.request.path,
              format: args.request.path.endsWith(".saz")
                ? "saz"
                : args.request.path.endsWith(".har")
                  ? "har"
                  : "native",
              sessions: 3,
              bytes: 4096,
              context: { networkContext: "Original fixture network context" },
              sources: [],
              notes: ["Interrupted tail; valid prefix recovered"],
            };
          case "export_capture":
            if (state.captureExportError) throw new Error("Fixture source file could not be read");
            state.lastCaptureExport = structuredClone(args.request);
            return {
              destination: args.request.destination,
              records: 12,
              bytes: 4096,
              sourceSealed: false,
              sourceTruncatedTail: true,
              fidelity: "Valid prefix preserved",
            };
          case "composer_history":
            return state.composerHistory ?? [];
          case "execute_composer": {
            state.composerInput = structuredClone(args.request);
            if (state.deferComposer) {
              state.composerPending = true;
              await new Promise((resolve) => (state.finishComposer = resolve));
            } else await new Promise((resolve) => setTimeout(resolve, 80));
            if (state.composerError) throw new Error("Fixture send failed");
            state.composerResponseCount = (state.composerResponseCount ?? 0) + 1;
            const body = "<script>globalThis.unsafeComposer=true</script>",
              id = "composer-response-" + state.composerResponseCount;
            const response = detail(id);
            response.composerOrigin = state.composerResultSource ?? null;
            response.requests[0].target = args.request.url;
            response.responses[0].status = 201;
            response.responses[0].headers = [
              { name: "X-Duplicate", value: "first", sensitive: false, binary: false },
              { name: "X-Duplicate", value: "second", sensitive: false, binary: false },
            ];
            state.composerBytes =
              state.nextComposerBytes ?? Array.from(new TextEncoder().encode(body));
            Object.assign(response.storedBodies[0], {
              observedBytes: state.composerBytes.length,
              retainedBytes: state.composerBytes.length,
              mediaType: state.nextComposerType ?? "text/html",
            });
            state.composerResponseDetail = response;
            return {
              detail: structuredClone(response),
              id: 1,
              status: 201,
              headers: response.responses[0].headers,
              body,
              bodyIsHex: false,
              truncated: false,
              attribution: "composer",
              source: state.composerResultSource ?? null,
            };
          }
          case "script_help":
            return scriptHelp;
          case "script_declarations":
            return scriptDeclarations;
          case "script_runtime_status":
            return (state.activeScripts ?? []).map((item) => ({
              scriptId: item.manifest.id,
              revision: item.manifest.revision,
              available: !state.scriptRuntimeError,
              error: state.scriptRuntimeError ?? null,
            }));
          case "retry_script": {
            if (args.expectedGeneration !== state.scriptGeneration)
              throw new Error("script workspace changed");
            if (state.retryScriptError) throw new Error("Fixture retry failed");
            state.scriptRuntimeError = null;
            return {
              generation: state.scriptGeneration,
              active: state.activeScripts ?? [],
              saved: state.savedScript ? [state.savedScript] : [],
              built: state.scriptBuilds ?? [],
              paused: state.pausedScripts ?? [],
              runtimes: (state.activeScripts ?? []).map((item) => ({
                scriptId: item.manifest.id,
                revision: item.manifest.revision,
                available: true,
                error: null,
              })),
              candidateCount: 0,
              historyCount: 0,
            };
          }
          case "script_status":
            return {
              generation: state.scriptGeneration ?? 0,
              active: state.activeScripts ?? [],
              saved: state.savedScripts ?? (state.savedScript ? [state.savedScript] : []),
              built: state.scriptBuilds ?? [],
              paused: state.pausedScripts ?? [],
              runtimes: (state.activeScripts ?? []).map((item) => ({
                scriptId: item.manifest.id,
                revision: item.manifest.revision,
                available: !state.scriptRuntimeError,
                error: state.scriptRuntimeError ?? null,
              })),
              candidateCount: 0,
              historyCount: 0,
            };
          case "save_script": {
            if (args.expectedGeneration !== (state.scriptGeneration ?? 0))
              throw new Error("script workspace changed");
            if (state.deferScriptSave) {
              state.scriptSavePending = true;
              await new Promise((resolve) => (state.finishScriptSave = resolve));
            }
            state.savedScript = structuredClone(args.draft);
            state.scriptGeneration = (state.scriptGeneration ?? 0) + 1;
            return {
              generation: state.scriptGeneration,
              active: state.activeScripts ?? [],
              saved: [state.savedScript],
              built: state.scriptBuilds ?? [],
              paused: state.pausedScripts ?? [],
              runtimes: (state.activeScripts ?? []).map((item) => ({
                scriptId: item.manifest.id,
                revision: item.manifest.revision,
                available: !state.scriptRuntimeError,
                error: state.scriptRuntimeError ?? null,
              })),
              candidateCount: 0,
              historyCount: 0,
            };
          }
          case "validate_script": {
            if (state.changeScriptBeforeValidation) {
              state.changeScriptBeforeValidation = false;
              state.savedScript = {
                ...structuredClone(args.draft),
                source: args.draft.source + "\n// saved concurrently in another window",
              };
              state.scriptGeneration = (state.scriptGeneration ?? 0) + 1;
            }
            if (
              args.expectedGeneration !== undefined &&
              args.expectedGeneration !== (state.scriptGeneration ?? 0)
            )
              throw new Error("script workspace changed");
            state.scriptCandidate = structuredClone(args.draft);
            state.scriptGeneration = (state.scriptGeneration ?? 0) + 1;
            state.scriptCandidateGeneration = state.scriptGeneration;
            state.scriptCandidate.capabilities.writeHeaders = [
              ...new Set(state.scriptCandidate.capabilities.writeHeaders),
            ].sort();
            if (state.deferScriptValidation) {
              state.scriptValidationPending = true;
              await new Promise((resolve) => (state.finishScriptValidation = resolve));
            }
            if (state.scriptValidationError)
              throw new Error("Fixture TypeScript validation failed");
            const { source, language, retainedSource, javascriptHistory, ...manifest } =
              state.scriptCandidate;
            const javascript =
              language === "javascript" ? source : await window.__compileFixtureScript(source);
            state.scriptBuilds = [
              {
                manifest: { ...manifest, apiRevision: 2, sourceHash: "0".repeat(64) },
                source,
                language,
                retainedSource,
                javascriptHistory,
                javascript,
                sourceMap: [],
                javascriptHash: "0".repeat(64),
                sourceMapHash: "0".repeat(64),
                compiler: "fixture",
                declarationsHash: "0".repeat(64),
              },
            ];
            return {
              candidateId: "fixture-script",
              scriptId: args.draft.id,
              revision: args.draft.revision,
              sourceHash: "0".repeat(64),
            };
          }
          case "test_script": {
            state.scriptTestInput = structuredClone(args.invocation);
            if (state.deferScriptTest) {
              state.scriptTestPending = true;
              await new Promise((resolve) => (state.finishScriptTest = resolve));
            }
            return {
              decision: ["onCompleted", "onFailed"].includes(args.invocation.context.handler)
                ? { action: "continue" }
                : {
                    action: "edit",
                    edits: {
                      headers: [
                        {
                          operation: "set",
                          name: "User-Agent",
                          value: "Fixture/1.0",
                        },
                      ],
                    },
                  },
            };
          }
          case "activate_script": {
            const draft = structuredClone(state.scriptCandidate);
            if (state.deferScriptActivation) {
              state.scriptActivationPending = true;
              await new Promise((resolve) => (state.finishScriptActivation = resolve));
            }
            if (state.scriptActivationError) throw new Error("Fixture sandbox startup failed");
            if (state.scriptCandidateGeneration !== (state.scriptGeneration ?? 0))
              throw new Error("script workspace changed");
            if (
              state.activeScripts?.some(
                (item) => item.manifest.id === draft.id && item.manifest.revision >= draft.revision,
              )
            )
              throw new Error("script revision must increase before activation");
            state.activeScripts = structuredClone(state.scriptBuilds);
            state.pausedScripts = [];
            state.savedScript = draft;
            delete state.savedScripts;
            state.scriptGeneration = (state.scriptGeneration ?? 0) + 1;
            return {
              generation: state.scriptGeneration,
              active: state.activeScripts,
              saved: [draft],
              built: state.scriptBuilds ?? [],
              paused: state.pausedScripts ?? [],
              runtimes: (state.activeScripts ?? []).map((item) => ({
                scriptId: item.manifest.id,
                revision: item.manifest.revision,
                available: !state.scriptRuntimeError,
                error: state.scriptRuntimeError ?? null,
              })),
              candidateCount: 0,
              historyCount: 0,
            };
          }
          case "resume_script": {
            if (args.expectedGeneration !== state.scriptGeneration)
              throw new Error("script workspace changed");
            state.activeScripts = structuredClone(state.pausedScripts);
            state.pausedScripts = [];
            state.scriptGeneration++;
            return {
              generation: state.scriptGeneration,
              active: state.activeScripts,
              saved: state.savedScript ? [state.savedScript] : [],
              built: state.scriptBuilds ?? [],
              paused: [],
              runtimes: (state.activeScripts ?? []).map((item) => ({
                scriptId: item.manifest.id,
                revision: item.manifest.revision,
                available: !state.scriptRuntimeError,
                error: state.scriptRuntimeError ?? null,
              })),
              candidateCount: 0,
              historyCount: 0,
            };
          }
          case "disable_script":
            state.pausedScripts = structuredClone(state.activeScripts);
            state.activeScripts = [];
            state.scriptGeneration = (state.scriptGeneration ?? 0) + 1;
            return {
              generation: state.scriptGeneration,
              active: [],
              saved: state.savedScript ? [state.savedScript] : [],
              built: state.scriptBuilds ?? [],
              paused: state.pausedScripts ?? [],
              runtimes: (state.activeScripts ?? []).map((item) => ({
                scriptId: item.manifest.id,
                revision: item.manifest.revision,
                available: !state.scriptRuntimeError,
                error: state.scriptRuntimeError ?? null,
              })),
              candidateCount: 0,
              historyCount: 0,
            };
          case "breakpoint_status":
            return { enabled: true, paused: structuredClone(state.paused) };
          case "decide_breakpoint":
            state.paused = state.paused.filter(
              (item) => item.decisionId !== args.decision.decisionId,
            );
            return { enabled: true, paused: structuredClone(state.paused) };
          default:
            throw new Error("Unexpected fixture command: " + command);
        }
      },
    };
    globalThis.__cspViolations = [];
    addEventListener("securitypolicyviolation", (event) =>
      globalThis.__cspViolations.push(event.effectiveDirective),
    );
  },
  {
    workspace: initial.workspace,
    authConversions,
    networkProfiles: initial.networkProfiles,
    scriptDeclarations,
    scriptHelp,
    filterCatalog,
  },
);

const view = async (name) => {
  await page.locator('app-shell a[data-view="' + name + '"]').click();
  await page.locator("#" + name).waitFor({ state: "visible" });
};
const discardIfAsked = async () => {
  const dialog = page.locator("automation-workspace .unsaved-rule-dialog");
  if (await dialog.isVisible())
    await dialog.getByRole("button", { name: "Discard changes", exact: true }).click();
};
const newScratch = async () => {
  await page.locator(".workspace-tabs").getByRole("button", { name: "Rules", exact: true }).click();
  await newTrafficRule(page, "Create saved response from scratch…");
  await discardIfAsked();
  await page.locator(".auto-response-editor").waitFor({ state: "visible" });
};
const savedProperties = async () => {
  await page.waitForFunction(() => {
    const workspace = document
      .querySelector("app-shell")
      .shadowRoot.querySelector("automation-workspace");
    return !workspace.savingAutoResponse && workspace.existingResponse && !workspace.draftDirty;
  });
};
const cancelRule = async () => {
  await page
    .locator(".auto-response-editor")
    .getByRole("button", { name: "Cancel", exact: true })
    .first()
    .click();
  await discardIfAsked();
};
try {
  if (process.argv.includes("--mcp-only")) {
    await checkMcp(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write("MCP permission, key lifecycle, failure and layout checks passed.\n");
    process.exit(0);
  }
  if (process.argv.includes("--body-source-only")) {
    await checkBodySource(page, root);
    await checkJsonFields(page);
    await checkBodySourceBounds(page);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write("Body source navigation, copying, lifecycle and layouts verified.\n");
    process.exit(0);
  }
  if (process.argv.includes("--capture-tools-only")) {
    await page.goto("https://workspace.test/");
    await page.waitForFunction(() => document.querySelector("app-shell").traffic.queryLoaded);
    await page
      .locator("#traffic")
      .getByLabel("Filter or search traffic", { exact: true })
      .fill("example.test");
    await page.waitForFunction(() => {
      const traffic = document.querySelector("app-shell").traffic;
      return (
        traffic.liveFilter === "example.test" &&
        !traffic.queryPending &&
        traffic.sessions.length >= 2
      );
    });
    const rows = page.locator("#traffic tr[data-session-id]");
    await rows.nth(0).click();
    await rows.nth(1).click({ modifiers: ["Control"] });
    await page.waitForFunction(
      () => document.querySelector("app-shell").traffic.selectedTrafficCount === 2,
    );
    await checkCaptureTools(page, root, view);
    assert.deepEqual(errors, []);
    await browser.close();
    process.exit(0);
  }
  if (process.argv.includes("--traffic-perf-only")) {
    const index = process.argv.indexOf("--perf-output");
    await checkTrafficPerformance(page, index >= 0 ? process.argv[index + 1] : undefined);
    assert.deepEqual(errors, []);
    await browser.close();
    process.exit(0);
  }
  if (process.argv.includes("--streams-only")) {
    await page.goto("https://workspace.test/");
    await checkStreams(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write("Stream inspection and context copying verified.\n");
    process.exit(0);
  }
  if (process.argv.includes("--copy-only")) {
    await checkCopy(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write("Inspection copy menus verified.\n");
    process.exit(0);
  }
  if (process.argv.includes("--visualizers-only")) {
    await page.goto("https://workspace.test/");
    await checkVisualizers(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Host visualizers, bounded structures, exact source spans and inactive cancellation verified.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--extensions-only")) {
    await page.goto("https://workspace.test/");
    await checkExtensions(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Extension management, declared inputs, inert metadata and async lifetimes verified.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--converter-only")) {
    await checkFormatConverter(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Converter flyout, edits, races, context actions, handoff and layouts verified.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--incidents-only")) {
    await checkIncidents(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Incident review, export dialog lifecycle, stale reviews and capture markers verified.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--traffic-presentation-only")) {
    await page.goto("https://workspace.test/");
    await checkTrafficPresentation(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write("Traffic presentation and one-click inspector switching verified.\n");
    process.exit(0);
  }
  if (process.argv.includes("--classifications-only")) {
    await page.goto("https://workspace.test/");
    await checkTrafficClassifications(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Static header classifications, bounded windows, stale replies and inactive cleanup verified.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--request-controls-only")) {
    await checkRequestControls(page);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Request cancellation, uncached selections and delayed removal focus verified.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--traffic-shortcuts-only")) {
    await checkTrafficShortcuts(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write("Traffic clear, Undo/Redo and Follow newest shortcuts verified.\n");
    process.exit(0);
  }
  if (process.argv.includes("--response-followups-only")) {
    await checkResponseSetFollowups(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Response collection, active state, cancellation and Clear regressions verified.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--saved-demos-only")) {
    await checkSavedDemos(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Durable demo save/review/playback/replace/delete and layouts verified.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--lifecycle-only")) {
    await checkResponseSetLifecycle(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write("Response set lifecycle verified.\n");
    process.exit(0);
  }
  if (process.argv.includes("--set-intake-move-only")) {
    await checkResponseSetIntakeMove(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Response set Move source drafts, native checkbox Space, failures and newer edits verified.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--set-intake-refresh-only")) {
    await checkResponseSetIntakeRefresh(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Response set intake current baseline refresh verified for clean, Save, Discard and newer drafts.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--set-intake-only")) {
    await checkResponseSetIntake(page, root);
    await checkResponseSetIntakeRefresh(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Response set bulk intake, Copy/Move, drafts, failures and layouts verified.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--set-collection-adds-only")) {
    await checkResponseSetCollectionAdds(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Scratch/file additions retain dirty collection ownership, snapshots and saved bodies.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--sets-only")) {
    await checkResponseSetWorkspaces(page, root);
    await checkResponseSetDraftRegressions(page);
    await checkResponseSetPendingDialogs(page, root);
    await checkResponseSetCollectionAdds(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write("Response set collection workspaces verified.\n");
    process.exit(0);
  }
  if (
    process.argv.includes("--body-refresh-only") ||
    process.argv.includes("--body-refresh-reproduce")
  ) {
    await checkBodyRefresh(page, root, {
      reproduce: process.argv.includes("--body-refresh-reproduce"),
    });
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write("Body refresh lifecycle verified.\n");
    process.exit(0);
  }
  if (
    process.argv.includes("--preview-prerequisites-only") ||
    process.argv.includes("--preview-prerequisites-reproduce")
  ) {
    await checkPreviewPrerequisites(page, root, {
      reproduce: process.argv.includes("--preview-prerequisites-reproduce"),
    });
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write("Preview prerequisites verified.\n");
    process.exit(0);
  }
  if (process.argv.includes("--auth-only")) {
    await checkAuth(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Auth masking, reveal/copy, stale replies, coverage, keyboard and layouts verified.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--body-decoding-only")) {
    await checkBodyDecoding(page);
    await checkXssiJson(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write("Binary body Decode controls and XSSI JSON views verified.\n");
    process.exit(0);
  }
  if (process.argv.includes("--hex-only")) {
    await page.goto("https://workspace.test/");
    await page.locator('tr[data-session-id="first"]').click();
    const inspector = page.locator('traffic-workspace message-inspector[side="response"]');
    await inspector.getByLabel("Body viewer", { exact: true }).selectOption("bytes");
    await inspector.locator("hex-viewer").waitFor({ state: "visible" });
    await checkHexViewer(
      page,
      inspector,
      resolve(root, "../../../target/ui-check/hex-selection.png"),
    );
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Hex byte navigation, selection, paging and compact reachability verified.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--traffic-filters-only")) {
    await page.goto("https://workspace.test/");
    await checkTrafficFilters(page, root);
    await checkTrafficAdvanced(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write("Traffic filtering and stream lifetime checks passed.\n");
    process.exit(0);
  }
  if (process.argv.includes("--scripts-only")) {
    await page.goto("https://workspace.test/");
    await view("automation");
    await page
      .locator(".workspace-tabs")
      .getByRole("button", { name: "Scripts", exact: true })
      .click();
    await checkScripts(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Script draft, activation, discard, async state and responsive layouts verified.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--automation-ux-only")) {
    await checkAutomationContracts(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Automation draft, activation, failure feedback and responsive contracts verified.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--shortcuts-only")) {
    await checkWindowShortcuts(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Window and Traffic shortcuts, scopes, dialogs, import and responsive menus verified.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--browser-headers-only")) {
    await page.goto("https://workspace.test/");
    await view("automation");
    await checkUserAgentOverrides(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Browser header authoring, presets, ordering, drafts and responsive layouts verified.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--rules-only")) {
    await checkUnifiedRules(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Unified rule creation, direction/search filters, pause scope, drafts and responsive layouts verified.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--actions-only")) {
    await checkActionMenus(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Actions submenus, keyboard focus, copying and responsive layouts verified.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--navigation-only")) {
    await page.goto("https://workspace.test/");
    await page.waitForFunction(() => document.querySelector("app-shell")?.traffic?.queryLoaded);
    await checkTrafficNavigation(page);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write("Traffic navigation verified.\n");
    process.exit(0);
  }
  if (process.argv.includes("--timeline-only")) {
    await page.goto("https://workspace.test/");
    await checkTrafficTimeline(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Timeline scope, dragging, keyboard, live updates, races, failures and virtualization verified.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--timing-labels-only")) {
    await page.goto("https://workspace.test/");
    await checkTimingLabels(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write("Timing names, values and reports verified.\n");
    process.exit(0);
  }
  if (process.argv.includes("--selection-only")) {
    await page.goto("https://workspace.test/");
    await checkAppSelection(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write("App selection and selectable evidence verified.\n");
    process.exit(0);
  }
  if (process.argv.includes("--text-edits-only")) {
    await checkTextEdits(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Literal response edit flows, async drafts, comparison evidence and responsive layouts verified.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--header-rules-only")) {
    await checkHeaderRules(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Header rule authoring, raw bytes, ordered tests, activation, stale updates, Undo, evidence and responsive layouts verified.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--set-preview-only")) {
    await checkResponseSetPreview(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Current-set preview origin, frozen context and historical report/reveal guards verified.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--guide-only")) {
    await checkResponseSetGuide(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Response set guide lifecycle, native preference contract, focus and compact layouts verified.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--file-snapshots-only")) {
    await checkResponseSnapshots(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Response snapshot identities, changed output, incomplete retention and responsive evidence verified.\n",
    );
    process.exit(0);
  }
  await checkResponseSnapshots(page, root);
  if (process.argv.includes("--file-modes-only")) {
    await checkResponseFiles(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Explicit file copy/link, repair, drafts and size-independent modes verified.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--portable-cases-only")) {
    await checkPortableCases(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Portable case review, password retry, inert inspection, disabled import, cancellation and responsive layouts verified.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--incidents-only")) {
    await checkIncidents(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Incident intake, markers, reviewed export and responsive layouts verified.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--comparison-only")) {
    await checkComparison(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write(
      "Comparison interactions, races, pairing and responsive layouts verified.\n",
    );
    process.exit(0);
  }
  if (process.argv.includes("--network-profiles")) {
    await page.goto("https://workspace.test/");
    await view("settings");
    await checkNetworkProfiles(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write("Network profile interactions and responsive layouts verified.\n");
    process.exit(0);
  }
  const startupNavigationRequests = requests.length;
  await page.goto("https://workspace.test/");
  await page.waitForFunction(() =>
    document
      .querySelector("app-shell")
      .shadowRoot.querySelector(".session-status")
      .textContent.includes("4 traffic entries."),
  );
  await page.getByRole("button", { name: "Stable update available", exact: true }).waitFor();
  assert.equal(
    await page.locator("#update-panel").isVisible(),
    false,
    "Startup opened About without user action",
  );
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.calls.check_updates),
    1,
    "Startup did not check exactly once",
  );
  assert.equal(
    await page.evaluate(() => document.activeElement === document.body),
    true,
    "The startup update prompt stole focus",
  );
  await page.evaluate(() => sessionStorage.setItem("startup-update-tested", "true"));
  assert.match(
    await page.locator(".session-status").textContent(),
    /Proxy stopped/,
    "Traffic banner claims live capture while the proxy is stopped",
  );
  assert.match(await page.locator(".list-footer").textContent(), /Showing captured traffic/);
  assert.doesNotMatch(
    await page.locator(".list-footer").textContent(),
    /Following live|capture continues/,
  );
  assert.equal(await page.locator("#traffic").isVisible(), true);
  assert.equal(await page.locator('a[data-view="traffic"]').getAttribute("aria-current"), "page");
  const editorOutputs = Object.entries(metadata.outputs)
    .filter(([, output]) =>
      Object.keys(output.inputs).some((input) => input.includes("node_modules/monaco-editor")),
    )
    .map(([file]) => "/" + file.replace(/^dist\//, ""));
  assert.equal(
    requests
      .slice(startupNavigationRequests)
      .some(
        (path) => path === "/monaco.js" || path === "/monaco.css" || editorOutputs.includes(path),
      ),
    false,
    "Monaco loaded at startup",
  );
  assert.equal(
    await page.evaluate(() => customElements.get("automation-workspace") !== undefined),
    false,
    "Automation hydrated at startup",
  );
  if (process.env.TRANSMOG_STREAM_ONLY) {
    await checkStreams(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write("Stream UI checks passed.\n");
    process.exit(0);
  }
  const startupRequests = requests.length;
  if (process.env.TRANSMOG_LOOPBACK_ONLY) {
    await checkLoopback(page, root);
    assert.deepEqual(errors, []);
    await browser.close();
    process.stdout.write("Loopback modal checks passed.\n");
    process.exit(0);
  }
  assert.equal(
    await page.locator(".traffic-table tr[data-session-id][draggable]").count(),
    0,
    "Traffic rows still advertise cross-tab dragging",
  );
  await view("automation");
  await checkUserAgentOverrides(page, root);
  assert.equal(
    requests.slice(startupRequests).some((path) => path === "/monaco.js"),
    false,
    "Response management eagerly loaded the script editor",
  );
  await page.getByRole("button", { name: "New rule…", exact: true }).waitFor({ state: "visible" });
  assert.equal(
    await page
      .getByRole("button", { name: "Create saved response from scratch…", exact: true })
      .isVisible(),
    false,
    "Scratch authoring is prominent",
  );
  assert.match(
    await page.locator(".automation-activity").textContent(),
    /Enabled for new traffic|No active automation/,
  );
  await newTrafficRule(page, "Serve a captured response…");
  await page.locator("#traffic").waitFor({ state: "visible" });
  // Populated lists must leave bulk actions reachable by an actual mouse click.
  await page.evaluate(async () => {
    const state = globalThis.__workspaceFixture;
    state.reviewOriginalRows = state.sessions;
    state.sessions = Array.from({ length: 60 }, (_, index) => ({
      ...state.sessions[1],
      id: "layout-" + index,
      path: "/layout/" + index,
      url: "http://example.test/layout/" + index,
      startedAt: 2000 + index,
    }));
    await document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace")
      .refreshSessions(undefined, true);
  });
  await page.evaluate(
    async () => await document.querySelector("app-shell").traffic.revealCurrent("layout-59"),
  );
  await page.locator('tr[data-session-id="layout-59"]').click();
  await page.keyboard.press("Control+A");
  const bulkAction = page
    .locator(".traffic-selection-bar")
    .getByRole("button", { name: /^Create \d+ saved responses?…$/ });
  const selectionBounds = await page.locator(".traffic-selection-bar").boundingBox();
  const tableBounds = await page.locator(".table-wrap").boundingBox();
  assert.ok(
    selectionBounds.y + selectionBounds.height <= tableBounds.y + 1,
    "Traffic table overlaps bulk actions",
  );
  await bulkAction.click();
  await page.locator(".batch-review").waitFor({ state: "visible" });
  await page.getByRole("button", { name: "Cancel batch", exact: true }).click();
  await view("traffic");
  await page.evaluate(async () => {
    const state = globalThis.__workspaceFixture;
    state.sessions = state.reviewOriginalRows;
    delete state.reviewOriginalRows;
    const traffic = document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace");
    traffic.clearTrafficSelection();
    await traffic.refreshSessions(undefined, true);
  });
  assert.equal(
    await page.evaluate(() => CSS.supports("width", "attr(data-width type(<length>))")),
    true,
    "Typed CSS attributes required by resizable panels are unavailable",
  );
  assert.deepEqual(
    await page
      .locator(".traffic-table th")
      .evaluateAll((headers) => headers.map((header) => header.dataset.columnId)),
    [
      "request-number",
      "method",
      "status",
      "process",
      "host",
      "path",
      "duration",
      "response-bytes",
      "protocol",
    ],
  );
  assert.match(
    await page.locator('tr[data-session-id="first"] td[data-column-id="process"]').textContent(),
    /Fixture \(42\)/,
  );
  assert.equal(
    await page.locator('tr[data-session-id="cached"]').getAttribute("data-tone"),
    "not-modified",
  );
  assert.equal(
    await page.locator('tr[data-session-id="first"] .navigation-badge').textContent(),
    "Page",
  );
  assert.match(
    await page.locator('tr[data-session-id="first"]').getAttribute("aria-label"),
    /^Top-level navigation:/,
  );
  assert.equal(await page.locator('tr[data-session-id="second"] .navigation-badge').count(), 0);
  assert.equal(
    await page.evaluate(async () => {
      const workspace = document
        .querySelector("app-shell")
        .shadowRoot.querySelector("traffic-workspace");
      workspace.timingDialog.showModal();
      workspace.closeTimings();
      workspace.timingDialog.showModal();
      const generation = workspace.timingGeneration;
      await new Promise((resolve) => setTimeout(resolve, 20));
      const stable = workspace.timingGeneration === generation;
      workspace.closeTimings();
      return stable;
    }),
    true,
    "A stale close event invalidated reopened timings",
  );
  await page.getByRole("button", { name: "Filters", exact: true }).click();
  const destinations = page.locator("#traffic-filter-picker:popover-open");
  await destinations.getByLabel("Filter field", { exact: true }).selectOption("fetch-destination");
  await page.waitForFunction(() => !document.querySelector("app-shell").traffic.pickerLoading);
  await destinations.getByRole("checkbox", { name: "Document", exact: true }).check();
  const imageDestination = destinations.getByRole("checkbox", { name: "Image", exact: true });
  await imageDestination.focus();
  await page.keyboard.press("Space");
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/fetch-destinations-wide.png"),
  });
  await page.setViewportSize({ width: 760, height: 520 });
  const destinationBounds = await destinations.boundingBox();
  assert.ok(
    destinationBounds.x >= 0 &&
      destinationBounds.y >= 0 &&
      destinationBounds.x + destinationBounds.width <= 760 &&
      destinationBounds.y + destinationBounds.height <= 520,
    "Destination dropdown escaped the viewport",
  );
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/fetch-destinations-compact.png"),
  });
  await destinations.getByRole("button", { name: "Apply filter", exact: true }).click();
  await page.waitForFunction(() => document.querySelector("app-shell").traffic.totalMatched === 2);
  assert.deepEqual(
    await page
      .locator("tr[data-session-id]")
      .evaluateAll((rows) => rows.map((row) => row.dataset.sessionId).sort()),
    ["first", "image"],
  );
  assert.equal(await page.locator(".filter-chip").count(), 1);
  await page.locator(".filter-chip-remove").click();
  await page.waitForFunction(() => document.querySelector("app-shell").traffic.totalMatched === 4);
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.evaluate(() => {
    const workspace = document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace");
    globalThis.__workspaceFixture.deferResultQuery = true;
    workspace.searchText = "/";
    workspace.searchInput.value = "/";
    globalThis.__destinationSearch =
      ((workspace.trafficSearchKind = "contents"), workspace.runContentSearch());
  });
  await page.waitForFunction(() => globalThis.__workspaceFixture.resultQueryPending);
  await page.evaluate(async () => {
    await document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace")
      .setFetchDestination("script", { currentTarget: { checked: true } });
    globalThis.__workspaceFixture.releaseResultQuery();
    await globalThis.__destinationSearch;
  });
  assert.deepEqual(
    await page.evaluate(
      () =>
        document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
          .searchMatchIds,
    ),
    ["second"],
    "A late search ignored the updated destination filter",
  );
  assert.deepEqual(
    await page
      .locator("tr[data-session-id]")
      .evaluateAll((rows) => rows.map((row) => row.dataset.sessionId)),
    ["second"],
  );
  await page.evaluate(async () => {
    const workspace = document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace");
    await workspace.selectAllSearchMatches();
  });
  assert.equal(await page.locator('tr[data-session-id][aria-selected="true"]').count(), 1);
  await page.evaluate(async () => {
    const workspace = document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace");
    await workspace.clearContentSearch(false);
    await workspace.clearFilters();
    workspace.selectSearchMatches = false;
  });
  const toggle = page.getByRole("button", { name: "Toggle navigation labels" });
  await toggle.click();
  assert.equal(await toggle.getAttribute("aria-expanded"), "false");
  const methodEdge = page.getByRole("separator", { name: "Resize Method column", exact: true });
  const originalWidth = await page
    .locator("#header-method")
    .evaluate((header) => header.getBoundingClientRect().width);
  await methodEdge.focus();
  await page.keyboard.press("ArrowRight");
  const resizedWidth = await page
    .locator("#header-method")
    .evaluate((header) => header.getBoundingClientRect().width);
  assert.ok(
    Math.abs(resizedWidth - originalWidth - 10) < 1,
    "Keyboard resizing stretched the column beyond the requested width",
  );
  const edgeBounds = await methodEdge.boundingBox();
  await page.mouse.move(edgeBounds.x + 2, edgeBounds.y + 10);
  await page.mouse.down();
  await page.mouse.move(edgeBounds.x + 3, edgeBounds.y + 10);
  assert.ok(
    Math.abs(
      (await page
        .locator("#header-method")
        .evaluate((header) => header.getBoundingClientRect().width)) -
        resizedWidth -
        1,
    ) < 1,
    "Column snapped at the start of a resize",
  );
  await page.mouse.move(edgeBounds.x + 42, edgeBounds.y + 10, { steps: 5 });
  await page.mouse.up();
  assert.ok(
    Math.abs(
      (await page
        .locator("#header-method")
        .evaluate((header) => header.getBoundingClientRect().width)) -
        resizedWidth -
        40,
    ) < 1,
    "Pointer resizing did not track cursor movement",
  );
  const processHeader = await page.locator("#header-process .column-trigger").boundingBox();
  const pathHeader = await page.locator("#header-path .column-trigger").boundingBox();
  await page.mouse.move(processHeader.x + 15, processHeader.y + 12);
  await page.mouse.down();
  await page.mouse.move(processHeader.x + 25, processHeader.y + 12, { steps: 3 });
  assert.equal(
    await page.locator("#header-process").getAttribute("data-dragging"),
    "",
    "Header button did not enter drag mode",
  );
  await page.mouse.move(pathHeader.x + pathHeader.width / 2, pathHeader.y + 12, { steps: 8 });
  assert.equal(
    await page.locator("#header-path").getAttribute("data-drop-target"),
    "",
    "Column drop target is not visible",
  );
  await page.mouse.up();
  assert.deepEqual(
    await page
      .locator(".traffic-table th")
      .evaluateAll((headers) => headers.map((header) => header.dataset.columnId)),
    [
      "request-number",
      "method",
      "status",
      "host",
      "path",
      "process",
      "duration",
      "response-bytes",
      "protocol",
    ],
  );
  await page.waitForTimeout(300); // Let the accidental post-drag click guard expire.
  const dragCancelBounds = await page.locator("#header-process .column-trigger").boundingBox();
  await page.mouse.move(dragCancelBounds.x + 15, dragCancelBounds.y + 12);
  await page.mouse.down();
  await page.mouse.move(dragCancelBounds.x - 50, dragCancelBounds.y + 12, { steps: 5 });
  await page.keyboard.press("Escape");
  await page.mouse.up();
  assert.deepEqual(
    await page
      .locator(".traffic-table th")
      .evaluateAll((headers) => headers.map((header) => header.dataset.columnId)),
    [
      "request-number",
      "method",
      "status",
      "host",
      "path",
      "process",
      "duration",
      "response-bytes",
      "protocol",
    ],
    "Cancelling a column drag changed the layout",
  );
  await page.waitForTimeout(300);
  await page.evaluate(() =>
    document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace")
      .setColumnWidth("method", 32, true),
  );
  assert.ok(
    Math.abs(
      (await page
        .locator("#header-method")
        .evaluate((header) => header.getBoundingClientRect().width)) - 32,
    ) < 1,
    "Minimum column width is stretched by table or header content",
  );
  await methodEdge.press("Home");
  const openColumn = async (id) => {
    await page.locator("#header-" + id + " .column-trigger").click();
    await page.locator("#column-actions").waitFor({ state: "visible" });
  };
  await openColumn("host");
  await page
    .locator("#column-actions")
    .getByRole("button", { name: "Move left", exact: true })
    .click();
  assert.deepEqual(
    await page
      .locator(".traffic-table th")
      .evaluateAll((headers) => headers.slice(0, 4).map((header) => header.dataset.columnId)),
    ["request-number", "method", "host", "status"],
    "Moving across pinned columns did not change visible order",
  );
  await openColumn("host");
  await page
    .locator("#column-actions")
    .getByRole("button", { name: "Move right", exact: true })
    .click();
  await openColumn("host");
  await page
    .locator("#column-actions")
    .getByRole("button", { name: "Unpin column", exact: true })
    .click();
  await openColumn("status");
  await page
    .locator("#column-actions")
    .getByRole("button", { name: "Highest first", exact: false })
    .click();
  await page.waitForFunction(
    () =>
      document
        .querySelector("app-shell")
        .shadowRoot.querySelector(".traffic-table tbody tr[data-session-id]").dataset.sessionId ===
      "cached",
  );
  assert.equal(await page.locator("#header-status").getAttribute("aria-sort"), "descending");
  await openColumn("host");
  await page
    .locator("#column-actions")
    .getByRole("button", { name: "Pin column", exact: true })
    .click();
  assert.equal(await page.locator("#header-host").getAttribute("data-pinned"), "");
  await openColumn("response-bytes");
  await page
    .locator("#column-actions")
    .getByRole("button", { name: "Hide column", exact: true })
    .click();
  assert.equal(await page.locator("#header-response-bytes").count(), 0);
  await page.getByRole("button", { name: "View", exact: true }).click();
  await page.locator("#table-settings").getByText("List columns", { exact: true }).click();
  await page.locator("#table-settings").getByLabel("Response size", { exact: true }).check();
  await page.keyboard.press("Escape");
  assert.equal(await page.locator("#header-response-bytes").count(), 1);
  await openColumn("status");
  await page
    .locator("#column-actions")
    .getByRole("button", { name: "Filter Status…", exact: true })
    .click();
  await page.locator("#traffic-filter-picker .filter-custom summary").click();
  await page.getByRole("textbox", { name: "Column filter value" }).fill("304");
  await page
    .locator("#traffic-filter-picker")
    .getByRole("button", { name: "Apply filter" })
    .click();
  await page.waitForFunction(
    () =>
      document
        .querySelector("app-shell")
        .shadowRoot.querySelectorAll(".traffic-table tbody tr[data-session-id]").length === 1,
  );
  assert.equal(
    await page.locator(".traffic-table tbody tr[data-session-id]").getAttribute("data-session-id"),
    "cached",
  );
  await page.getByRole("button", { name: "Clear filters", exact: true }).click();
  await page.getByRole("button", { name: "Follow newest", exact: true }).click();
  const listDivider = page.getByRole("separator", {
    name: "Resize traffic list and inspector",
    exact: true,
  });
  const listBefore = await page
    .locator(".session-list-pane")
    .evaluate((pane) => pane.getBoundingClientRect().height);
  await listDivider.focus();
  await page.keyboard.press("ArrowDown");
  assert.ok(
    (await page
      .locator(".session-list-pane")
      .evaluate((pane) => pane.getBoundingClientRect().height)) >
      listBefore + 5,
    "Panel divider did not resize list",
  );
  await page.getByRole("button", { name: "View", exact: true }).click();
  await page.getByLabel("Traffic layout", { exact: true }).selectOption("side-by-side");
  await page.keyboard.press("Escape");
  const panes = await page.locator(".traffic-workspace").evaluate((grid) => ({
    list: grid.querySelector(".session-list-pane").getBoundingClientRect().toJSON(),
    details: grid.querySelector(".details-pane").getBoundingClientRect().toJSON(),
  }));
  assert.ok(
    panes.details.x > panes.list.x + panes.list.width,
    "Side-by-side layout did not arrange panes horizontally",
  );
  await page.getByRole("button", { name: "View", exact: true }).click();
  await page.getByLabel("Traffic layout", { exact: true }).selectOption("stacked");
  await page.keyboard.press("Escape");
  const proxy = page.locator(".top-actions proxy-toggle button");
  const proxyBackground = async (lifecycle) => {
    await page.waitForFunction(
      (lifecycle) =>
        document
          .querySelector("app-shell")
          .shadowRoot.querySelector(".top-actions proxy-toggle button").dataset.lifecycle ===
        lifecycle,
      lifecycle,
    );
    await proxy.evaluate(async (button) => {
      // Reading the style starts any pending CSS transition; labels can update
      // before the 100ms background transition has reached its final color.
      getComputedStyle(button).backgroundColor;
      await Promise.all(
        button.getAnimations().map((animation) => animation.finished.catch(() => {})),
      );
    });
    return proxy.evaluate((button) => getComputedStyle(button).backgroundColor);
  };
  await page.evaluate(() => (globalThis.__workspaceFixture.holdProxyStart = true));
  await proxy.click();
  await page
    .getByRole("button", { name: "Starting…", exact: true })
    .first()
    .waitFor({ state: "visible" });
  assert.match(await page.locator(".session-status").textContent(), /^Starting proxy\./);
  assert.equal(await proxy.isEnabled(), false);
  await page.evaluate(() => {
    globalThis.__workspaceFixture.holdProxyStart = false;
    globalThis.__workspaceFixture.releaseProxyStart();
  });
  await page
    .getByRole("button", { name: "Stop proxy", exact: true })
    .first()
    .waitFor({ state: "visible" });
  assert.match(await page.locator(".session-status").textContent(), /^Proxy running\./);
  const runningColor = await proxyBackground("running");
  await page.evaluate(() => (globalThis.__workspaceFixture.holdProxyStop = true));
  await proxy.click();
  await page
    .getByRole("button", { name: "Stopping…", exact: true })
    .first()
    .waitFor({ state: "visible" });
  assert.match(await page.locator(".session-status").textContent(), /^Stopping proxy\./);
  await page.evaluate(() => {
    globalThis.__workspaceFixture.holdProxyStop = false;
    globalThis.__workspaceFixture.releaseProxyStop();
  });
  await page
    .getByRole("button", { name: "Start proxy", exact: true })
    .first()
    .waitFor({ state: "visible" });
  assert.notEqual(await proxyBackground("stopped"), runningColor);
  await page.evaluate(async () => {
    globalThis.__workspaceFixture.lifecycle = "running";
    await document.querySelector("app-shell").refreshStatus();
  });
  await page
    .getByRole("button", { name: "Stop proxy", exact: true })
    .first()
    .waitFor({ state: "visible" });
  await proxy.click();
  await page
    .getByRole("button", { name: "Start proxy", exact: true })
    .first()
    .waitFor({ state: "visible" });
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.calls.stop_application),
    2,
    "Proxy action did not track refreshed lifecycle state",
  );

  await page.evaluate(async () => {
    globalThis.__workspaceFixture.testDrain = true;
    globalThis.__workspaceFixture.lifecycle = "running";
    await document.querySelector("app-shell").refreshStatus();
  });
  await proxy.click();
  await page.waitForFunction(() => {
    const shell = document.querySelector("app-shell");
    return shell.lifecycleKind === "draining" && !shell.proxyPending;
  });
  assert.equal(
    await page.getByRole("button", { name: "Start proxy", exact: true }).first().isEnabled(),
    true,
    "An in-flight drain prevented resume",
  );
  await page.getByRole("button", { name: "Start proxy", exact: true }).first().click();
  await page.waitForFunction(() => {
    const shell = document.querySelector("app-shell");
    return shell.lifecycleKind === "running" && !shell.proxyPending;
  });
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.calls.resume_application),
    1,
  );
  await page.evaluate(() => (globalThis.__workspaceFixture.testDrain = false));
  await proxy.click();
  await page.getByRole("button", { name: "Start proxy", exact: true }).first().waitFor();
  await page.evaluate(async () => {
    globalThis.__workspaceFixture.statusError = true;
    await document.querySelector("app-shell").refreshStatus();
  });
  await page
    .locator(".global-diagnostics")
    .getByText("Status unavailable: Fixture status unavailable", { exact: true })
    .waitFor({ state: "visible" });
  await page.evaluate(async () => {
    globalThis.__workspaceFixture.statusError = false;
    await document.querySelector("app-shell").refreshStatus();
  });

  const retainedRows = await page.evaluate(async () => {
    const state = globalThis.__workspaceFixture;
    const rows = state.sessions;
    state.sessions = [];
    await document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace")
      .refreshSessions(undefined, true);
    return rows;
  });
  assert.equal(
    await page.locator(".session-status").textContent(),
    "Proxy stopped. Start the proxy to capture traffic.",
  );
  await page.evaluate(async () => {
    globalThis.__workspaceFixture.lifecycle = "running";
    await document.querySelector("app-shell").refreshStatus();
  });
  assert.equal(
    await page.locator(".session-status").textContent(),
    "Proxy running. Waiting for traffic. Make a request through the proxy.",
  );
  for (const lifecycle of ["stopping", "failed"]) {
    await page.evaluate(async (lifecycle) => {
      globalThis.__workspaceFixture.lifecycle = lifecycle;
      await document.querySelector("app-shell").refreshStatus();
    }, lifecycle);
    assert.doesNotMatch(
      await page.locator(".session-status").textContent(),
      /Waiting for proxied requests|Watching live/,
    );
    assert.doesNotMatch(
      await page.locator(".list-footer").textContent(),
      /Following live|capture continues/,
    );
  }
  await page.evaluate(async () => {
    const shell = document.querySelector("app-shell"),
      state = globalThis.__workspaceFixture;
    state.lifecycle = "running";
    await shell.refreshStatus();
    state.queryDelay = 80;
    const refresh = shell.shadowRoot.querySelector("traffic-workspace").refreshSessions();
    state.lifecycle = "stopped";
    await shell.refreshStatus();
    await refresh;
    state.queryDelay = 0;
  });
  assert.equal(
    await page.locator(".session-status").textContent(),
    "Proxy stopped. Start the proxy to capture traffic.",
    "A delayed query restored a stale running message",
  );
  await page.evaluate(async (rows) => {
    globalThis.__workspaceFixture.sessions = rows;
    await document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace")
      .refreshSessions(undefined, true);
  }, retainedRows);
  await page.evaluate(async () => {
    const state = globalThis.__workspaceFixture,
      shell = document.querySelector("app-shell");
    state.queryError = true;
    await shell.shadowRoot.querySelector("traffic-workspace").refreshSessions();
    state.lifecycle = "running";
    await shell.refreshStatus();
  });
  assert.match(
    await page.locator(".session-status").textContent(),
    /^Proxy running\. Traffic query failed: Fixture query failure/,
    "A lifecycle refresh hid a traffic query failure",
  );
  await page.evaluate(async () => {
    const state = globalThis.__workspaceFixture,
      shell = document.querySelector("app-shell");
    state.queryError = false;
    state.lifecycle = "stopped";
    await shell.refreshStatus();
    await shell.shadowRoot.querySelector("traffic-workspace").refreshSessions(undefined, true);
  });

  await page.evaluate(() => {
    globalThis.__row = document
      .querySelector("app-shell")
      .shadowRoot.querySelector('tr[data-session-id="first"]');
  });
  await page.locator("form.filters button[type=submit]").click();
  await page.waitForFunction(() =>
    document
      .querySelector("app-shell")
      .shadowRoot.querySelector(".session-status")
      .textContent.includes("traffic entries."),
  );
  assert.equal(
    await page.evaluate(
      () =>
        globalThis.__row ===
        document.querySelector("app-shell").shadowRoot.querySelector('tr[data-session-id="first"]'),
    ),
    true,
    "A refresh replaced a keyed row",
  );
  const coalesced = await page.evaluate(async () => {
    const traffic = document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace");
    const before = globalThis.__workspaceFixture.calls.query_traffic_view;
    globalThis.__workspaceFixture.queryDelay = 25;
    await Promise.all(Array.from({ length: 30 }, () => traffic.refreshSessions()));
    globalThis.__workspaceFixture.queryDelay = 0;
    return globalThis.__workspaceFixture.calls.query_traffic_view - before;
  });
  assert.ok(coalesced <= 2, "Burst refreshes were not coalesced");
  await page.evaluate(async () => {
    const traffic = document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace");
    globalThis.__workspaceFixture.slowDetail = true;
    await Promise.all([
      traffic.inspectSession(globalThis.__workspaceFixture.sessions[0]),
      traffic.inspectSession(globalThis.__workspaceFixture.sessions[1]),
    ]);
  });
  await page.waitForFunction(() =>
    document
      .querySelector("app-shell")
      .shadowRoot.querySelector("#inspector-heading")
      .textContent.endsWith("/second"),
  );
  assert.equal(
    await page.locator('tr[data-session-id="second"]').getAttribute("aria-selected"),
    "true",
  );
  assert.match(
    await page.locator(".list-footer").textContent(),
    /Auto-scroll paused · showing captured traffic/,
  );
  await page
    .locator('traffic-workspace message-inspector[side="response"]')
    .getByText("Auto · JSON", { exact: true })
    .waitFor({ state: "visible" });
  assert.match(
    await bodyText(page.locator('traffic-workspace message-inspector[side="response"]')),
    /"fixture": true/,
  );
  const requestHeaders = page.locator('traffic-workspace message-inspector[side="request"]');
  await requestHeaders.getByText("Header options", { exact: true }).click();
  await requestHeaders.getByLabel("Show byte sizes", { exact: true }).check();
  assert.equal(await requestHeaders.getByText("AUTH", { exact: true }).isVisible(), true);
  assert.match(await requestHeaders.locator(".field-help").first().textContent(), /2 headers/);
  await page.evaluate(
    () =>
      (globalThis.__workspaceFixture.completeCopiedUrl =
        "https://example.test/complete?query=" + "x".repeat(20000)),
  );
  await requestHeaders.getByRole("button", { name: "Copy URL", exact: true }).click();
  assert.equal(
    await page.evaluate(() => navigator.clipboard.readText()),
    "https://example.test/complete?query=" + "x".repeat(20000),
  );
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.lastCopiedUrl.boundary),
    "client-request",
  );
  assert.equal(
    await requestHeaders.locator(".inspector-actions > button").first().textContent(),
    "Copy URL",
  );
  await page.evaluate(() => {
    delete globalThis.__workspaceFixture.completeCopiedUrl;
  });
  await requestHeaders.getByRole("button", { name: "Largest first", exact: true }).click();
  assert.match(
    await requestHeaders.locator(".headers-table tbody tr").first().textContent(),
    /Authorization.*5.0 KB/,
  );
  assert.equal(
    await requestHeaders
      .getByRole("button", { name: "Largest first", exact: true })
      .getAttribute("aria-pressed"),
    "true",
  );
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/header-sizes-wide.png") });
  await page.setViewportSize({ width: 760, height: 520 });
  await page
    .getByRole("group", { name: "Message to inspect", exact: true })
    .getByRole("radio", { name: "Request", exact: true })
    .check();
  assert.equal(
    await requestHeaders.getByRole("button", { name: "Largest first", exact: true }).isVisible(),
    true,
  );
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/header-sizes-small.png") });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.evaluate(async () => {
    const state = globalThis.__workspaceFixture;
    state.fullHeaders = Array.from({ length: 700 }, (_, index) => ({
      name: "X-Field-" + index,
      value: "v",
      valueBytes: 1,
      fieldBytes: 18,
      sensitive: false,
      binary: false,
    }));
    state.fullHeaders.push(
      {
        name: "Cookie",
        value: "x".repeat(5000),
        valueBytes: 5000,
        fieldBytes: 5010,
        sensitive: false,
        binary: false,
      },
      {
        name: "Authorization",
        value: "[redacted]",
        valueBytes: 20,
        fieldBytes: 37,
        sensitive: true,
        binary: false,
      },
      {
        name: "Proxy-Authorization",
        value: "[redacted]",
        valueBytes: 20,
        fieldBytes: 43,
        sensitive: true,
        binary: false,
      },
    );
    await document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace")
      .inspectSession(state.sessions[1]);
  });
  await requestHeaders
    .getByText("Showing 1–512 of 703 headers", { exact: true })
    .waitFor({ state: "visible" });
  assert.equal(await requestHeaders.getByText("PROXY AUTH", { exact: true }).isVisible(), true);
  assert.match(
    await requestHeaders.locator(".headers-table tbody tr").first().textContent(),
    /Cookie.*4.9 KB/,
  );
  assert.match(
    await requestHeaders
      .locator(".headers-table tbody tr")
      .first()
      .locator("td")
      .nth(2)
      .getAttribute("title"),
    /5,000 bytes/,
  );
  await requestHeaders.getByRole("button", { name: "Largest first", exact: true }).click();
  await requestHeaders.getByRole("button", { name: "Next headers", exact: true }).click();
  await requestHeaders
    .getByText("Showing 513–703 of 703 headers", { exact: true })
    .waitFor({ state: "visible" });
  await requestHeaders.getByRole("button", { name: "Copy headers", exact: true }).click();
  const fullCopy = await page.evaluate(() => navigator.clipboard.readText());
  assert.equal(fullCopy.split("\r\n").length, 703);
  assert.ok(fullCopy.includes("Cookie: " + "x".repeat(5000)));
  assert.ok(fullCopy.includes("Proxy-Authorization: [redacted]"));
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/header-pages-wide.png") });
  await page.setViewportSize({ width: 760, height: 520 });
  await requestHeaders
    .getByRole("button", { name: "Previous headers", exact: true })
    .scrollIntoViewIfNeeded();
  const pagerBounds = await requestHeaders
    .getByRole("button", { name: "Previous headers", exact: true })
    .boundingBox();
  assert.ok(
    pagerBounds && pagerBounds.y >= 0 && pagerBounds.y + pagerBounds.height <= 520,
    "Header navigation is unreachable in a compact window",
  );
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/header-pages-small.png") });
  await requestHeaders.getByRole("button", { name: "Previous headers", exact: true }).click();
  await requestHeaders
    .getByText("Showing 1–512 of 703 headers", { exact: true })
    .waitFor({ state: "visible" });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.evaluate(async () => {
    const state = globalThis.__workspaceFixture;
    delete state.fullHeaders;
    await document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace")
      .inspectSession(state.sessions[0]);
    await document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace")
      .inspectSession(state.sessions[1]);
  });
  await page.locator('tr[data-session-id="second"]').click({ button: "right" });
  await openActionGroup(page.locator("#traffic-entry-menu"), "Copy");
  await page.getByRole("button", { name: "Copy as cURL", exact: true }).click();
  const commandDialog = page.locator(".request-command-dialog");
  await commandDialog
    .getByText("Command copied. Supply the body file before running it.", { exact: true })
    .waitFor({ state: "visible" });
  assert.equal(
    await commandDialog.getByLabel("Generated command").inputValue(),
    "fixture generated curl",
  );
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.calls.execute_composer ?? 0),
    0,
    "Copying a command executed a request",
  );
  await commandDialog.getByRole("button", { name: "Save as…", exact: true }).click();
  await commandDialog
    .getByText("Request body saved. Updated command copied.", { exact: true })
    .waitFor({ state: "visible" });
  assert.equal(
    await commandDialog.getByLabel("Generated command").inputValue(),
    "fixture saved body command",
  );
  assert.deepEqual(await page.evaluate(() => globalThis.__workspaceFixture.savedRequestBody), {
    id: "second",
    format: "curl",
  });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/request-command-wide.png"),
  });
  await page.setViewportSize({ width: 760, height: 520 });
  assert.equal(
    await commandDialog.getByRole("button", { name: "Close", exact: true }).isVisible(),
    true,
  );
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/request-command-small.png"),
  });
  await commandDialog.getByRole("button", { name: "Close", exact: true }).click();
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.evaluate(() => (globalThis.__workspaceFixture.directCommand = true));
  await page.locator('tr[data-session-id="second"]').click({ button: "right" });
  await openActionGroup(page.locator("#traffic-entry-menu"), "Copy");
  await page.getByRole("button", { name: "Copy as PowerShell", exact: true }).click();
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
        .commandBusy,
  );
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.lastRequestCommand.format),
    "powershell",
  );
  assert.equal(
    await commandDialog.isVisible(),
    false,
    "A direct clipboard command unnecessarily opened the body-save dialog",
  );
  assert.equal(
    await page.evaluate(() => navigator.clipboard.readText()),
    "fixture direct powershell",
  );
  await page.locator('tr[data-session-id="second"]').click({ button: "right" });
  await openActionGroup(page.locator("#traffic-entry-menu"), "Copy");
  await page.getByRole("button", { name: "Copy as cURL", exact: true }).click();
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
        .commandBusy,
  );
  assert.equal(await commandDialog.isVisible(), false);
  assert.equal(await page.evaluate(() => navigator.clipboard.readText()), "fixture direct curl");
  await page
    .locator(".selection-actions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await page.getByRole("button", { name: "Edit and replay", exact: true }).click();
  await page.locator("#composer").waitFor({ state: "visible" });
  assert.equal(
    await page.locator('#composer input[name="url"]').inputValue(),
    "http://example.test/second",
  );
  assert.equal(
    await page.locator('#composer textarea[name="headers"]').inputValue(),
    "Accept: */*",
  );
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.calls.execute_composer ?? 0),
    0,
    "Replay ran before explicit execution",
  );
  await page.locator('#composer input[name="url"]').fill("http://example.test/edited-request");
  await view("traffic");
  await page.locator('tr[data-session-id="first"]').click();
  await page
    .locator(".selection-actions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await page.getByRole("button", { name: "Edit and replay", exact: true }).click();
  await page
    .locator("composer-workspace dialog")
    .getByRole("button", { name: "Keep editing", exact: true })
    .click();
  assert.equal(
    await page.locator('#composer input[name="url"]').inputValue(),
    "http://example.test/edited-request",
    "Loading another request replaced edited content",
  );
  await view("traffic");
  await page
    .locator(".selection-actions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await page.getByRole("button", { name: "Edit and replay", exact: true }).click();
  await page
    .locator("composer-workspace dialog")
    .getByRole("button", { name: "Replace draft", exact: true })
    .click();
  await page.locator('#composer input[name="url"]').getAttribute("value");
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("composer-workspace")
        .composerSourceId === "first",
  );
  await page.locator('#composer input[name="method"]').fill("POST");
  assert.equal(await page.locator('#composer input[name="nonIdempotent"]').isVisible(), true);
  await page.locator('#composer input[name="method"]').fill("GET");
  assert.equal(await page.locator('#composer input[name="nonIdempotent"]').isVisible(), false);
  await page.locator('#composer textarea[name="headers"]').fill("  Authorization: explicit");
  assert.equal(await page.locator('#composer input[name="credentials"]').isVisible(), true);
  await page.locator('#composer textarea[name="headers"]').fill("Accept: */*");
  await page.evaluate(() => (globalThis.__workspaceFixture.composerError = true));
  await page.getByRole("button", { name: "Send request", exact: true }).click();
  await page
    .locator(".composer-feedback")
    .getByText("Send failed: Fixture send failed", { exact: true })
    .waitFor({ state: "visible" });
  assert.equal(
    await page.locator('#composer input[name="url"]').inputValue(),
    "http://example.test/first",
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.composerError = false;
    globalThis.__workspaceFixture.deferComposer = true;
  });
  await page.getByRole("button", { name: "Send request", exact: true }).click();
  await page.waitForFunction(() => globalThis.__workspaceFixture.composerPending);
  assert.equal(
    await page.getByRole("button", { name: "Sending…", exact: true }).isDisabled(),
    true,
  );
  await page.locator('#composer textarea[name="body"]').fill("new draft while sending");
  await page.evaluate(() => globalThis.__workspaceFixture.finishComposer());
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("composer-workspace")
        .composerResult?.status === 201,
  );
  assert.equal(
    await page.evaluate(
      () =>
        document.querySelector("app-shell").shadowRoot.querySelector("composer-workspace")
          .composerPane,
    ),
    "request",
    "Late responses interrupted request editing",
  );
  const composerInspector = page.locator("composer-workspace message-inspector");
  await waitForBodyText(composerInspector, "<script>");
  assert.equal(
    await page.evaluate(() => globalThis.unsafeComposer),
    undefined,
    "Response markup executed",
  );
  await composerInspector.getByRole("button", { name: "Headers", exact: true }).click();
  assert.equal(
    await composerInspector.locator(".headers-table tbody tr").count(),
    2,
    "Duplicate response headers were combined",
  );
  await composerInspector
    .locator(".message-tabs button[data-selected]")
    .getByText("Headers", { exact: true })
    .waitFor({ state: "visible" });
  const bodySpacing = await page.locator(".composer-body").evaluate((label) => {
    const range = document.createRange();
    range.selectNodeContents(label.firstElementChild);
    return (
      label.querySelector("textarea").getBoundingClientRect().top -
      range.getBoundingClientRect().bottom
    );
  });
  assert.ok(bodySpacing < 24, "Body label drifted away from its editor");
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/composer-wide.png") });
  await page.setViewportSize({ width: 760, height: 520 });
  await page
    .locator(".composer-pane-tabs")
    .getByRole("button", { name: "Response", exact: true })
    .click();
  await page
    .locator(".composer-pane-tabs button[data-selected]")
    .getByText("Response", { exact: true })
    .waitFor({ state: "visible" });
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/composer-small.png") });
  await page.setViewportSize({ width: 1280, height: 800 });

  await page.evaluate(async () => {
    const state = globalThis.__workspaceFixture;
    state.deferComposer = false;
    state.composerSourceOverride = {
      method: "POST",
      url: "http://example.test/large",
      headers: [{ name: "Content-Encoding", value: "gzip" }],
      body: "",
      bodyAvailable: true,
      bodyStreamed: true,
      bodyBytes: 8388609,
      notices: [],
    };
    const workspace = document
      .querySelector("app-shell")
      .shadowRoot.querySelector("composer-workspace");
    workspace.composerDirty = false;
    await workspace.populateRequest({ id: "first" });
  });
  await view("composer");
  await page.getByLabel("Body source", { exact: true }).selectOption("captured");
  assert.equal(await page.locator('#composer textarea[name="body"]').isVisible(), false);
  await page.getByText(/8,388,609 encoded bytes/).waitFor({ state: "visible" });
  await page.locator('#composer input[name="nonIdempotent"]').check();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.composerResultSource = {
      entryId: "first",
      traceId: "trace-fixture",
      originalId: "17",
    };
    globalThis.__workspaceFixture.composerHistory = [
      {
        id: 1,
        method: "POST",
        target: "http://example.test/edited",
        status: 201,
        source: { entryId: "first", traceId: "trace-fixture", originalId: "17" },
      },
    ];
  });
  await page.getByRole("button", { name: "Send request", exact: true }).click();
  await page
    .locator(".composer-feedback")
    .getByText("Response received: HTTP 201", { exact: true })
    .waitFor();
  assert.deepEqual(
    await page.evaluate(() => globalThis.__workspaceFixture.composerInput.bodySource),
    { kind: "captured", entryId: "first" },
  );
  assert.equal(await page.evaluate(() => globalThis.__workspaceFixture.composerInput.body), "");
  await composerInspector.getByRole("button", { name: "Raw data", exact: true }).click();
  assert.match(await composerInspector.locator(".exchange-details").textContent(), /trace-fixture/);
  await page.getByText("Replay history", { exact: true }).click();
  await page
    .locator(".composer-history")
    .getByText("trace-fixture · 17", { exact: true })
    .waitFor();
  await page.setViewportSize({ width: 760, height: 520 });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/composer-streamed-small.png"),
  });
  await page
    .locator(".composer-pane-tabs")
    .getByRole("button", { name: "Request", exact: true })
    .click();
  await page.getByLabel("Body source", { exact: true }).selectOption("inline");
  assert.equal(
    await page.getByRole("button", { name: "Send request", exact: true }).isDisabled(),
    true,
    "An empty inline replacement silently replaced the captured body",
  );
  await page.locator('#composer textarea[name="body"]').fill("edited body");
  await page.getByLabel("Body source", { exact: true }).selectOption("file");
  assert.equal(
    await page.getByRole("button", { name: "Send request", exact: true }).isDisabled(),
    true,
  );
  await page.evaluate(() => (globalThis.__workspaceFixture.capturePick = null));
  await page.getByRole("button", { name: "Choose body file…", exact: true }).click();
  await page
    .locator(".composer-feedback")
    .getByText("Body file selection canceled. Your draft is unchanged.", { exact: true })
    .waitFor();
  assert.equal(
    await page.getByRole("button", { name: "Send request", exact: true }).isDisabled(),
    true,
  );
  await page.evaluate(
    () => (globalThis.__workspaceFixture.capturePick = "C:/captures/request body.bin"),
  );
  await page.getByRole("button", { name: "Choose body file…", exact: true }).click();
  await page.getByText("C:/captures/request body.bin", { exact: true }).waitFor();
  assert.equal(
    await page.getByRole("button", { name: "Send request", exact: true }).isDisabled(),
    false,
  );
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/composer-file-small.png"),
  });
  await page.getByRole("button", { name: "Send request", exact: true }).click();
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("composer-workspace")
        .composerBusy,
  );
  assert.deepEqual(
    await page.evaluate(() => globalThis.__workspaceFixture.composerInput.bodySource),
    { kind: "file", path: "C:/captures/request body.bin" },
  );
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.composerInput.sourceEntryId),
    "first",
  );
  await page
    .locator(".composer-pane-tabs")
    .getByRole("button", { name: "Request", exact: true })
    .click();
  await page.getByLabel("Body source", { exact: true }).selectOption("inline");
  assert.equal(await page.locator('#composer textarea[name="body"]').inputValue(), "edited body");
  await page.getByLabel("Body source", { exact: true }).selectOption("captured");
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/composer-streamed-wide.png"),
  });
  await page.evaluate(() => {
    delete globalThis.__workspaceFixture.composerSourceOverride;
    globalThis.__workspaceFixture.capturePick = null;
  });

  await composerInspector.getByRole("button", { name: "Body", exact: true }).click();
  await page.evaluate(() => {
    const f = globalThis.__workspaceFixture;
    f.nextComposerType = "image/png";
    f.nextComposerBytes = Array.from(
      Uint8Array.from(
        atob(
          "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==",
        ),
        (char) => char.charCodeAt(0),
      ),
    );
  });
  await page.getByRole("button", { name: "Send request", exact: true }).click();
  await composerInspector.getByText("Auto · Image", { exact: true }).waitFor({ state: "visible" });
  await page.waitForFunction(
    () =>
      document
        .querySelector("app-shell")
        .shadowRoot.querySelector("composer-workspace message-inspector img")?.naturalWidth === 1,
  );
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/composer-image-wide.png"),
  });
  await composerInspector.getByRole("button", { name: "Save as…", exact: true }).click();
  await page.waitForFunction(
    () =>
      globalThis.__workspaceFixture.lastSave?.sessionId ===
      globalThis.__workspaceFixture.composerResponseDetail.id,
  );
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.lastSave.boundary),
    "client-response",
  );
  await page.evaluate(() => {
    const f = globalThis.__workspaceFixture;
    f.nextComposerType = "application/octet-stream";
    f.nextComposerBytes = Array.from({ length: 300019 }, (_, index) => index % 256);
  });
  await page.getByRole("button", { name: "Send request", exact: true }).click();
  await composerInspector.getByText("Auto · Hex", { exact: true }).waitFor({ state: "visible" });
  const composerHex = composerInspector.locator("hex-viewer"),
    composerGrid = composerHex.getByRole("grid");
  await composerGrid.press("Control+End");
  await composerHex
    .locator('[data-byte="300018"][data-column="hex"]')
    .getByText("F2", { exact: true })
    .waitFor({ state: "visible" });
  assert.equal(
    await composerHex.evaluate((element) => element.preview.source.totalBytes),
    "300019",
  );
  assert.ok((await composerHex.locator(".hex-data-row").count()) < 100);
  assert.doesNotMatch(
    await composerInspector.locator(".body-facts").textContent(),
    /preview truncated/,
  );
  await page.setViewportSize({ width: 760, height: 520 });
  await page
    .locator(".composer-pane-tabs")
    .getByRole("button", { name: "Response", exact: true })
    .click();
  await composerGrid.press("Control+End");
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/composer-hex-small.png") });
  assert.ok(
    await composerGrid.evaluate((element) => element.clientHeight > 40),
    "Compact Composer left no space for bytes",
  );
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.evaluate(() => {
    delete globalThis.__workspaceFixture.nextComposerBytes;
    delete globalThis.__workspaceFixture.nextComposerType;
  });

  await view("traffic");
  await page.locator('tr[data-session-id="cached"]').click();
  await page
    .locator('traffic-workspace message-inspector[side="response"] .body-notice')
    .getByText("304 Not Modified", { exact: false })
    .waitFor({ state: "visible" });
  await page.locator('tr[data-session-id="image"]').click();
  await page
    .locator('traffic-workspace message-inspector[side="response"]')
    .getByText("Auto · Image", { exact: true })
    .waitFor({ state: "visible" });
  await page
    .locator('traffic-workspace message-inspector[side="response"] img')
    .waitFor({ state: "visible" });
  const responseInspector = page.locator('traffic-workspace message-inspector[side="response"]');
  assert.equal(await page.getByRole("button", { name: "Copy body", exact: true }).count(), 0);
  const saveBody = responseInspector.getByRole("button", { name: "Save as…", exact: true });
  await page.evaluate(() => (globalThis.__workspaceFixture.holdSaveBody = true));
  await saveBody.click();
  await responseInspector
    .getByRole("button", { name: "Saving…", exact: true })
    .waitFor({ state: "visible" });
  assert.equal(
    await responseInspector.getByRole("button", { name: "Saving…", exact: true }).isEnabled(),
    false,
  );
  await page.evaluate(() => globalThis.__workspaceFixture.finishSaveBody());
  await page
    .locator(".global-diagnostics")
    .getByText("Saved original.webp (4 B).", { exact: true })
    .waitFor({ state: "visible" });
  assert.deepEqual(
    await page.evaluate(() => globalThis.__workspaceFixture.lastSave),
    { sessionId: "image", boundary: "client-response" },
    "Save as used preview bytes or a preview URL",
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.saveCancelled = true;
    globalThis.__workspaceFixture.holdSaveBody = true;
  });
  await saveBody.click();
  await responseInspector
    .getByRole("button", { name: "Saving…", exact: true })
    .waitFor({ state: "visible" });
  await page.evaluate(() => globalThis.__workspaceFixture.finishSaveBody());
  await saveBody.waitFor({ state: "visible" });
  assert.equal(await saveBody.isEnabled(), true, "Cancelling Save as did not restore the control");
  assert.equal(
    await page.locator(".global-diagnostics").textContent(),
    "Saved original.webp (4 B).",
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.saveCancelled = false;
    globalThis.__workspaceFixture.saveError = true;
  });
  await saveBody.click();
  await page
    .locator(".notice")
    .getByText("Response save failed", { exact: true })
    .waitFor({ state: "visible" });
  assert.match(await page.locator(".notice").textContent(), /Fixture file write failed/);
  await page.getByRole("button", { name: "Dismiss notification", exact: true }).click();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.saveError = false;
  });
  const originalCodingDetail = await page.evaluate(() =>
    structuredClone(
      document
        .querySelector("app-shell")
        .shadowRoot.querySelector('message-inspector[side="response"]').detail,
    ),
  );
  const codingFactsWasOpen = await responseInspector
    .locator(".body-facts")
    .evaluate((details) => details.open);
  if (!codingFactsWasOpen) await responseInspector.locator(".body-facts > summary").click();
  for (const codings of [
    ["x-gzip", "br"],
    ["X-GZIP", "BR"],
    ["gzip", "x-gzip"],
  ]) {
    await page.evaluate((codings) => {
      const inspector = document
          .querySelector("app-shell")
          .shadowRoot.querySelector('message-inspector[side="response"]'),
        detail = structuredClone(inspector.detail);
      const override = (globalThis.__workspaceFixture.bodyOverride = {
        contentCodings: codings,
        wireBodyBytes: 17,
      });
      Object.assign(detail.storedBodies[0], override);
      inspector.detail = detail;
    }, codings);
    await responseInspector
      .locator(".body-facts")
      .getByText(`17 wire body bytes (${codings.join(", ")}, excludes framing/TLS)`, {
        exact: false,
      })
      .waitFor();
    assert.deepEqual(
      await page.evaluate(
        () =>
          document
            .querySelector("app-shell")
            .shadowRoot.querySelector('message-inspector[side="response"]').bodyMetadata
            .contentCodings,
      ),
      codings,
      "Display normalization mutated captured metadata",
    );
  }
  await page.evaluate((detail) => {
    delete globalThis.__workspaceFixture.bodyOverride;
    document
      .querySelector("app-shell")
      .shadowRoot.querySelector('message-inspector[side="response"]').detail = detail;
  }, originalCodingDetail);
  if (!codingFactsWasOpen) await responseInspector.locator(".body-facts > summary").click();
  await page.evaluate(() => {
    const inspector = document
      .querySelector("app-shell")
      .shadowRoot.querySelector('message-inspector[side="response"]');
    const detail = structuredClone(inspector.detail);
    const override = (globalThis.__workspaceFixture.bodyOverride = {
      availability: "lost",
      observedBytes: 8,
      retainedBytes: 4,
      contentCodings: ["gzip"],
      reason: "exchange failed at ResponseBody before completion",
    });
    Object.assign(detail.storedBodies[0], override);
    inspector.detail = detail;
  });
  await responseInspector
    .getByText("Preview unavailable:", { exact: false })
    .waitFor({ state: "visible" });
  assert.match(
    await responseInspector.locator(".body-facts").textContent(),
    /4 B retained \/ 8 B observed · lost/,
  );
  assert.match(
    await responseInspector.locator(".body-notice").textContent(),
    /Reason: exchange failed at ResponseBody/,
  );
  assert.match(
    await responseInspector.locator(".body-notice").textContent(),
    /Raw Hex shows the 4 B retained bytes/,
  );
  assert.equal(await saveBody.isEnabled(), false, "Save as accepts an incomplete response");
  await responseInspector.getByLabel("Body viewer", { exact: true }).selectOption("bytes");
  await responseInspector
    .locator(".body-facts strong")
    .getByText("Hex", { exact: true })
    .waitFor({ state: "visible" });
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.lastRangeRequest.offset),
    "0",
    "Hex did not request raw retained bytes",
  );
  assert.equal(await responseInspector.getByLabel("Decode", { exact: true }).isChecked(), false);
  assert.equal(await responseInspector.getByLabel("Decode", { exact: true }).isEnabled(), false);
  await checkHexViewer(
    page,
    responseInspector,
    resolve(root, "../../../target/ui-check/hex-selection.png"),
  );
  await responseInspector.getByLabel("Body viewer", { exact: true }).selectOption("metadata");
  assert.match(
    await responseInspector.locator(".body-preview").textContent(),
    /"reason": "exchange failed at ResponseBody/,
  );
  await page.evaluate(() => {
    const inspector = document
      .querySelector("app-shell")
      .shadowRoot.querySelector('message-inspector[side="response"]');
    const detail = structuredClone(inspector.detail);
    Object.assign(detail.storedBodies[0], {
      availability: "truncated",
      observedBytes: 1851,
      retainedBytes: 1800,
      reason: "per-body retention limit reached",
    });
    inspector.detail = detail;
  });
  assert.match(
    await responseInspector.locator(".body-facts").textContent(),
    /1,800 B retained \/ 1,851 B observed/,
    "Rounded counts hid missing body bytes",
  );
  await page.evaluate(() => {
    const inspector = document
      .querySelector("app-shell")
      .shadowRoot.querySelector('message-inspector[side="response"]');
    const detail = structuredClone(inspector.detail);
    Object.assign(detail.storedBodies[0], {
      availability: "evicted",
      retainedBytes: 0,
      reason: "evicted by circular response-body quota",
    });
    inspector.detail = detail;
  });
  assert.match(
    await responseInspector.locator(".body-preview").textContent(),
    /"availability": "evicted"/,
    "Metadata cannot inspect a body without retained bytes",
  );
  await responseInspector.getByLabel("Body viewer", { exact: true }).selectOption("auto");
  assert.match(
    await responseInspector.locator(".body-notice").textContent(),
    /removed to make room/,
  );
  assert.doesNotMatch(
    await responseInspector.locator(".body-notice").textContent(),
    /Hex/,
    "Unavailable bytes incorrectly recommend Hex",
  );
  assert.equal(await saveBody.isEnabled(), false, "Save as accepts evicted response bytes");
  await page.evaluate(() => delete globalThis.__workspaceFixture.bodyOverride);
  await page.locator('tr[data-session-id="second"]').click();
  await page
    .locator('traffic-workspace message-inspector[side="response"]')
    .getByText("Auto · JSON", { exact: true })
    .waitFor({ state: "visible" });
  const heldOrder = await page
    .locator(".traffic-table tbody tr[data-session-id]")
    .evaluateAll((rows) => rows.map((row) => row.dataset.sessionId));
  await page.evaluate(async () => {
    const state = globalThis.__workspaceFixture;
    state.sessions.push({
      ...state.sessions[0],
      id: "new",
      url: "http://example.test/new",
      path: "/new",
      startedAt: 9999,
    });
    await document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace")
      .refreshSessions();
  });
  assert.deepEqual(
    await page
      .locator(".traffic-table tbody tr[data-session-id]")
      .evaluateAll((rows) => rows.map((row) => row.dataset.sessionId)),
    heldOrder,
    "Live arrival moved rows during inspection",
  );
  await page.locator(".new-traffic-button").waitFor({ state: "visible" });
  assert.match(await page.locator(".new-traffic-button").textContent(), /1\s+new/);
  await page.locator(".new-traffic-button").click();
  assert.equal(
    await page
      .locator(".traffic-table tbody tr[data-session-id]")
      .first()
      .getAttribute("data-session-id"),
    "new",
  );
  await page.evaluate(async () => {
    const state = globalThis.__workspaceFixture;
    Object.assign(state.sessions[0], { status: null, terminal: "active", responseBytes: 0 });
    await document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace")
      .refreshSessions(undefined, true);
  });
  await page.locator('tr[data-session-id="first"]').click();
  await page
    .locator(".selection-bar")
    .getByText("Pending", { exact: true })
    .waitFor({ state: "visible" });
  await page.evaluate(() => {
    const state = globalThis.__workspaceFixture;
    Object.assign(state.sessions[0], { status: 200, terminal: "completed", responseBytes: 4 });
    state.channel.onmessage({ exchangeId: "first", sequence: 2, lagged: false });
  });
  await waitForBodyText(
    page.locator('traffic-workspace message-inspector[side="response"]'),
    "body for first",
  );
  await checkTrafficFailures(page, resolve(root, "../../../target/ui-check"));
  await page.locator('tr[data-session-id="second"]').click();
  await page
    .locator('traffic-workspace message-inspector[side="response"]')
    .getByText("Auto · JSON", { exact: true })
    .waitFor({ state: "visible" });
  await page
    .locator(".selection-actions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await openActionGroup(page.locator("#selected-request-actions"), "Create rule");
  assert.equal(
    await page.getByRole("button", { name: "Serve a saved response…", exact: true }).isEnabled(),
    true,
  );
  await page.getByRole("button", { name: "Serve a saved response…", exact: true }).click();
  await page.locator("#automation").waitFor({ state: "visible" });
  await page.locator(".auto-response-editor").waitFor({ state: "visible" });
  await page
    .locator("#auto-response-body-help")
    .getByText("Complete body preview. Enable editing or open the dialog to replace it.", {
      exact: true,
    })
    .waitFor({ state: "visible" });
  const autoEditor = page.locator(".auto-response-editor");
  const sourceLink = page.locator(".auto-response-source a");
  const autoField = (name) => autoEditor.locator('[name="' + name + '"]');
  assert.equal(
    await page.getByRole("button", { name: "New rule…", exact: true }).isVisible(),
    true,
  );
  assert.equal(
    await page
      .getByRole("button", { name: "Create saved response from scratch…", exact: true })
      .isVisible(),
    false,
  );
  assert.equal(await autoField("body").getAttribute("rows"), "12");
  assert.equal(await autoField("responseHeaders").getAttribute("rows"), "10");
  assert.equal(
    await autoField("status").evaluate(
      (el) => el.closest("details").querySelector("summary").textContent,
    ),
    "Response",
  );
  assert.equal(
    await page
      .locator(".auto-response-source")
      .getByText("Original response context", { exact: true })
      .isVisible(),
    true,
  );
  assert.equal(
    await autoField("body").evaluate((body) => getComputedStyle(body).resize),
    "vertical",
  );
  assert.match(
    await autoField("body").evaluate((body) => getComputedStyle(body).fontFamily),
    /Mono|Consolas/,
  );
  assert.equal(
    await autoField("name").evaluate((input) => input.getRootNode().activeElement === input),
    true,
    "Rule editor did not receive focus",
  );
  await sourceLink
    .getByText("GET example.test/second", { exact: true })
    .waitFor({ state: "visible" });
  assert.equal(
    await autoField("status").getAttribute("readonly"),
    null,
    "Captured status requires an unnecessary first save",
  );
  await autoField("name").fill("Changed captured draft");
  await autoField("status").fill("201");
  const capturedPreview = await autoField("body").inputValue();
  await autoEditor.getByRole("button", { name: "Discard changes", exact: true }).click();
  await page
    .locator("#auto-response-body-help")
    .getByText("Complete body preview. Enable editing or open the dialog to replace it.", {
      exact: true,
    })
    .waitFor();
  assert.equal(await autoField("status").inputValue(), "200");
  assert.equal(await autoField("body").inputValue(), capturedPreview);
  assert.equal(await autoEditor.locator("h3").textContent(), "New rule from captured response");
  assert.equal(await sourceLink.isVisible(), true, "Revert lost captured source");
  await autoField("status").fill("199");
  await autoEditor.locator(".response-group > summary").click();
  await autoEditor.getByRole("button", { name: "Save rule", exact: true }).click();
  assert.equal(
    await autoEditor.locator(".response-group").getAttribute("open"),
    "",
    "Invalid hidden status did not reopen Response",
  );
  assert.equal(
    await autoField("status").evaluate((el) => el.getRootNode().activeElement === el),
    true,
  );
  await autoField("status").fill("200");
  assert.equal(
    await autoEditor.getByRole("button", { name: "Discard changes", exact: true }).isDisabled(),
    true,
    "Restoring original values must clear dirty state",
  );
  await page.setViewportSize({ width: 1440, height: 900 });
  await autoField("responseHeaders").scrollIntoViewIfNeeded();
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/auto-response-response-wide.png"),
  });
  await page.setViewportSize({ width: 800, height: 600 });
  await autoEditor.evaluate((form) => (form.closest(".rule-properties-pane").scrollTop = 0));
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/auto-response-response-compact.png"),
  });
  await page.setViewportSize({ width: 1280, height: 800 });

  for (const codings of [
    ["x-gzip", "br"],
    ["X-GZIP", "BR"],
    ["gzip", "x-gzip"],
  ]) {
    await page.evaluate(async (codings) => {
      const w = document
          .querySelector("app-shell")
          .shadowRoot.querySelector("automation-workspace"),
        detail = await window.__TAURI_INTERNALS__.invoke("session_detail", { id: "second" });
      detail.storedBodies[0].contentCodings = codings;
      await w.populateCapturedAutoResponse("second", detail);
    }, codings);
    const expected = codings[1].toLowerCase() === "br" ? "gzip, Brotli" : "gzip, gzip";
    assert.match(
      await page.locator(".auto-response-source").textContent(),
      new RegExp(codings.join(", ")),
      "Captured response context changed the server's coding token",
    );
    await autoField("editBody").check();
    await autoEditor
      .getByText(`Apply original compression type (${expected})`, { exact: false })
      .waitFor();
    assert.doesNotMatch(
      await autoField("preserveEncoding").evaluate((input) => input.closest("label").textContent),
      /x-gzip/i,
      "Compression control exposed a separate alias label",
    );
    assert.equal(
      await autoField("preserveEncoding").isEnabled(),
      true,
      "Recognized compression aliases were classified as unsupported",
    );
    assert.deepEqual(
      await page.evaluate(
        () =>
          document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace")
            .autoResponseSourceState.contentCodings,
      ),
      codings,
      "Context label normalization changed the saved compression stack",
    );
    await autoEditor.getByRole("button", { name: "Discard changes", exact: true }).click();
    await page.waitForFunction(() => {
      const w = document
        .querySelector("app-shell")
        .shadowRoot.querySelector("automation-workspace");
      return !w.draftDirty && !w.loadingSavedResponse && w.autoResponseSourceState?.bodyEditable;
    });
  }

  for (const [url, expected] of [
    ["http://example.test:80/second", "http://example.test/second"],
    ["https://example.test:443/second", "https://example.test/second"],
    ["https://example.test:8443/second", "https://example.test:8443/second"],
  ]) {
    await page.evaluate(async (url) => {
      const w = document
        .querySelector("app-shell")
        .shadowRoot.querySelector("automation-workspace");
      const detail = await window.__TAURI_INTERNALS__.invoke("session_detail", { id: "second" });
      detail.requests[0].target = url;
      await w.populateCapturedAutoResponse("second", detail);
    }, url);
    assert.equal(
      await autoField("url").inputValue(),
      expected,
      "Traffic-derived URL has the wrong port",
    );
  }
  const rawCapturedHeaders = ["alpha \t", [255], "omega"].map((value) => ({
    name: Array.from(Buffer.from("X-Repeat")),
    value: Array.isArray(value) ? value : Array.from(Buffer.from(value)),
  }));
  await page.evaluate(async (headers) => {
    globalThis.__workspaceFixture.rawResponseHeaders = headers;
    const w = document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace");
    await w.populateCapturedAutoResponse(
      "second",
      await window.__TAURI_INTERNALS__.invoke("session_detail", { id: "second" }),
    );
  }, rawCapturedHeaders);
  await autoField("status").fill("201");
  await autoEditor.getByRole("button", { name: "Save rule", exact: true }).click();
  await savedProperties();
  assert.deepEqual(
    await page.evaluate(() => globalThis.__workspaceFixture.lastAssetEdit.headers),
    rawCapturedHeaders,
    "Metadata-only captured Save changed header bytes or repeated-field order",
  );
  await autoField("responseHeaders").fill("X-Repeat: alpha\nx-repeat: changed");
  await autoEditor.getByRole("button", { name: "Save rule", exact: true }).click();
  await savedProperties();
  assert.deepEqual(
    await page.evaluate(() =>
      globalThis.__workspaceFixture.lastAssetEdit.headers.map((field) => field.value),
    ),
    [rawCapturedHeaders[0].value, [255], Array.from(Buffer.from("changed"))],
    "Editing a repeated text value moved the protected value",
  );
  assert.equal(
    await page.evaluate(() =>
      new TextDecoder().decode(
        new Uint8Array(globalThis.__workspaceFixture.lastAssetEdit.headers[2].name),
      ),
    ),
    "x-repeat",
    "Merging headers lost an authored capitalization change",
  );
  await autoField("responseHeaders").fill("X-Repeat: changed");
  await autoField("status").fill("200");
  await autoEditor.getByRole("button", { name: "Save rule", exact: true }).click();
  await savedProperties();
  assert.deepEqual(
    await page.evaluate(() =>
      globalThis.__workspaceFixture.lastAssetEdit.headers.map((field) => field.value),
    ),
    [[255], Array.from(Buffer.from("changed"))],
    "Deleting an earlier text header swapped a protected repeated value",
  );
  await page.evaluate(() => delete globalThis.__workspaceFixture.rawResponseHeaders);
  const capturedRuleId = await page.locator(".auto-response-rule").getAttribute("data-rule-id");
  const capturedRule = page.locator("#rule-" + capturedRuleId);
  await autoEditor.locator("rule-activation button").click();
  await capturedRule
    .locator(".rule-state")
    .getByText("Disabled", { exact: true })
    .waitFor({ state: "visible" });
  await capturedRule.click();
  await sourceLink.waitFor({ state: "visible" });
  assert.equal(
    await autoField("status").inputValue(),
    "200",
    "Saved response metadata was hidden or lost",
  );
  assert.equal(
    await autoField("body").isVisible(),
    true,
    "Saved response body cannot be edited independently of Traffic",
  );
  await autoField("name").fill("Disabled captured response");
  await autoEditor.getByRole("button", { name: "Save rule", exact: true }).click();
  await savedProperties();
  assert.equal(
    await capturedRule.locator(".rule-state").textContent(),
    "Disabled",
    "Editing silently enabled a disabled rule",
  );
  const previewSourceBefore = await page.evaluate(() => {
    const t = document.querySelector("app-shell").traffic,
      detail = structuredClone(t.selectedDetail),
      html = structuredClone(detail),
      row = globalThis.__workspaceFixture.sessions.find((row) => row.id === detail.id),
      contentType = row.contentType;
    row.contentType = "text/html";
    html.storedBodies[0].mediaType = "text/html";
    t.applyDetail(html);
    return { detail, contentType };
  });
  await autoEditor.getByRole("button", { name: "Preview…", exact: true }).click();
  await autoEditor.getByRole("button", { name: "With saved changes…", exact: true }).click();
  await page.waitForFunction(() => {
    const t = document.querySelector("app-shell").traffic;
    return t.previewPageDialog.open && !t.previewRulesBusy;
  });
  assert.equal(
    await page
      .locator(".captured-page-warning[open]")
      .getByLabel("Responses", { exact: true })
      .inputValue(),
    "modified",
  );
  assert.equal(
    await page.evaluate(() => {
      const shell = document.querySelector("app-shell");
      return shell.traffic.previewRules.find(
        (row) => row.id === shell.automation.editingAutoResponseId,
      )?.included;
    }),
    true,
    "Saved disabled rule was not selected by editor preview",
  );
  await page.keyboard.press("Escape");
  await page.evaluate(({ detail, contentType }) => {
    globalThis.__workspaceFixture.sessions.find((row) => row.id === detail.id).contentType =
      contentType;
    document.querySelector("app-shell").traffic.applyDetail(detail);
  }, previewSourceBefore);
  await view("automation");
  // Both views share one gate, preserving each rule's own enabled state.
  await page.locator("#automation autoresponse-switch button").click();
  await page.waitForFunction(
    () => globalThis.__workspaceFixture.automation.autoresponsesEnabled === false,
  );
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.automation.rules[0].enabled),
    false,
  );
  await view("traffic");
  assert.equal(
    await page.locator("#traffic autoresponse-switch button").count(),
    0,
    "Traffic shows an automation switch when no response rules are enabled",
  );
  await view("automation");
  await page.locator("#automation autoresponse-switch button").click();
  await page.waitForFunction(
    () => globalThis.__workspaceFixture.automation.autoresponsesEnabled === true,
  );
  await page
    .locator("#automation autoresponse-switch button")
    .getByText(/On/)
    .waitFor({ state: "visible" });
  await capturedRule.click();
  await sourceLink.waitFor({ state: "visible" });

  // Reveal a source on a later page even when current filters exclude it.
  const originalSourceRows = await page.evaluate(async () => {
    const state = globalThis.__workspaceFixture;
    const rows = structuredClone(state.sessions);
    for (let index = 0; index < 130; index++)
      state.sessions.push({
        ...rows[0],
        id: "source-page-" + index,
        path: "/filler/" + index,
        url: "http://example.test/filler/" + index,
        startedAt: 2000 + index,
      });
    const traffic = document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace");
    traffic.sort = { column: "started-at", direction: "descending" };
    traffic.filters = [{ column: "status", operator: "equals", value: "304" }];
    traffic.searchText = "cached";
    traffic.searchInput.value = "cached";
    await traffic.refreshSessions(undefined, true);
    state.deferFocus = true;
    return rows;
  });
  await sourceLink.click();
  await page.waitForFunction(() => globalThis.__workspaceFixture.focusPending);
  await page.evaluate(() => {
    const state = globalThis.__workspaceFixture;
    state.deferRefresh = true;
    globalThis.__pendingRefresh = document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace")
      .refreshSessions(undefined, true);
  });
  await page.waitForFunction(() => globalThis.__workspaceFixture.refreshPending);
  await page.evaluate(() => globalThis.__workspaceFixture.releaseFocus());
  await page.locator("#traffic").waitFor({ state: "visible" });
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
        .selectedSessionId === "second" &&
      document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
        .sessionScroller.scrollTop > 0,
  );
  await page.evaluate(async () => {
    globalThis.__workspaceFixture.releaseRefresh();
    await globalThis.__pendingRefresh;
  });
  assert.equal(
    await page.locator('tr[data-session-id="second"]').getAttribute("aria-selected"),
    "true",
  );
  assert.equal(
    await page.evaluate(
      () =>
        document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
          .totalMatched,
    ),
    originalSourceRows.length + 130,
    "A late filtered refresh replaced the source page",
  );
  assert.deepEqual(
    await page.evaluate(() => {
      const traffic = document
        .querySelector("app-shell")
        .shadowRoot.querySelector("traffic-workspace");
      return { filters: traffic.filters, search: traffic.searchText };
    }),
    { filters: [], search: "" },
  );
  await view("automation");
  await sourceLink.waitFor({ state: "visible" });
  await page.evaluate(async (rows) => {
    const state = globalThis.__workspaceFixture;
    state.sessions = rows.filter((row) => row.id !== "second");
    state.evictedIds = ["second"];
    await document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace")
      .refreshSessions(undefined, true);
  }, originalSourceRows);
  await sourceLink.waitFor({ state: "detached" });
  await page
    .locator(".auto-response-source")
    .getByText("Source no longer in Traffic.", { exact: true })
    .waitFor({ state: "visible" });
  assert.match(
    await page.locator(".auto-response-source").textContent(),
    /Saved response/,
    "Eviction removed the durable response metadata",
  );
  await cancelRule();
  await capturedRule.click();
  await page
    .locator(".auto-response-source")
    .getByText("Source no longer in Traffic.", { exact: true })
    .waitFor({ state: "visible" });
  assert.equal(await sourceLink.count(), 0, "Reopening an evicted source restored a broken link");
  await autoField("body").fill("Edited after source removal");
  await autoField("status").fill("201");
  await autoEditor.getByRole("button", { name: "Save rule", exact: true }).click();
  await savedProperties();
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.lastAssetEdit.decodedBody.length),
    "Edited after source removal".length,
  );
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.assets[0].display),
    "body for second",
    "Editing rewrote historical saved bytes",
  );
  await capturedRule.click();
  await page
    .locator(".auto-response-source")
    .getByText("Source no longer in Traffic.", { exact: true })
    .waitFor({ state: "visible" });
  assert.equal(await autoField("status").inputValue(), "201");
  assert.equal(await autoField("body").inputValue(), "Edited after source removal");
  await page.evaluate(async (rows) => {
    const state = globalThis.__workspaceFixture;
    state.sessions = rows;
    state.evictedIds = [];
    const traffic = document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace");
    await traffic.refreshSessions(undefined, true);
  }, originalSourceRows);
  await sourceLink.waitFor({ state: "visible" });
  await cancelRule();

  // A slow captured-response preview cannot replace a newer draft.
  await page.evaluate(() => {
    const state = globalThis.__workspaceFixture;
    state.deferBody = "second";
    globalThis.__pendingAutoResponse = document
      .querySelector("app-shell")
      .shadowRoot.querySelector("automation-workspace")
      .beginAutoResponseFromSelected();
  });
  await page.waitForFunction(() => globalThis.__workspaceFixture.bodyPending);
  await newScratch();
  await autoField("body").fill("New scratch draft");
  await page.evaluate(async () => {
    globalThis.__workspaceFixture.deferBody = null;
    globalThis.__workspaceFixture.releaseBody();
    await globalThis.__pendingAutoResponse;
  });
  assert.equal(
    await autoField("body").inputValue(),
    "New scratch draft",
    "Late captured body overwrote a newer draft",
  );
  assert.equal(await autoField("body").evaluate((body) => body.readOnly), false);
  await autoField("name").fill("Authored response");
  await autoField("url").fill("http://example.test/authored");
  await autoField("method").selectOption("POST");
  await autoField("requestHeaders").fill("Content-Type: application/json");
  await autoField("method").selectOption("GET");
  await autoField("method").selectOption("POST");
  assert.equal(
    await autoField("requestHeaders").inputValue(),
    "Content-Type: application/json",
    "Method switch discarded the header draft",
  );
  const assetsBeforeInvalid = await page.evaluate(
    () => globalThis.__workspaceFixture.assets.length,
  );
  await autoField("requestHeaders").fill("Not a header");
  await autoEditor.getByRole("button", { name: "Save rule", exact: true }).click();
  await page.locator("#auto-request-headers-error").waitFor({ state: "visible" });
  assert.equal(
    await autoField("requestHeaders").evaluate((input) => input.validity.customError),
    true,
  );
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.assets.length),
    assetsBeforeInvalid,
    "Invalid matcher created an unused response asset",
  );
  await autoField("requestHeaders").fill("Content-Type: application/json");
  if (
    !(await autoEditor
      .locator('details:has([name="responseHeaders"])')
      .evaluate((details) => details.open))
  )
    await autoEditor
      .locator('details:has([name="responseHeaders"])')
      .locator("summary")
      .first()
      .click();
  await autoField("responseHeaders").fill("Also not a header");
  await autoEditor.getByRole("button", { name: "Save rule", exact: true }).click();
  await page.locator("#auto-response-headers-error").waitFor({ state: "visible" });
  if (
    !(await autoEditor
      .locator('details:has([name="responseHeaders"])')
      .evaluate((details) => details.open))
  )
    await autoEditor
      .locator('details:has([name="responseHeaders"])')
      .locator("summary")
      .first()
      .click();
  await autoField("responseHeaders").fill(
    "X-Fixture: yes\nContent-Length: 999\nContent-Encoding: gzip\nContent-Type: incorrect/type",
  );
  await autoField("body").fill("Authored response body");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.assetError = true;
  });
  await autoEditor.getByRole("button", { name: "Save rule", exact: true }).click();
  await autoEditor
    .getByText("Fixture asset write failed", { exact: true })
    .waitFor({ state: "visible" });
  assert.equal(
    await autoField("body").inputValue(),
    "Authored response body",
    "Failed save lost the body draft",
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.assetError = false;
    globalThis.__workspaceFixture.assetDelay = 400;
  });
  await autoEditor.getByRole("button", { name: "Save rule", exact: true }).click({ clickCount: 2 });
  await autoEditor
    .getByRole("button", { name: "Saving…", exact: true })
    .waitFor({ state: "visible" });
  assert.equal(
    await page.getByRole("button", { name: "New rule…", exact: true }).isEnabled(),
    false,
    "Creation remains available during a save",
  );
  assert.equal(await autoField("body").isEnabled(), false, "Body can change during a save");
  await savedProperties();
  assert.equal(
    await page.evaluate(
      () =>
        globalThis.__workspaceFixture.automation.rules.filter(
          (rule) => rule.displayName === "Authored response",
        ).length,
    ),
    1,
    "Double submit created duplicate rules",
  );
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.assets.length),
    assetsBeforeInvalid + 1,
  );
  assert.deepEqual(
    await page.evaluate(() => {
      const asset = globalThis.__workspaceFixture.assets.find(
        (asset) =>
          asset.id + "@" + asset.revision ===
          globalThis.__workspaceFixture.automation.rules.find(
            (rule) => rule.displayName === "Authored response",
          ).request.responseAsset,
      );
      return asset.headers.map((header) => new TextDecoder().decode(new Uint8Array(header.name)));
    }),
    ["X-Fixture"],
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.assetDelay = 0;
  });

  await checkResponseBodyEditor(page, root);
  await checkBinaryResponse(page, root);
  await page.evaluate(async () => {
    const s = globalThis.__workspaceFixture;
    s.assetCodings = ["X-GZIP", "BR"];
    s.assetTextEncoding = null;
    const w = document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace");
    await w.editAutoResponse(
      w.rules.find((rule) => rule.id === w.editingAutoResponseId),
      true,
    );
  });
  await autoEditor
    .getByText("Apply original compression type (gzip, Brotli)", { exact: false })
    .waitFor();
  assert.equal(
    await autoField("preserveEncoding").isEnabled(),
    true,
    "Complete binary bodies can apply original compression",
  );
  await page.evaluate(() => (globalThis.__workspaceFixture.bodyFile = "C:/fixtures/binary.bin"));
  await autoEditor
    .getByRole("button", { name: "Serve response body from file…", exact: true })
    .click();
  assert.equal(
    await autoField("preserveEncoding").isEnabled(),
    true,
    "Binary file replacement cannot apply original compression",
  );
  await autoEditor.getByRole("button", { name: "Clear file choice", exact: true }).click();
  await page.evaluate(async () => {
    const s = globalThis.__workspaceFixture;
    s.assetCodings = ["compress"];
    const w = document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace");
    await w.editAutoResponse(
      w.rules.find((rule) => rule.id === w.editingAutoResponseId),
      true,
    );
  });
  await autoEditor
    .getByRole("button", { name: "Serve response body from file…", exact: true })
    .click();
  assert.equal(
    await autoField("preserveEncoding").isChecked(),
    false,
    "Unsupported compression traps replacement in an unusable checked state",
  );
  assert.equal(await autoField("preserveEncoding").isDisabled(), true);
  await autoEditor.getByText(/Original compression is unsupported/).waitFor();

  await autoEditor.getByRole("button", { name: "Clear file choice", exact: true }).click();
  await page.evaluate(async () => {
    const s = globalThis.__workspaceFixture;
    s.assetCodings = ["constructor"];
    const w = document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace");
    await w.editAutoResponse(
      w.rules.find((rule) => rule.id === w.editingAutoResponseId),
      true,
    );
  });
  await autoEditor
    .getByRole("button", { name: "Serve response body from file…", exact: true })
    .click();
  await autoEditor
    .getByText("Apply original compression type (constructor)", { exact: false })
    .waitFor();
  assert.equal(
    await autoField("preserveEncoding").isChecked(),
    false,
    "Unknown coding inherited a supported label from the object prototype",
  );
  assert.equal(await autoField("preserveEncoding").isDisabled(), true);
  await autoEditor.getByRole("button", { name: "Clear file choice", exact: true }).click();
  await page.evaluate(async () => {
    const s = globalThis.__workspaceFixture;
    delete s.assetCodings;
    delete s.assetTextEncoding;
    const w = document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace");
    await w.editAutoResponse(
      w.rules.find((rule) => rule.id === w.editingAutoResponseId),
      true,
    );
  });
  const originalFileDraft = await autoField("body").inputValue();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.bodyFile = "C:/fixtures/small.txt";
    globalThis.__workspaceFixture.bodyFileBytes = 25000000;
  });
  await autoEditor
    .getByRole("button", { name: "Serve response body from file…", exact: true })
    .click();
  assert.equal(await autoField("body").isDisabled(), true);
  await autoEditor.getByText(/Contents will be copied when you save/).waitFor();
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/auto-response-file.png") });
  await autoEditor.getByRole("button", { name: "Clear file choice", exact: true }).click();
  assert.equal(
    await autoField("body").inputValue(),
    originalFileDraft,
    "Clearing a pending file lost the inline draft",
  );
  assert.equal(await autoField("body").isEnabled(), true);
  await page.evaluate(() => {
    globalThis.__workspaceFixture.bodyFile = "C:/fixtures/current-response.bin";
    globalThis.__workspaceFixture.bodyFileBytes = 25000001;
  });
  await autoField("mediaType").fill("text/plain; charset=utf-16le");
  await autoEditor
    .getByRole("button", { name: "Serve response body from file…", exact: true })
    .click();
  await autoEditor.getByLabel("File use", { exact: true }).selectOption("linked");
  await autoEditor.getByText(/Read directly for each request/).waitFor();
  await autoEditor.getByRole("button", { name: "Save rule", exact: true }).click();
  await savedProperties();
  assert.equal(await autoField("body").isDisabled(), true, "Saving a linked rule lost file mode");
  await autoEditor.getByText("C:/fixtures/current-response.bin", { exact: true }).waitFor();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.bodyFileError = true;
    document
      .querySelector("app-shell")
      .shadowRoot.querySelector("automation-workspace")
      .refreshUsageSoon();
  });
  await autoEditor.getByText(/Response file is missing or unreadable/).waitFor();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.bodyFileError = false;
    document
      .querySelector("app-shell")
      .shadowRoot.querySelector("automation-workspace")
      .refreshUsageSoon();
  });
  await autoEditor.getByText(/Response file is missing or unreadable/).waitFor({ state: "hidden" });
  await autoEditor.getByRole("button", { name: "Clear file choice", exact: true }).click();
  await page
    .locator("#auto-response-body-help")
    .getByText(/Text uses UTF-16LE/)
    .waitFor();
  await autoField("body").fill("Hi");
  await autoField("mediaType").fill("text/plain; charset=windows-1252");
  await page
    .locator("#auto-response-body-help")
    .getByText(/set a supported charset/)
    .waitFor();
  await autoEditor.getByRole("button", { name: "Save rule", exact: true }).click();
  await autoEditor.getByText(/This body has an unsupported text encoding/).waitFor();
  assert.equal(
    await autoField("body").inputValue(),
    "Hi",
    "Unsupported text encoding discarded the draft",
  );
  await autoField("mediaType").fill("text/plain; charset=utf-16le");
  await autoEditor.getByRole("button", { name: "Save rule", exact: true }).click();
  await savedProperties();
  assert.deepEqual(
    await page.evaluate(() => ({
      body: globalThis.__workspaceFixture.lastAssetEdit.decodedBody,
      type: globalThis.__workspaceFixture.lastAssetEdit.mediaType,
    })),
    { body: [72, 0, 105, 0], type: "text/plain; charset=utf-16le" },
    "Clearing a linked file encoded text differently from its declared charset",
  );
  assert.equal(await autoField("body").inputValue(), "Hi");
  await autoField("body").fill(originalFileDraft);
  await autoEditor.getByRole("button", { name: "Save rule", exact: true }).click();
  await savedProperties();
  assert.equal(await autoField("body").isEnabled(), true);
  const authoredRuleId = await page
    .locator(".auto-response-rule")
    .first()
    .getAttribute("data-rule-id");
  const authoredRule = page.locator("#rule-" + authoredRuleId);
  await authoredRule.click();
  await page
    .locator(".rule-selection-toolbar")
    .getByRole("button", { name: "More", exact: true })
    .click();
  await page.getByRole("button", { name: "Move later", exact: true }).click();
  await page.waitForFunction(
    (id) =>
      document.querySelector("app-shell").shadowRoot.querySelector(".auto-response-rule").dataset
        .ruleId !== id,
    authoredRuleId,
  );
  assert.equal(await authoredRule.locator(".rule-order").textContent(), "2");
  const orderBeforeRemove = await page
    .locator(".auto-response-rule")
    .evaluateAll((rules) => rules.map((rule) => rule.dataset.ruleId));
  await capturedRule.click();
  await capturedRule.press("Delete");
  await page.getByRole("button", { name: "Undo", exact: true }).waitFor({ state: "visible" });
  assert.equal(
    await page
      .getByRole("button", { name: "Undo", exact: true })
      .evaluate((button) => button.getRootNode().activeElement === button),
    true,
    "Removal left keyboard focus on a deleted control",
  );
  await page.getByRole("button", { name: "Undo", exact: true }).click();
  await capturedRule.waitFor({ state: "visible" });
  assert.deepEqual(
    await page
      .locator(".auto-response-rule")
      .evaluateAll((rules) => rules.map((rule) => rule.dataset.ruleId)),
    orderBeforeRemove,
    "Undo changed first-match order",
  );
  assert.equal(
    await capturedRule.locator(".rule-state").textContent(),
    "Disabled",
    "Undo changed the rule enabled state",
  );
  await capturedRule.click();
  assert.match(await capturedRule.locator(".rule-criteria").getAttribute("title"), /example.test/);
  assert.equal(await autoField("url").inputValue(), "http://example.test/second");

  await newScratch();
  await autoField("name").fill("Numeric account pattern");
  await autoEditor.getByLabel("URL matching", { exact: true }).selectOption("pattern");
  await autoField("url").fill("https://api.example.test/users/{:digits}");
  await autoEditor.locator(".matcher-test > summary").click();
  await autoEditor
    .getByLabel("Test URL", { exact: true })
    .fill("https://api.example.test/users/42");
  await autoEditor.getByRole("button", { name: "Check match", exact: true }).click();
  await autoEditor
    .getByText("This rule would serve the saved response.", { exact: true })
    .waitFor({ state: "visible" });
  assert.equal(
    await page.evaluate(
      () => globalThis.__workspaceFixture.lastMatcherTest.matcher.url.value.address,
    ),
    "https://api.example.test/users/{:digits}",
  );
  await autoEditor.locator("match-editor").evaluate((editor) => {
    const position = editor.addressInput.value.indexOf("{") + 1;
    editor.addressInput.setSelectionRange(position, position);
    editor.inspectSelection();
  });
  await autoEditor.getByRole("button", { name: "Edit placeholder", exact: true }).click();
  await autoEditor.getByLabel("Optional annotation", { exact: true }).fill("account");
  await autoEditor.getByRole("button", { name: "Apply placeholder", exact: true }).click();
  assert.equal(
    await autoField("url").inputValue(),
    "https://api.example.test/users/{account:digits}",
  );
  await autoEditor.getByRole("button", { name: "Add test case", exact: true }).click();
  await checkMatchingTests(page, root);
  await autoEditor.getByRole("button", { name: "Save rule", exact: true }).click();
  await savedProperties();
  const patternRuleId = await page
    .locator(".auto-response-rule")
    .first()
    .getAttribute("data-rule-id");
  await page.locator("#rule-" + patternRuleId).click();
  assert.equal(
    await autoEditor.getByLabel("URL matching", { exact: true }).inputValue(),
    "pattern",
  );
  assert.equal(
    await autoField("url").inputValue(),
    "https://api.example.test/users/{account:digits}",
  );
  assert.equal(
    await page.evaluate(
      () =>
        globalThis.__workspaceFixture.automation.rules.find(
          (rule) => rule.displayName === "Numeric account pattern",
        ).matcher.examples.length,
    ),
    1,
  );
  await checkMatchingSelectionRace(page, await capturedRule.getAttribute("data-rule-id"));
  await page.evaluate((id) => {
    const state = globalThis.__workspaceFixture;
    state.automation.usage = [{ ruleId: id, matches: 2, lastMatchedAt: 1000 }];
    document.querySelector("app-shell").onTrafficRefreshed();
  }, patternRuleId);
  await autoEditor.getByText(/2 retained matches/).waitFor({ state: "visible" });
  await page.evaluate(() => {
    globalThis.__workspaceFixture.automation.usage = [];
    document.querySelector("app-shell").onTrafficRefreshed();
  });
  await autoEditor
    .getByText("No matches in retained Traffic.", { exact: true })
    .waitFor({ state: "visible" });
  await cancelRule();

  // The dense list shares keyboard range selection, bulk state changes and Undo.
  await checkAutoResponseDragging(page, root);
  const firstRule = page.locator(".auto-response-rule").first();
  await firstRule.click();
  await savedProperties();
  await firstRule.press("Control+Shift+ArrowDown");
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace")
        .ruleSelectionCount === 2,
  );
  assert.equal(
    await page.locator(".auto-response-rule").nth(1).getAttribute("aria-selected"),
    "true",
  );
  assert.equal(
    await page.locator(".auto-response-rule").nth(1).locator('input[type="checkbox"]').isChecked(),
    true,
  );
  const enabledBeforeBulk = await page.evaluate(() =>
    globalThis.__workspaceFixture.automation.rules.map((rule) => ({
      id: rule.id,
      enabled: rule.enabled,
    })),
  );
  await page
    .locator(".rule-selection-toolbar")
    .getByRole("button", { name: "More", exact: true })
    .click();
  await page
    .locator("#rule-actions-menu")
    .getByRole("button", { name: "Enable selected", exact: true })
    .click();
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace")
        .changingAutoResponse,
  );
  await page.locator(".auto-response-rule").nth(1).press("Control+z");
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace")
        .changingAutoResponse,
  );
  assert.deepEqual(
    await page.evaluate(() =>
      globalThis.__workspaceFixture.automation.rules.map((rule) => ({
        id: rule.id,
        enabled: rule.enabled,
      })),
    ),
    enabledBeforeBulk,
  );
  await page.locator("#rule-" + patternRuleId).click();
  await savedProperties();
  await page
    .locator(".rule-selection-toolbar")
    .getByRole("button", { name: "More", exact: true })
    .click();
  await page.getByRole("button", { name: "Duplicate as disabled copies", exact: true }).click();
  const copyRule = page.locator(".auto-response-rule").filter({
    has: page.locator("strong").getByText("Copy of Numeric account pattern", { exact: true }),
  });
  await copyRule.waitFor({ state: "visible" });
  await copyRule
    .locator(".rule-warning")
    .getByText("Duplicate", { exact: true })
    .waitFor({ state: "visible" });
  await copyRule.click();
  await savedProperties();
  await autoField("url").fill("https://api.example.test/users/{:digits}");
  await autoEditor.getByRole("button", { name: "Save rule", exact: true }).click();
  await savedProperties();
  assert.equal(
    await copyRule.locator(".rule-warning").textContent(),
    "Duplicate",
    "Anonymous annotations changed duplicate matching identity",
  );
  await autoEditor.locator("rule-activation button").click();
  await copyRule
    .locator(".rule-state")
    .getByText("Enabled", { exact: true })
    .waitFor({ state: "visible" });
  assert.match(await page.locator(".rule-diagnostic").textContent(), /always superseded/);
  await autoEditor.locator("rule-activation button").click();
  await copyRule
    .locator(".rule-state")
    .getByText("Disabled", { exact: true })
    .waitFor({ state: "visible" });
  await copyRule.press("Delete");
  await copyRule.waitFor({ state: "detached" });
  assert.equal(
    await page.evaluate(
      () =>
        document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace")
          .ruleSelectionCount,
    ),
    0,
  );
  await page.getByRole("button", { name: "Undo", exact: true }).press("Control+z");
  await copyRule.waitFor({ state: "visible" });
  assert.equal(await copyRule.locator(".rule-state").textContent(), "Disabled");

  // Switching selection cannot silently discard property edits; Del in a field edits text.
  await authoredRule.click();
  await savedProperties();
  await autoField("name").fill("Unsaved name");
  await capturedRule.click();
  await page
    .locator("automation-workspace .unsaved-rule-dialog")
    .getByRole("button", { name: "Keep editing", exact: true })
    .click();
  assert.equal(await autoField("name").inputValue(), "Unsaved name");
  await capturedRule.click();
  await discardIfAsked();
  await savedProperties();
  await autoField("name").fill("Saved while switching");
  await authoredRule.click();
  await page
    .locator("automation-workspace .unsaved-rule-dialog")
    .getByRole("button", { name: "Save and continue", exact: true })
    .click();
  await savedProperties();
  assert.equal(
    await page.evaluate(
      () =>
        globalThis.__workspaceFixture.automation.rules.find(
          (rule) => rule.displayName === "Saved while switching",
        ) !== undefined,
    ),
    true,
  );
  await capturedRule.click();
  await savedProperties();
  const countBeforeTextDelete = await page.locator(".auto-response-rule").count();
  await autoField("name").fill("Keep");
  await autoField("name").press("Home");
  await autoField("name").press("Delete");
  assert.equal(await autoField("name").inputValue(), "eep");
  assert.equal(await page.locator(".auto-response-rule").count(), countBeforeTextDelete);
  await cancelRule();

  await page.getByLabel("Search rules", { exact: true }).fill("Numeric account");
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace")
        .visibleRules.length === 2,
  );
  assert.equal(
    await page.evaluate(
      () =>
        document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace")
          .ruleSelectionCount,
    ),
    0,
  );
  await page.getByLabel("Search rules", { exact: true }).fill("");
  const ruleDivider = page.getByRole("separator", {
    name: "Resize rule list and properties",
    exact: true,
  });
  const ruleSplitBefore = await page.evaluate(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace")
        .ruleListSplit,
  );
  await ruleDivider.press("ArrowRight");
  await page.waitForFunction(
    (before) => globalThis.__workspaceFixture.workspace.autoresponseSplit > before,
    ruleSplitBefore,
  );
  await ruleDivider.press("ArrowLeft");

  // Traffic supports multiselection, eligibility review, atomic failure and batch creation.
  await view("traffic");
  await checkTrafficList(page, resolve(root, "../../../target/ui-check"));
  await page.evaluate(async () => {
    const state = globalThis.__workspaceFixture;
    state.sessions.find((row) => row.id === "cached").terminal = "active";
    await document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace")
      .refreshSessions(undefined, true);
  });
  await page.locator('tr[data-session-id="first"]').click();
  await page.locator('tr[data-session-id="second"]').click({ modifiers: ["Control"] });
  await page.locator('tr[data-session-id="cached"]').click({ modifiers: ["Control"] });
  assert.equal(await page.locator('tr[aria-selected="true"][data-session-id]').count(), 3);
  await page
    .locator(".traffic-selection-bar")
    .getByRole("button", { name: /^Create \d+ saved responses?…$/ })
    .click();
  await page.waitForFunction(() => {
    const auto = document
      .querySelector("app-shell")
      .shadowRoot.querySelector("automation-workspace");
    return !auto.batchReviewHidden && !auto.batchLoading && auto.batchRows.length === 3;
  });
  assert.match(
    await page.locator(".batch-review").textContent(),
    /3 selected · 2 ready · 1 unavailable/,
  );
  assert.match(
    await page.locator(".batch-review").textContent(),
    /Wait for this request to complete/,
  );
  const includedResponse = page.locator('.batch-table tbody input[type="checkbox"]').nth(1);
  await includedResponse.uncheck();
  await page
    .getByRole("button", { name: "Create 1 rule", exact: true })
    .waitFor({ state: "visible" });
  await page.getByRole("button", { name: "Refresh review", exact: true }).click();
  await page.waitForFunction(() => {
    const auto = document
      .querySelector("app-shell")
      .shadowRoot.querySelector("automation-workspace");
    return !auto.batchLoading && auto.batchEligibleCount === 1;
  });
  assert.equal(
    await includedResponse.isChecked(),
    false,
    "Refreshing the batch lost excluded choices",
  );
  await includedResponse.check();
  const rulesBeforeBatch = await page.evaluate(() =>
    structuredClone(globalThis.__workspaceFixture.automation.rules),
  );
  await page.evaluate(() => (globalThis.__workspaceFixture.batchError = true));
  await page.getByRole("button", { name: "Create 2 rules", exact: true }).click();
  await page
    .locator(".batch-review")
    .getByText("Fixture batch storage failed", { exact: true })
    .waitFor({ state: "visible" });
  assert.deepEqual(
    await page.evaluate(() => globalThis.__workspaceFixture.automation.rules),
    rulesBeforeBatch,
  );
  await page.evaluate(() => (globalThis.__workspaceFixture.batchError = false));
  await page.getByRole("button", { name: "Create 2 rules", exact: true }).click();
  await page.locator(".batch-review").waitFor({ state: "hidden" });
  assert.equal(
    await page.evaluate(
      () =>
        document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace")
          .ruleSelectionCount,
    ),
    2,
  );
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.automation.rules.length),
    rulesBeforeBatch.length + 2,
  );
  await capturedRule
    .locator(".rule-warning")
    .getByText("Shadowed", { exact: true })
    .waitFor({ state: "visible" });

  await view("traffic");
  await page.locator('tr[data-session-id="first"]').click();
  await page.locator('tr[data-session-id="second"]').click({ modifiers: ["Control"] });
  await page.locator('tr[data-session-id="second"]').press("Delete");
  await page.locator('tr[data-session-id="first"]').waitFor({ state: "detached" });
  await page.locator('tr[data-session-id="second"]').waitFor({ state: "detached" });
  await page.getByRole("button", { name: "Undo traffic removal", exact: true }).press("Control+z");
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
        .removingTraffic,
  );
  await page.locator('tr[data-session-id="first"]').waitFor({ state: "visible" });
  await page.locator('tr[data-session-id="second"]').waitFor({ state: "visible" });
  assert.equal(await page.locator('tr[aria-selected="true"][data-session-id]').count(), 2);
  await view("automation");

  const rowBaselines = await page
    .locator(".auto-response-rule")
    .first()
    .evaluate((row) => {
      const bottom = (selector) => {
        const element = row.querySelector(selector);
        const range = document.createRange();
        range.selectNodeContents(element);
        return range.getBoundingClientRect().bottom;
      };
      return [bottom(".rule-description strong"), bottom("rule-activation button")];
    });
  assert.ok(
    Math.abs(rowBaselines[0] - rowBaselines[1]) <= 2,
    "Rule name and state text baselines do not align",
  );
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/auto-response-dense.png"),
  });
  await newScratch();
  await autoField("body").fill(
    Array.from({ length: 30 }, (_, index) => "line " + (index + 1)).join("\n"),
  );
  await page.setViewportSize({ width: 800, height: 600 });
  await autoEditor.evaluate((form) => form.scrollIntoView({ block: "start" }));
  const savePosition = await autoEditor
    .getByRole("button", { name: "Save rule", exact: true })
    .boundingBox();
  assert.ok(
    savePosition.y >= 50 && savePosition.y + savePosition.height < 592,
    "Sticky save actions are outside the small window",
  );
  assert.ok(
    (await autoField("body").boundingBox()).height >= 240,
    "Twelve-line body editor has insufficient height",
  );
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/auto-response-small.png"),
  });
  await page.emulateMedia({ colorScheme: "dark" });
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/auto-response-dark.png") });
  await page.emulateMedia({ colorScheme: "light" });
  await page.setViewportSize({ width: 1280, height: 800 });
  await cancelRule();
  await newScratch();
  await page.locator('.auto-response-editor input[name="name"]').fill("Preserved draft");
  await page.locator('.auto-response-editor select[name="method"]').selectOption("POST");
  assert.equal(
    await page.locator('.auto-response-editor textarea[name="requestHeaders"]').isVisible(),
    true,
  );
  await view("composer");
  await page
    .locator('input[name="url"]')
    .filter({ visible: true })
    .fill("http://example.test/replay");
  const composerFocus = await page
    .locator('input[name="url"]')
    .filter({ visible: true })
    .evaluate((input) => ({
      control: input.closest(".composer-request").querySelector("input").getBoundingClientRect().x,
      clip: input.closest(".composer-request").getBoundingClientRect().x,
    }));
  assert.ok(
    composerFocus.control - composerFocus.clip >= 4,
    "Composer focus outline is clipped by its scrolling pane",
  );
  await view("automation");
  assert.equal(
    await page.locator('.auto-response-editor input[name="name"]').inputValue(),
    "Preserved draft",
  );
  await page
    .locator(".workspace-tabs")
    .getByRole("button", { name: "Scripts", exact: true })
    .click();
  await checkScripts(page, root);
  const editorsBefore = await page.evaluate(
    () =>
      document
        .querySelector("app-shell")
        .shadowRoot.querySelector("script-editor")
        .monaco.editor.getModels().length,
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.paused = Array.from({ length: 20 }, (_, index) => ({
      decisionId: index + 1,
      exchangeId: "paused-" + index,
      phase: index % 2 ? "response-head" : "request-head",
      requestHead: { method: "GET", target: "https://example.test/paused/" + index },
      responseHead: index % 2 ? { status: 200, headers: [] } : null,
      bodyHex: null,
      hookId: "fixture",
      expiresAtUnixMs: Date.now() + 60000,
    }));
  });
  await view("breakpoints");
  await page.getByRole("button", { name: "Phases", exact: true }).click();
  const phaseButton = await page.getByRole("button", { name: "Phases", exact: true }).boundingBox();
  const phaseMenu = await page.locator("#breakpoint-phases").boundingBox();
  assert.ok(
    phaseMenu.x < phaseButton.x + phaseButton.width &&
      phaseMenu.x + phaseMenu.width > phaseButton.x &&
      Math.abs(phaseMenu.y - phaseButton.y - phaseButton.height) < 12,
    "Phases menu is detached from its button",
  );
  await page.keyboard.press("Escape");
  await page
    .locator("paused-exchange")
    .first()
    .getByRole("button", { name: "Continue", exact: true })
    .waitFor({ state: "visible" });
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("paused-exchange")?.editorReady,
  );
  await checkMonacoIcons(page.locator("paused-exchange .monaco-editor-host"));
  await page.setViewportSize({ width: 760, height: 520 });
  await page.locator('tr[data-decision-id="1"]').click();
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/breakpoints-small.png") });
  const pausedBounds = await page.locator("paused-exchange").first().boundingBox();
  const expandBounds = await page
    .locator("paused-exchange")
    .first()
    .getByRole("button", { name: "Expand editor", exact: true })
    .boundingBox();
  assert.ok(
    pausedBounds.x + pausedBounds.width <= 760,
    "Paused editor exceeds the narrow viewport",
  );
  assert.ok(expandBounds.x + expandBounds.width <= 760, "Expand editor control is clipped");
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/breakpoints-wide.png") });
  assert.equal(
    await page.locator("paused-exchange").count(),
    1,
    "The queue allocated an editor for every request",
  );
  await page.evaluate(() =>
    document
      .querySelector("app-shell")
      .shadowRoot.querySelector("paused-exchange")
      .editor.setValue('{\"method\":\"PATCH\"}'),
  );
  await page.locator('tr[data-decision-id="2"]').click();
  await page.waitForFunction(() =>
    document
      .querySelector("app-shell")
      .shadowRoot.querySelector("paused-exchange")
      .draftText.includes("status"),
  );
  assert.equal(await page.locator('a[data-view="breakpoints"] .nav-count').textContent(), "20");
  await page.locator('tr[data-decision-id="1"]').click();
  assert.match(
    await page.evaluate(() =>
      document
        .querySelector("app-shell")
        .shadowRoot.querySelector("paused-exchange")
        .editor.getValue(),
    ),
    /PATCH/,
    "Queue navigation lost a replacement draft",
  );
  const editorsVisible = await page.evaluate(
    () =>
      document
        .querySelector("app-shell")
        .shadowRoot.querySelector("script-editor")
        .monaco.editor.getModels().length,
  );
  assert.ok(
    editorsVisible > editorsBefore && editorsVisible <= editorsBefore + 2,
    "Offscreen breakpoint editors were eagerly created",
  );
  await page.evaluate(async () => {
    globalThis.__workspaceFixture.paused = [];
    await document
      .querySelector("app-shell")
      .shadowRoot.querySelector("breakpoint-workspace")
      .refreshBreakpoints();
  });
  await page.waitForFunction(
    () =>
      document
        .querySelector("app-shell")
        .shadowRoot.querySelector("script-editor")
        .monaco.editor.getModels().length === 1,
  );
  await page.evaluate(async () => {
    globalThis.__workspaceFixture.lifecycle = "running";
    await document.querySelector("app-shell").refreshStatus();
  });
  await view("captures");
  const captures = page.locator("capture-workspace");
  await checkCaptureTools(page, root, view);
  const focusBounds = await captures
    .getByLabel("Traffic to record", { exact: true })
    .evaluate((input) => ({
      control: input.getBoundingClientRect().x,
      clip: input.closest(".capture-content").getBoundingClientRect().x,
    }));
  assert.ok(
    focusBounds.control - focusBounds.clip >= 4,
    "Capture focus outline is clipped by its scrolling pane",
  );
  await captures.getByRole("button", { name: "Switch to recording…", exact: true }).waitFor();
  await page.evaluate(async () => {
    globalThis.__workspaceFixture.lifecycle = "stopped";
    await document.querySelector("app-shell").refreshStatus();
  });
  await page.evaluate(async () => {
    const state = globalThis.__workspaceFixture;
    state.recordingDefaultBodies = true;
    await document
      .querySelector("app-shell")
      .shadowRoot.querySelector("capture-workspace")
      .loadRecordingDefaults();
  });
  assert.equal(
    await captures.getByLabel("Traffic to record", { exact: true }).inputValue(),
    "bodies",
  );
  await captures.getByLabel("Traffic to record", { exact: true }).selectOption("headers");
  await page.evaluate(async () => {
    await document
      .querySelector("app-shell")
      .shadowRoot.querySelector("capture-workspace")
      .loadRecordingDefaults();
    delete globalThis.__workspaceFixture.recordingDefaultBodies;
  });
  assert.equal(
    await captures.getByLabel("Traffic to record", { exact: true }).inputValue(),
    "headers",
    "Loading defaults overwrote an explicit recording choice",
  );
  const captureIdle = () =>
    page.waitForFunction(
      () =>
        !document.querySelector("app-shell").shadowRoot.querySelector("capture-workspace")
          .captureBusy,
    );
  await page.evaluate(
    () => (globalThis.__workspaceFixture.capturePick = "C:/captures/placeholder.tmcap"),
  );
  await captures.getByRole("button", { name: "Start recording…", exact: true }).click();
  await captureIdle();
  await captures
    .locator(".capture-state")
    .getByText("Recording · 2 KiB written", { exact: true })
    .waitFor({ state: "visible" });
  assert.equal(
    await page.evaluate(() =>
      Object.hasOwn(globalThis.__workspaceFixture.lastCaptureRequest, "maxFileBytes"),
    ),
    false,
  );
  assert.equal(
    await captures.getByRole("button", { name: "Start recording…", exact: true }).isDisabled(),
    true,
  );
  await captures.getByRole("button", { name: "Stop recording", exact: true }).click();
  await captureIdle();
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("capture-workspace")
        .refreshPending,
  );
  await page.evaluate(() => {
    const capture = document
      .querySelector("app-shell")
      .shadowRoot.querySelector("capture-workspace");
    clearTimeout(capture.timer);
    globalThis.__workspaceFixture.deferCaptureStatus = true;
    void capture.refreshCapture();
  });
  await page.waitForFunction(() => globalThis.__workspaceFixture.captureStatusPending);
  await captures.getByRole("button", { name: "Start recording…", exact: true }).click();
  await captureIdle();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.captureStatusError = true;
    globalThis.__workspaceFixture.releaseCaptureStatus();
  });
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("capture-workspace")
        .refreshPending,
  );
  assert.equal(
    await captures.locator(".capture-error").isVisible(),
    false,
    "A stale status failure replaced a successful recording action",
  );
  assert.equal(
    await captures.getByRole("button", { name: "Stop recording", exact: true }).isEnabled(),
    true,
  );
  await page.evaluate(() => {
    delete globalThis.__workspaceFixture.captureStatusError;
    delete globalThis.__workspaceFixture.captureStatusPending;
  });
  await captures.getByRole("button", { name: "Stop recording", exact: true }).click();
  await captureIdle();
  // A canceled destination never stops live capture; draining can be canceled.
  const beforeRecordingSwitch = await page.evaluate(async () => {
    const s = globalThis.__workspaceFixture,
      c = document.querySelector("app-shell").shadowRoot.querySelector("capture-workspace");
    s.lifecycle = "running";
    s.capturePick = null;
    c.$emit("status-changed", await window.__TAURI_INTERNALS__.invoke("app_status"));
    return { stop: s.calls.stop_application ?? 0, start: s.calls.start_capture ?? 0 };
  });
  await captures.getByRole("button", { name: "Switch to recording…", exact: true }).click();
  await captureIdle();
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.calls.stop_application ?? 0),
    beforeRecordingSwitch.stop,
    "Canceling the destination stopped live capture",
  );
  await page.evaluate(() => {
    const s = globalThis.__workspaceFixture;
    s.capturePick = "C:/captures/switch.tmcap";
    s.testDrain = true;
  });
  await captures.getByRole("button", { name: "Switch to recording…", exact: true }).click();
  await captures.getByRole("button", { name: "Cancel recording", exact: true }).waitFor();
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.calls.start_capture ?? 0),
    beforeRecordingSwitch.start,
    "Recording started before active requests drained",
  );
  await captures.getByRole("button", { name: "Cancel recording", exact: true }).click();
  await captureIdle();
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.calls.start_capture ?? 0),
    beforeRecordingSwitch.start,
  );
  // Canceling the recording must not cancel observation of the proxy drain.
  await page.evaluate(() => {
    globalThis.__workspaceFixture.lifecycle = "stopped";
    globalThis.__workspaceFixture.testDrain = false;
  });
  await page.waitForFunction(() => {
    const shell = document.querySelector("app-shell");
    return shell.lifecycleKind === "stopped" && shell.settings.proxyLifecycle === "stopped";
  });
  await captures.getByRole("button", { name: "Start recording…", exact: true }).waitFor();

  await page.evaluate(async () => {
    const s = globalThis.__workspaceFixture,
      c = document.querySelector("app-shell").shadowRoot.querySelector("capture-workspace");
    s.testDrain = false;
    s.lifecycle = "running";
    c.$emit("status-changed", await window.__TAURI_INTERNALS__.invoke("app_status"));
  });
  await captures.getByLabel("Traffic to record", { exact: true }).selectOption("bodies");
  await captures.getByRole("button", { name: "Switch to recording…", exact: true }).click();
  await captureIdle();
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.lastCaptureRequest.retainBodySamples),
    true,
  );
  await captures.getByRole("button", { name: "Stop recording", exact: true }).click();
  await captureIdle();
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/record-unlimited-wide.png"),
  });
  await captures
    .locator(".capture-tabs")
    .getByRole("button", { name: "Inspect / recover", exact: true })
    .click();
  await captures.getByLabel("Trace file", { exact: true }).fill("session.tmcap");
  await captures.getByRole("button", { name: "Inspect trace metadata", exact: true }).click();
  await captureIdle();
  const inspectedSource = await captures.getByLabel("Trace file", { exact: true }).inputValue();
  await page.evaluate(() => (globalThis.__workspaceFixture.captureInspectionError = true));
  await captures.getByRole("button", { name: "Inspect trace metadata", exact: true }).click();
  await page
    .getByRole("alertdialog", { name: "Trace metadata inspection failed", exact: true })
    .getByRole("button", { name: "Close", exact: true })
    .click();
  await captureIdle();
  assert.equal(
    await captures.getByLabel("Trace file", { exact: true }).inputValue(),
    inspectedSource,
  );
  await captures.getByRole("button", { name: "Dismiss capture error", exact: true }).click();
  assert.equal(await captures.locator(".capture-error").isVisible(), false);
  await page.evaluate(() => delete globalThis.__workspaceFixture.captureInspectionError);
  await captures.getByRole("button", { name: "Inspect trace metadata", exact: true }).click();
  await captureIdle();
  assert.match(
    await captures.locator(".capture-receipt").textContent(),
    /Interrupted tail; valid prefix recovered/,
  );
  await captures.getByLabel("Trace file", { exact: true }).fill("uninspected.tmcap");
  await captures.getByRole("button", { name: "Convert inspected trace…", exact: true }).click();
  assert.equal(
    await captures.getByLabel("Source file (.tmcap)", { exact: true }).inputValue(),
    "C:/captures/" + inspectedSource,
    "Recovery used an edited filename instead of the inspected capture",
  );
  assert.equal(
    await captures.getByLabel("Destination file", { exact: true }).inputValue(),
    "C:/captures/session.converted.tmcap",
  );
  await captures.getByLabel("Format", { exact: true }).selectOption("json-lines");
  assert.equal(
    await captures.getByLabel("Destination file", { exact: true }).inputValue(),
    "C:/captures/session.converted.jsonl",
  );
  await captures.getByLabel("Redact sensitive header values in this copy", { exact: true }).check();
  await captures.getByRole("button", { name: "Convert file", exact: true }).click();
  await captureIdle();
  assert.equal(
    await page.evaluate(
      () => globalThis.__workspaceFixture.lastCaptureExport.redactSensitiveHeaders,
    ),
    true,
  );
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.lastCaptureExport.format),
    "json-lines",
  );
  assert.match(await captures.locator(".capture-receipt").textContent(), /Valid prefix preserved/);
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/capture-wide.png") });
  await checkTraceImport(page, root, view);
  await view("captures");
  await page.setViewportSize({ width: 760, height: 520 });
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/capture-small.png") });
  await page.setViewportSize({ width: 1280, height: 800 });
  await view("traffic");
  await page.emulateMedia({ colorScheme: "dark", reducedMotion: "reduce" });
  assert.equal(
    await page.evaluate(() => getComputedStyle(document.querySelector("app-shell")).colorScheme),
    "dark",
  );
  const layout = await page.evaluate(() => ({
    height: document.documentElement.clientHeight,
    scrollHeight: document.documentElement.scrollHeight,
  }));
  assert.ok(layout.scrollHeight <= layout.height + 1, "The document viewport can scroll");
  assert.deepEqual(await page.evaluate(() => globalThis.__cspViolations), []);
  assert.deepEqual(errors, []);
  await page.waitForFunction(
    () =>
      globalThis.__workspaceFixture.workspace.sidebarCollapsed &&
      globalThis.__workspaceFixture.workspace.listSplit === 47 &&
      globalThis.__workspaceFixture.workspace.columns.find((column) => column.id === "host").pinned,
  );
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/workspace.png") });
  const beforeReload = requests.length;
  await page.reload();
  await page.waitForFunction(() =>
    document
      .querySelector("app-shell")
      .shadowRoot.querySelector(".session-status")
      .textContent.includes("traffic entries."),
  );
  assert.equal(
    await page
      .getByRole("button", { name: "Toggle navigation labels" })
      .getAttribute("aria-expanded"),
    "false",
  );
  assert.equal(
    await page
      .getByRole("separator", { name: "Resize traffic list and inspector", exact: true })
      .getAttribute("aria-valuenow"),
    "47",
  );
  assert.equal(await page.locator("#header-host").getAttribute("data-pinned"), "");
  assert.equal(
    requests
      .slice(beforeReload)
      .some(
        (path) => path === "/monaco.js" || path === "/monaco.css" || editorOutputs.includes(path),
      ),
    false,
    "Reload eagerly fetched Monaco",
  );
  assert.deepEqual(errors, []);
  await page.setViewportSize({ width: 760, height: 520 });
  await page.locator('tr[data-session-id="second"]').click();
  await page
    .locator('traffic-workspace message-inspector[side="response"]')
    .getByText("Auto · JSON", { exact: true })
    .waitFor({ state: "visible" });
  const smallPreview = await page
    .locator('traffic-workspace message-inspector[side="response"] .body-scroll')
    .boundingBox();
  assert.ok(smallPreview.height >= 48, "Small-window response preview has no usable space");
  await waitForBodySource(page.locator('traffic-workspace message-inspector[side="response"]'));
  await page
    .locator('traffic-workspace message-inspector[side="response"] .source-code-host')
    .scrollIntoViewIfNeeded();
  assert.equal(
    await page.evaluate(
      () => document.documentElement.scrollHeight <= document.documentElement.clientHeight + 1,
    ),
    true,
    "Small-window inspection scroll escaped the pane",
  );
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/workspace-small.png") });
  await openColumn("status");
  const smallMenu = await page.locator("#column-actions").boundingBox();
  assert.ok(smallMenu.y + smallMenu.height <= 520, "Column menu extends below a small window");
  await page.keyboard.press("Escape");
  await page.setViewportSize({ width: 1280, height: 800 });
  await view("settings");
  const setupCertificate = page
    .locator("#settings")
    .getByRole("button", { name: "Check certificate setup", exact: true });
  const resetCertificate = page
    .locator("#settings")
    .getByRole("button", { name: "Reset certificate and set up again", exact: true });
  const certificateNotice = page.locator(".notice");
  const caIdle = () =>
    page.waitForFunction(
      () =>
        !document.querySelector("app-shell").shadowRoot.querySelector("settings-workspace")
          .proxyPending,
    );
  await page
    .locator(".settings-tabs")
    .getByRole("button", { name: "Preferences", exact: true })
    .click();
  const preferences = page.locator("settings-workspace");
  const advancedNetwork = preferences.locator(".preferences-form .connection-advanced");
  assert.equal(
    await advancedNetwork.getAttribute("open"),
    null,
    "Advanced settings were expanded by default",
  );
  assert.equal(
    await preferences.getByLabel("Decrypt HTTPS traffic", { exact: true }).isVisible(),
    true,
    "HTTPS decryption is hidden in advanced settings",
  );
  await advancedNetwork.locator("summary").click();
  assert.equal(
    await preferences.getByLabel("Allow remote computers to connect", { exact: true }).isChecked(),
    false,
  );

  assert.equal(await preferences.getByLabel("Proxy port", { exact: true }).isVisible(), false);
  await preferences.getByLabel("Allow remote computers to connect", { exact: true }).check();
  await preferences.getByLabel("Use a specific proxy port", { exact: true }).check();
  await preferences.getByLabel("Proxy port", { exact: true }).fill("8123");
  assert.equal(
    await preferences
      .getByLabel("Isolate outbound connections by caller process", { exact: true })
      .isChecked(),
    true,
  );
  assert.equal(
    await preferences.getByLabel("Decrypt HTTPS traffic", { exact: true }).isChecked(),
    true,
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.configureSystemProxy = false;
  });
  await preferences
    .getByLabel("Isolate outbound connections by caller process", { exact: true })
    .uncheck();
  await preferences.getByRole("button", { name: "Save settings", exact: true }).click();
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("settings-workspace")
        .settingsBusy,
  );
  const networkSaved = await page.evaluate(
    () => globalThis.__workspaceFixture.savedProduct.preferences,
  );
  assert.equal(networkSaved.allowRemoteClients, true);
  assert.equal(networkSaved.proxyPort, 8123);
  assert.equal(networkSaved.isolateOutboundConnections, false);
  await checkNetworkProfiles(page, root);
  assert.equal(networkSaved.configureSystemProxy, false, "Saving preferences enabled host routing");
  await preferences
    .getByLabel("Isolate outbound connections by caller process", { exact: true })
    .check();
  await preferences.getByRole("button", { name: "Revert changes", exact: true }).click();
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("settings-workspace")
        .settingsBusy,
  );
  assert.equal(
    await preferences
      .getByLabel("Isolate outbound connections by caller process", { exact: true })
      .isChecked(),
    false,
  );
  await preferences.getByLabel("Decrypt HTTPS traffic", { exact: true }).uncheck();
  await preferences.getByRole("button", { name: "Save settings", exact: true }).click();
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("settings-workspace")
        .settingsBusy,
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.caBootstrap = {
      caFilesPresent: false,
      caFilesExist: false,
      ownedCaSha256: null,
      ownedCaTrusted: false,
    };
    globalThis.__workspaceFixture.caEvents = [];
  });
  await page
    .locator(".settings-tabs")
    .getByRole("button", { name: "Connection", exact: true })
    .click();
  assert.equal(
    await preferences
      .getByRole("button", { name: "Set up HTTPS interception", exact: true })
      .isVisible(),
    false,
  );
  await preferences
    .locator("#settings")
    .getByRole("button", { name: "Start proxy", exact: true })
    .click();
  await caIdle();
  assert.deepEqual(
    await page.evaluate(() => globalThis.__workspaceFixture.caEvents),
    [],
    "Opaque mode attempted certificate setup",
  );
  assert.equal(await page.evaluate(() => globalThis.__workspaceFixture.lifecycle), "running");
  await preferences
    .locator("#settings")
    .getByRole("button", { name: "Stop proxy", exact: true })
    .click();
  await caIdle();
  await page
    .locator(".settings-tabs")
    .getByRole("button", { name: "Preferences", exact: true })
    .click();
  await preferences.getByLabel("Decrypt HTTPS traffic", { exact: true }).check();
  await preferences
    .getByLabel("Isolate outbound connections by caller process", { exact: true })
    .check();
  await preferences.getByRole("button", { name: "Save settings", exact: true }).click();
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("settings-workspace")
        .settingsBusy,
  );
  await advancedNetwork.scrollIntoViewIfNeeded();
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/network-settings-wide.png"),
  });
  await page.setViewportSize({ width: 760, height: 520 });
  await advancedNetwork.scrollIntoViewIfNeeded();
  assert.equal(
    await preferences
      .getByLabel("Isolate outbound connections by caller process", { exact: true })
      .isVisible(),
    true,
  );
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/network-settings-small.png"),
  });
  await page.setViewportSize({ width: 1280, height: 800 });
  await advancedNetwork.locator("summary").click();
  assert.equal(
    await preferences.getByLabel("Limit number of live entries", { exact: true }).isChecked(),
    false,
  );
  await preferences.getByLabel("Limit number of live entries", { exact: true }).check();
  await preferences.getByLabel("Maximum live entries", { exact: true }).fill("25000");
  await preferences.getByLabel("Circular buffer limit", { exact: true }).selectOption("custom");
  await preferences.getByLabel("Maximum size (GiB)", { exact: true }).fill("2");
  assert.equal(
    await preferences.getByLabel("Maximum size (GiB)", { exact: true }).isVisible(),
    true,
  );
  await preferences.getByLabel("Circular buffer limit", { exact: true }).selectOption("unlimited");
  assert.equal(
    await preferences.getByLabel("Maximum size (GiB)", { exact: true }).isVisible(),
    false,
  );
  await preferences.getByRole("button", { name: "Save settings", exact: true }).click();
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("settings-workspace")
        .settingsBusy,
  );
  assert.equal(
    (await page.evaluate(() => globalThis.__workspaceFixture.savedProduct)).privacy.bufferLimit
      .mode,
    "unlimited",
  );
  assert.equal(
    (await page.evaluate(() => globalThis.__workspaceFixture.savedProduct)).privacy.maxLiveEntries,
    25000,
  );
  await preferences.getByLabel("Circular buffer limit", { exact: true }).selectOption("automatic");
  await preferences.getByRole("button", { name: "Save settings", exact: true }).click();
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("settings-workspace")
        .settingsBusy,
  );
  await preferences.getByLabel("Theme", { exact: true }).selectOption("light");
  assert.equal(
    await preferences.getByRole("button", { name: "Save settings", exact: true }).isEnabled(),
    true,
  );
  await preferences.getByRole("button", { name: "Revert changes", exact: true }).click();
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("settings-workspace")
        .settingsBusy,
  );
  assert.equal(await preferences.getByLabel("Theme", { exact: true }).inputValue(), "system");
  await preferences
    .getByLabel("Redact Authorization, Proxy-Authorization, Cookie and Set-Cookie values", {
      exact: true,
    })
    .check();
  assert.equal(
    await preferences.getByLabel("Request body limit", { exact: true }).inputValue(),
    "25000000",
  );
  await preferences.getByLabel("Request body limit", { exact: true }).selectOption("custom");
  const customBody = preferences.getByLabel("Limit (MB)", { exact: true });
  await customBody.fill("0.5");
  assert.equal(await customBody.evaluate((input) => input.checkValidity()), false);
  await customBody.fill("1000001");
  assert.equal(await customBody.evaluate((input) => input.checkValidity()), false);
  for (const [mb, bytes] of [
    ["1", 1000000],
    ["1.5", 1500000],
    ["1000000", 1000000000000],
    ["25", 25000000],
  ]) {
    await customBody.fill(mb);
    await preferences.getByRole("button", { name: "Save settings", exact: true }).click();
    await page.waitForFunction(
      () =>
        !document.querySelector("app-shell").shadowRoot.querySelector("settings-workspace")
          .settingsBusy,
    );
    assert.equal(
      await page.evaluate(
        () => globalThis.__workspaceFixture.savedProduct.privacy.requestBodyLimit,
      ),
      bytes,
    );
    await preferences.evaluate((element) => element.loadSettings());
    await page.waitForFunction(
      () =>
        !document.querySelector("app-shell").shadowRoot.querySelector("settings-workspace")
          .settingsBusy,
    );
    assert.equal(
      await preferences.getByLabel("Request body limit", { exact: true }).inputValue(),
      "custom",
    );
    assert.equal(await customBody.inputValue(), mb);
  }
  await customBody.scrollIntoViewIfNeeded();
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/custom-body-limit.png") });

  await preferences.getByLabel("Request body limit", { exact: true }).selectOption("unlimited");
  assert.equal(await preferences.getByLabel("Entries per page", { exact: true }).count(), 0);
  await preferences.getByRole("button", { name: "Save settings", exact: true }).click();
  await preferences.getByText("Settings saved.", { exact: true }).waitFor({ state: "visible" });
  assert.equal(
    await page.evaluate(
      () => globalThis.__workspaceFixture.savedProduct.privacy.redactSensitiveHeaders,
    ),
    true,
  );
  assert.equal(
    await page.evaluate(
      () => globalThis.__workspaceFixture.savedProduct.privacy.retainRequestBodies,
    ),
    true,
  );
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.savedProduct.privacy.requestBodyLimit),
    null,
  );
  await preferences.getByLabel("Request body limit", { exact: true }).selectOption("25000000");
  await preferences.getByRole("button", { name: "Revert changes", exact: true }).click();
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("settings-workspace")
        .settingsBusy,
  );
  assert.equal(
    await preferences.getByLabel("Request body limit", { exact: true }).inputValue(),
    "unlimited",
  );
  assert.equal(
    await page.evaluate(
      () => globalThis.__workspaceFixture.savedProduct.preferences.sessionPageSize,
    ),
    100,
    "Saving settings discarded the legacy preference",
  );

  assert.equal(
    await preferences.getByRole("button", { name: "Save settings", exact: true }).isDisabled(),
    true,
  );
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/settings-wide.png") });
  await page.setViewportSize({ width: 760, height: 520 });
  const preferenceSave = await preferences
    .getByRole("button", { name: "Save settings", exact: true })
    .boundingBox();
  assert.ok(
    preferenceSave.y + preferenceSave.height <= 512,
    "Preference save actions leave the narrow window",
  );
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/settings-small.png") });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page
    .locator(".settings-tabs")
    .getByRole("button", { name: "Support", exact: true })
    .click();
  assert.equal(
    await preferences
      .getByLabel("Include remembered capture and export file locations", { exact: true })
      .isDisabled(),
    true,
  );
  await preferences.getByRole("button", { name: "Refresh diagnostics", exact: true }).click();
  await preferences
    .getByText("Diagnostics refreshed.", { exact: true })
    .waitFor({ state: "visible" });
  assert.match(await preferences.locator(".support-facts").textContent(), /Windows fixture/);
  await page.evaluate(
    () => (globalThis.__workspaceFixture.supportPick = "C:/captures/placeholder-support.zip"),
  );
  await preferences.getByRole("button", { name: "Create support bundle…", exact: true }).click();
  await preferences
    .getByText("Support bundle saved.", { exact: true })
    .waitFor({ state: "visible" });
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.supportInput.includeRecentPaths),
    false,
  );
  await page.locator(".settings-tabs").getByRole("button", { name: "About", exact: true }).click();
  assert.match(
    await preferences.locator("#update-panel").textContent(),
    /Installed version: 0.1.0/,
  );
  const website = preferences.getByRole("link", { name: "transmog.net", exact: true });
  assert.equal(await website.getAttribute("href"), "https://transmog.net");
  assert.ok(
    (await website.evaluate(
      (link) =>
        link.getBoundingClientRect().x -
        link.closest(".settings-content").getBoundingClientRect().x,
    )) >= 4,
    "About link focus outline is clipped",
  );
  await website.click();
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.calls.open_product_website),
    1,
  );
  await preferences.getByRole("button", { name: "Credits", exact: true }).click();
  const credits = page.locator(".credits-dialog[open]");
  await credits.locator("details").nth(1).waitFor();
  assert.ok(
    (await credits.locator("details").count()) > 100,
    "Credits omitted the dependency inventory",
  );
  for (const name of ["h2", "hyper", "quiche"]) {
    const dependency = credits
      .locator("details")
      .filter({ has: page.locator("summary", { hasText: new RegExp("^" + name + " ") }) });
    assert.equal(await dependency.count(), 1, "Credits omitted " + name);
    await dependency.locator("summary").click();
    assert.match(
      await dependency.locator("pre").textContent(),
      /copyright|permission|redistribution/i,
    );
    await dependency.locator("summary").click();
  }
  assert.doesNotMatch(
    await credits.textContent(),
    /Generated from the locked|Distributions must retain/,
  );
  await credits.locator("details").first().locator("summary").click();
  assert.match(
    await credits.locator("details").first().textContent(),
    /license|copyright|permission/i,
  );
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/credits-wide.png") });
  await page.keyboard.press("Escape");
  assert.equal(await credits.count(), 0);
  await page
    .locator(".settings-tabs")
    .getByRole("button", { name: "Connection", exact: true })
    .click();
  await preferences.locator(".connection-panel .connection-advanced summary").click();
  // Legacy, partial, and missing material all offer recovery without attempting
  // to overwrite files or trust a certificate with an unknown identity.
  for (const bootstrap of [
    { caFilesPresent: true, caFilesExist: true, ownedCaSha256: null, ownedCaTrusted: false },
    { caFilesPresent: false, caFilesExist: true, ownedCaSha256: null, ownedCaTrusted: false },
    {
      caFilesPresent: false,
      caFilesExist: false,
      ownedCaSha256: "0".repeat(64),
      ownedCaTrusted: true,
    },
    {
      caFilesPresent: true,
      caFilesExist: true,
      ownedCaSha256: "0".repeat(64),
      ownedCaTrusted: true,
      caKeyError:
        "Windows DPAPI cannot decrypt the saved interception CA private key. The key is unlikely to be recoverable. Remove the old trusted root and set up a new interception certificate.",
    },
  ]) {
    await page.evaluate((bootstrap) => {
      const state = globalThis.__workspaceFixture;
      state.caBootstrap = bootstrap;
      state.caEvents = [];
    }, bootstrap);
    await setupCertificate.click();
    await caIdle();
    assert.match(await certificateNotice.textContent(), /Interception certificate needs a reset/);
    if (bootstrap.caKeyError) {
      assert.match(await certificateNotice.textContent(), /DPAPI.*unlikely to be recoverable/);
      assert.equal(
        await preferences.evaluate((element) => element.certificateReady),
        false,
        "An inaccessible signing key was reported as ready",
      );
      await page.getByRole("button", { name: "Start proxy", exact: true }).first().click();
      await caIdle();
      assert.deepEqual(
        await page.evaluate(() => globalThis.__workspaceFixture.caEvents),
        [],
        "Starting with an inaccessible key altered trust or material",
      );
      await view("settings");
      await page.screenshot({
        path: resolve(root, "../../../target/ui-check/certificate-recovery-wide.png"),
      });
      await page.setViewportSize({ width: 760, height: 520 });
      await page.screenshot({
        path: resolve(root, "../../../target/ui-check/certificate-recovery-small.png"),
      });
      await page.setViewportSize({ width: 1280, height: 800 });
    }
    assert.deepEqual(await page.evaluate(() => globalThis.__workspaceFixture.caEvents), []);
    page.once("dialog", (dialog) => dialog.dismiss());
    await certificateNotice
      .getByRole("button", { name: "Reset certificate and set up again", exact: true })
      .click();
    await caIdle();
    assert.deepEqual(
      await page.evaluate(() => globalThis.__workspaceFixture.caEvents),
      [],
      "Canceling reset changed certificate state",
    );
    // Reset always targets the backend-owned paths even when the form contains
    // a user-selected CA, and uses the default paths for the fresh certificate.
    await page.getByLabel("CA certificate", { exact: true }).fill("custom.pem");
    await page.getByLabel("CA private key", { exact: true }).fill("custom.key");
    await page.evaluate(() => {
      globalThis.__workspaceFixture.deferCaReset = true;
    });
    page.once("dialog", async (dialog) => {
      assert.match(dialog.message(), /fixture.pem\nfixture.key/);
      await dialog.accept();
    });
    await resetCertificate.click();
    await page.waitForFunction(() => globalThis.__workspaceFixture.caResetPending);
    assert.equal(await setupCertificate.isDisabled(), true);
    assert.equal(await resetCertificate.isDisabled(), true);
    await page.evaluate(async () => {
      const settings = document
        .querySelector("app-shell")
        .shadowRoot.querySelector("settings-workspace");
      await settings.resetCa();
      globalThis.__workspaceFixture.releaseCaReset();
      delete globalThis.__workspaceFixture.caResetPending;
    });
    await caIdle();
    assert.match(await certificateNotice.textContent(), /HTTPS interception is ready/);
    assert.deepEqual(await page.evaluate(() => globalThis.__workspaceFixture.caEvents), [
      "reset",
      "create",
      "install",
    ]);
    assert.deepEqual(
      await page.evaluate(() => globalThis.__workspaceFixture.resetCaArgs),
      {},
      "Reset accepted arbitrary file paths",
    );
    assert.deepEqual(
      await page.evaluate(() => [
        globalThis.__workspaceFixture.lastCaCreate.certificatePath,
        globalThis.__workspaceFixture.lastCaCreate.privateKeyPath,
        globalThis.__workspaceFixture.lastCaInstall.path,
      ]),
      ["fixture.pem", "fixture.key", "fixture.pem"],
    );
    assert.equal(await page.getByLabel("CA SHA-256", { exact: true }).inputValue(), "1".repeat(64));
  }
  // A cleanup failure must not advance to certificate creation or trust.
  await page.evaluate(() => {
    const state = globalThis.__workspaceFixture;
    state.caResetError = true;
    state.caEvents = [];
  });
  page.once("dialog", (dialog) => dialog.accept());
  await resetCertificate.click();
  await caIdle();
  assert.match(
    await certificateNotice.textContent(),
    /Certificate reset failed: Fixture CA file deletion failed/,
  );
  assert.deepEqual(await page.evaluate(() => globalThis.__workspaceFixture.caEvents), ["reset"]);
  await page.evaluate(() => {
    globalThis.__workspaceFixture.caResetError = false;
    globalThis.__workspaceFixture.caTrustError = true;
    globalThis.__workspaceFixture.caEvents = [];
  });
  page.once("dialog", (dialog) => dialog.accept());
  await resetCertificate.click();
  await caIdle();
  assert.match(
    await certificateNotice.textContent(),
    /Certificate setup failed: Fixture Windows trust canceled/,
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.caTrustError = false;
    globalThis.__workspaceFixture.caEvents = [];
  });
  page.once("dialog", (dialog) => dialog.accept());
  await certificateNotice.getByRole("button", { name: "Try again", exact: true }).click();
  await caIdle();
  assert.deepEqual(
    await page.evaluate(() => globalThis.__workspaceFixture.caEvents),
    ["install"],
    "Retrying trust recreated the certificate",
  );
  await certificateNotice.getByRole("button", { name: "Start proxy", exact: true }).click();
  await caIdle();
  assert.equal(
    await resetCertificate.isDisabled(),
    true,
    "Reset remained available while the proxy was running",
  );
  await page.locator("#settings").getByRole("button", { name: "Stop proxy", exact: true }).click();
  await caIdle();
  assert.equal(await resetCertificate.isDisabled(), false);
  await page.evaluate(async () => {
    const shell = document.querySelector("app-shell");
    globalThis.__workspaceFixture.update = {
      ...globalThis.__workspaceFixture.update,
      phase: "idle",
      version: null,
      message: "You’re up to date with Stable updates.",
    };
    shell.openAbout();
    await shell.updates.check(true);
  });
  assert.equal(
    await page.locator(".update-toggle").isVisible(),
    false,
    "Toolbar Updates remains prominent with no update",
  );
  assert.match(
    await page.locator("#update-panel").textContent(),
    /up to date.*Last successful check/s,
  );
  await page.evaluate(async () => {
    globalThis.__workspaceFixture.updateNetworkError = true;
    await document.querySelector("app-shell").updates.check(true);
    globalThis.__workspaceFixture.updateNetworkError = false;
  });
  assert.match(await page.locator("#update-panel").textContent(), /GitHub unavailable/);
  assert.doesNotMatch(await page.locator("#update-panel").textContent(), /up to date/);
  const automaticChecks = page
    .locator("app-updates")
    .getByLabel("Automatically check for updates", { exact: true });
  assert.equal(await automaticChecks.isChecked(), true);
  await automaticChecks.uncheck();
  await page.waitForFunction(
    () => !document.querySelector("app-shell").updates.updatePreferenceBusy,
  );
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.update.automaticChecksEnabled),
    false,
  );
  const disabledChecks = await page.evaluate(
    () => globalThis.__workspaceFixture.calls.check_updates,
  );
  await page.evaluate(async () => await document.querySelector("app-shell").updates.check(false));
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.calls.check_updates),
    disabledChecks,
    "Disabled startup checking contacted updater",
  );
  await page
    .locator("#update-panel")
    .getByRole("button", { name: "Check for updates", exact: true })
    .click();
  await page.waitForFunction(() => !document.querySelector("app-shell").updates.updateBusy);
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.calls.check_updates),
    disabledChecks + 1,
    "Disabling automatic checks blocked manual checking",
  );
  assert.equal(await automaticChecks.isChecked(), false);
  await page.evaluate(() => (globalThis.__workspaceFixture.updatePreferenceError = true));
  await automaticChecks.click();
  await page.waitForFunction(
    () => !document.querySelector("app-shell").updates.updatePreferenceBusy,
  );
  assert.equal(
    await automaticChecks.isChecked(),
    false,
    "Failed preference save claimed automatic checking was enabled",
  );
  await page.evaluate(() => (globalThis.__workspaceFixture.updatePreferenceError = false));
  await automaticChecks.check();
  await page.waitForFunction(
    () => !document.querySelector("app-shell").updates.updatePreferenceBusy,
  );
  // Updates preserve startup, snooze, cancellation, drafts, and safe shutdown semantics.
  await page.evaluate(() => {
    const fixture = globalThis.__workspaceFixture;
    fixture.update = {
      ...fixture.update,
      channel: "Release",
      phase: "available",
      version: "0.2.0",
      notes: "A release note with <script>inert text</script>",
      message: "A new Stable update is available.",
    };
  });
  const updatePanel = page.locator("#update-panel");
  await page.evaluate(() => document.querySelector("app-shell").openAbout());
  await page.evaluate(async () => {
    await document.querySelector("app-shell").updates.check(true);
  });
  await updatePanel.getByRole("button", { name: "Update now", exact: true }).waitFor();
  await updatePanel.getByText("Stable release 0.2.0 is available.", { exact: true }).waitFor();
  await page.evaluate(() => {
    const shell = document.querySelector("app-shell");
    globalThis.__workspaceFixture.update = {
      ...globalThis.__workspaceFixture.update,
      channel: "Canary",
    };
    shell.updates.updateState = { ...shell.updates.updateState, channel: "Canary" };
    shell.onUpdateState(
      new CustomEvent("update-state-change", { detail: globalThis.__workspaceFixture.update }),
    );
  });
  await updatePanel.getByText("Canary build 0.2.0 is available.", { exact: true }).waitFor();
  await page.getByRole("button", { name: "Canary update available", exact: true }).waitFor();
  await page.evaluate(() => {
    const shell = document.querySelector("app-shell");
    globalThis.__workspaceFixture.update = {
      ...globalThis.__workspaceFixture.update,
      channel: "Release",
    };
    shell.updates.updateState = { ...shell.updates.updateState, channel: "Release" };
    shell.onUpdateState(
      new CustomEvent("update-state-change", { detail: globalThis.__workspaceFixture.update }),
    );
  });
  await updatePanel.getByText("Release notes", { exact: true }).click();
  assert.match(await updatePanel.locator("pre").textContent(), /<script>inert text<\/script>/);
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/update-available.png") });
  await page.evaluate(() => {
    const fixture = globalThis.__workspaceFixture;
    fixture.update = {
      ...fixture.update,
      channel: "Canary",
      message: "A new Canary update is available.",
    };
    fixture.deferUpdateCheck = true;
    void document.querySelector("app-shell").updates.check(true);
  });
  await page.waitForFunction(() => globalThis.__workspaceFixture.updateCheckPending);
  await updatePanel.getByText("Checking for Canary updates…", { exact: true }).waitFor();
  const checksWhilePending = await page.evaluate(
    () => globalThis.__workspaceFixture.calls.check_updates,
  );
  await page.evaluate(async () => {
    await document.querySelector("app-shell").updates.check(true);
  });
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.calls.check_updates),
    checksWhilePending,
    "Manual checking duplicated an in-flight startup check",
  );
  await view("traffic");
  await page.evaluate(() => globalThis.__workspaceFixture.releaseUpdateCheck());
  await page.waitForFunction(() => !document.querySelector("app-shell").updates.updateBusy);
  assert.equal(await updatePanel.isVisible(), false, "A late check navigated back to About");
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").activeView),
    "traffic",
  );
  await page.locator(".update-toggle").click();
  await page.setViewportSize({ width: 620, height: 640 });
  for (const name of ["Update now", "Update after this session ends", "Remind me in 30 days"]) {
    const button = updatePanel.getByRole("button", { name, exact: true });
    const bounds = await button.boundingBox();
    assert.ok(
      bounds.x >= 0 && bounds.x + bounds.width <= 620 && bounds.y + bounds.height <= 640,
      `Update action clipped: ${name}`,
    );
  }
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/update-narrow.png") });
  await page.setViewportSize({ width: 1280, height: 800 });
  await updatePanel.getByRole("button", { name: "Remind me in 30 days", exact: true }).click();
  await page.waitForFunction(() => !document.querySelector("app-shell").updateToolbarVisible);
  await view("traffic");
  await page.evaluate(async () => {
    await document.querySelector("app-shell").updates.check(false);
  });
  assert.equal(await updatePanel.isVisible(), false, "A snoozed update reappeared at startup");
  await page.evaluate(async () => {
    const shell = document.querySelector("app-shell");
    shell.openAbout();
    await shell.updates.check(true);
  });
  assert.match(await updatePanel.textContent(), /Automatic prompts paused until/);
  await updatePanel
    .getByRole("button", { name: "Update after this session ends", exact: true })
    .click();
  await updatePanel.getByRole("button", { name: "Install and reopen", exact: true }).waitFor();
  assert.equal(await page.evaluate(() => globalThis.__workspaceFixture.update.scheduled), true);
  await updatePanel.getByRole("button", { name: "Cancel update", exact: true }).click();
  await updatePanel.getByRole("button", { name: "Update now", exact: true }).waitFor();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.deferUpdateDownload = true;
  });
  await updatePanel.getByRole("button", { name: "Update now", exact: true }).click();
  await page.waitForFunction(() => globalThis.__workspaceFixture.updateDownloadPending);
  await updatePanel.getByRole("button", { name: "Cancel update", exact: true }).click();
  await updatePanel.getByRole("button", { name: "Update now", exact: true }).waitFor();
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.updateInstallCalls ?? 0),
    0,
    "Cancelled download initiated installation",
  );
  await page.evaluate(() => {
    const state = globalThis.__workspaceFixture;
    state.deferUpdateDownload = false;
    state.updateRestoreError = true;
    const shell = document.querySelector("app-shell");
    shell.settings.settingsDirty = false;
    if (shell.composer) shell.composer.composerDirty = false;
    if (shell.automation) shell.automation.draftDirty = false;
  });
  await updatePanel
    .getByRole("button", { name: "Update after this session ends", exact: true })
    .click();
  await updatePanel.getByRole("button", { name: "Install and reopen", exact: true }).waitFor();
  await page.evaluate(() => globalThis.__workspaceFixture.updateCloseChannel.onmessage(null));
  await page.waitForFunction(() =>
    document.querySelector("app-shell").updates.updateError.includes("restoration failed"),
  );
  assert.equal(
    await page.evaluate(() => Boolean(globalThis.__workspaceFixture.updateInstallerStarted)),
    false,
  );
  await updatePanel.getByRole("button", { name: "Cancel update", exact: true }).click();
  await updatePanel.getByRole("button", { name: "Update now", exact: true }).waitFor();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.updateRestoreError = false;
    document.querySelector("app-shell").settings.settingsDirty = true;
  });
  await updatePanel.getByRole("button", { name: "Update now", exact: true }).click();
  const draftDialog = page.getByRole("dialog", { name: "Unsaved work" });
  await draftDialog.getByRole("button", { name: "Keep working", exact: true }).click();
  assert.equal(await page.evaluate(() => globalThis.__workspaceFixture.updateInstallCalls), 1);
  await updatePanel.getByRole("button", { name: "Install and reopen", exact: true }).click();
  await draftDialog.getByRole("button", { name: "Discard drafts and update", exact: true }).click();
  await page.waitForFunction(() => globalThis.__workspaceFixture.updateInstallerStarted);
  assert.equal(await page.evaluate(() => globalThis.__workspaceFixture.updateReopen), true);
  await view("traffic");
  assert.deepEqual(await page.evaluate(() => globalThis.__cspViolations), []);
  assert.deepEqual(errors, []);

  await view("traffic");

  await page.evaluate(async () => {
    const traffic = document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace");
    await traffic.clearFilters();
    const state = globalThis.__workspaceFixture;
    state.sessions.find((row) => row.id === "first").searchBody = "Café marker";
    state.sessions.find((row) => row.id === "second").searchBody = "CAFE other";
  });
  const searchOptions = page.locator("#traffic-search-options");
  await page.getByLabel("Traffic search mode", { exact: true }).selectOption("contents");
  await page.getByRole("button", { name: "Search options", exact: true }).click();
  const searchButton = await page
    .getByRole("button", { name: "Search options", exact: true })
    .boundingBox();
  const searchMenu = await searchOptions.boundingBox();
  assert.ok(
    searchMenu.x < searchButton.x + searchButton.width &&
      searchMenu.x + searchMenu.width > searchButton.x &&
      Math.abs(searchMenu.y - searchButton.y - searchButton.height) < 12,
    "Search options menu is detached from its button",
  );
  await searchOptions.getByLabel("URL, method, process, PID and status", { exact: true }).uncheck();
  await searchOptions.getByLabel("Request headers", { exact: true }).uncheck();
  await searchOptions.getByLabel("Response headers", { exact: true }).uncheck();
  await searchOptions.getByLabel("Ignore accents (text mode)", { exact: true }).check();
  await searchOptions.getByLabel("Select all matches after searching", { exact: true }).check();
  await page.getByLabel("Traffic search mode", { exact: true }).selectOption("contents");
  await page.getByLabel("Filter or search traffic", { exact: true }).fill("cafe");
  await page
    .locator("#traffic .filters")
    .getByRole("button", { name: "Search", exact: true })
    .click();
  await page.getByRole("button", { name: "Select all 2 entries", exact: true }).waitFor();
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
        .selectedTrafficCount === 2,
  );
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.lastContentSearch.ignoreDiacritics),
    true,
  );
  assert.doesNotMatch(await page.locator(".content-search-status").textContent(), /binary|image/i);
  await page.getByRole("button", { name: "View matches…", exact: true }).click();
  const matchDialog = page.locator(".search-match-dialog[open]");
  await matchDialog.getByText(/Entry [12] of 2 · Match 1 of 2/).waitFor();
  const initialMatchEntry = await page.evaluate(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
        .matchEntryId,
  );
  assert.equal(await matchDialog.locator("mark").textContent(), "Café");
  assert.equal(await page.evaluate(() => globalThis.matchUnsafe), undefined);
  await matchDialog.getByRole("button", { name: "Next match", exact: false }).click();
  assert.equal(await matchDialog.locator("mark").textContent(), "Café");
  await matchDialog.press("Shift+F3");
  assert.equal(await matchDialog.locator("mark").textContent(), "Café");
  const nextEntry = matchDialog.getByRole("button", { name: "Next entry", exact: true });
  if (await nextEntry.isEnabled()) await nextEntry.click();
  else await matchDialog.getByRole("button", { name: "Previous entry", exact: true }).click();
  await page.waitForFunction(
    (previous) =>
      document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
        .matchEntryId !== previous,
    initialMatchEntry,
  );
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/search-matches-wide.png"),
  });
  await page.setViewportSize({ width: 760, height: 520 });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/search-matches-compact.png"),
  });
  const matchBounds = await matchDialog.boundingBox();
  assert.ok(matchBounds.width <= 760 && matchBounds.height <= 520);
  await matchDialog.getByRole("button", { name: "Close", exact: true }).click();
  assert.equal(
    await page
      .locator("tr[data-current]")
      .evaluate((node) => node.getRootNode().activeElement === node),
    true,
  );
  await page.setViewportSize({ width: 1280, height: 800 });

  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/content-search-wide.png"),
  });
  await page.getByLabel("Filter or search traffic", { exact: true }).fill("marker");
  await page
    .locator("#traffic .filters")
    .getByRole("button", { name: "Search", exact: true })
    .click();
  await page.waitForFunction(() => {
    const traffic = document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace");
    return (
      !traffic.searchingTraffic &&
      traffic.selectedTrafficCount === 1 &&
      traffic.trafficSelection.ids.has("first")
    );
  });
  await page.locator('tr[data-session-id="first"]').click({ button: "right" });
  await page
    .locator("#traffic-entry-menu")
    .getByRole("button", { name: "Remove unselected entries", exact: true })
    .click();
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
        .removingTraffic,
  );
  await page.getByRole("button", { name: "Clear search", exact: true }).click();
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
        .totalMatched === 1,
  );
  await page
    .locator(".traffic-undo")
    .getByRole("button", { name: "Undo traffic removal", exact: true })
    .click();
  await page.waitForFunction(() => {
    const traffic = document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace");
    return !traffic.removingTraffic && traffic.totalMatched >= 4;
  });
  await page.getByRole("button", { name: "Search options", exact: true }).click();
  await searchOptions.getByLabel("Match mode", { exact: true }).selectOption("regex");
  assert.equal(
    await searchOptions.getByLabel("Ignore accents (text mode)", { exact: true }).isDisabled(),
    true,
  );
  await page.getByLabel("Filter or search traffic", { exact: true }).fill("caf[ée]");
  await page
    .locator("#traffic .filters")
    .getByRole("button", { name: "Search", exact: true })
    .click();
  await page.getByRole("button", { name: "Select all 2 entries", exact: true }).waitFor();
  const searchFailureSelection = await page.evaluate(() =>
    Array.from(
      document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
        .trafficSelection.ids,
    ),
  );
  await page.getByLabel("Filter or search traffic", { exact: true }).fill("(");
  await page
    .locator("#traffic .filters")
    .getByRole("button", { name: "Search", exact: true })
    .click();
  await page
    .locator(".content-search-status")
    .getByText(/Search failed:/)
    .waitFor();
  assert.deepEqual(
    await page.evaluate(() =>
      Array.from(
        document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
          .trafficSelection.ids,
      ),
    ),
    searchFailureSelection,
  );
  assert.equal(
    await page.evaluate(
      () =>
        document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
          .selectedTrafficCount,
    ),
    2,
  );
  const searchCancelSelection = await page.evaluate(() =>
    Array.from(
      document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
        .trafficSelection.ids,
    ),
  );
  await page.evaluate(() => (globalThis.__workspaceFixture.deferSearch = true));
  await page.getByLabel("Filter or search traffic", { exact: true }).fill("body");
  await page
    .locator("#traffic .filters")
    .getByRole("button", { name: "Search", exact: true })
    .click();
  await page.getByRole("button", { name: "Cancel search", exact: true }).click();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.releaseSearch();
    globalThis.__workspaceFixture.deferSearch = false;
  });
  await page
    .locator(".content-search-status")
    .getByText("Search canceled. Previous results and selections are unchanged.", { exact: true })
    .waitFor();
  assert.deepEqual(
    await page.evaluate(() =>
      Array.from(
        document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
          .trafficSelection.ids,
      ),
    ),
    searchCancelSelection,
  );
  const previousSearchToken = await page.evaluate(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
        .searchResultId,
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.deferSearch = true;
    globalThis.__workspaceFixture.ignoreSearchCancel = true;
  });
  await page
    .locator("#traffic .filters")
    .getByRole("button", { name: "Search", exact: true })
    .click();
  await page.getByRole("button", { name: "Cancel search", exact: true }).click();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.releaseSearch();
    globalThis.__workspaceFixture.deferSearch = false;
  });
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
        .searchingTraffic,
  );
  assert.equal(
    await page.evaluate(
      () =>
        document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
          .searchResultId,
    ),
    previousSearchToken,
    "A late successful canceled search replaced the old result",
  );
  assert.deepEqual(
    await page.evaluate(() =>
      Array.from(
        document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
          .trafficSelection.ids,
      ),
    ),
    searchCancelSelection,
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.ignoreSearchCancel = false;
    globalThis.__workspaceFixture.searchQueryFailure = true;
  });
  await page
    .locator("#traffic .filters")
    .getByRole("button", { name: "Search", exact: true })
    .click();
  await page
    .locator(".content-search-status")
    .getByText(/Search failed: Fixture result paging failed/)
    .waitFor();
  assert.equal(
    await page.evaluate(
      () =>
        document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
          .searchResultId,
    ),
    previousSearchToken,
  );
  assert.deepEqual(
    await page.evaluate(() =>
      Array.from(
        document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
          .trafficSelection.ids,
      ),
    ),
    searchCancelSelection,
  );
  await page.evaluate(() => (globalThis.__workspaceFixture.searchQueryFailure = false));

  await page.getByRole("button", { name: "Clear search", exact: true }).click();
  await page.getByRole("button", { name: "Search options", exact: true }).click();
  await searchOptions.getByLabel("Match mode", { exact: true }).selectOption("text");
  await searchOptions.getByLabel("URL, method, process, PID and status", { exact: true }).check();
  await searchOptions.getByLabel("Request headers", { exact: true }).check();
  await searchOptions.getByLabel("Response headers", { exact: true }).check();
  await searchOptions.getByLabel("Select all matches after searching", { exact: true }).uncheck();
  await page.getByRole("button", { name: "Search options", exact: true }).click();
  await page.evaluate(
    () => (globalThis.__workspaceFixture.pickedTrace = "C:/captures/support.saz"),
  );
  await page.locator("#traffic").getByRole("button", { name: "Files", exact: true }).click();
  await page.getByRole("button", { name: "Import traffic…", exact: true }).click();
  await page
    .locator(".trace-import-status")
    .getByText(/Imported 1 entry from support.saz/)
    .waitFor();
  const trafficBeforeFailure = await page.evaluate(() =>
    structuredClone(globalThis.__workspaceFixture.sessions),
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.importError =
      "Capture format is invalid. " + "Long capture details ".repeat(30);
  });
  await page.locator("#traffic").getByRole("button", { name: "Files", exact: true }).click();
  await page.getByRole("button", { name: "Import traffic…", exact: true }).click();
  const operationError = page.getByRole("alertdialog", { name: "Import failed", exact: true });
  await operationError.waitFor();
  assert.match(await operationError.textContent(), /support.saz.*Capture format is invalid/s);
  assert.match(await operationError.textContent(), /Traffic is unchanged/);
  assert.equal(
    await operationError
      .getByRole("button", { name: "Close", exact: true })
      .evaluate((node) => node.getRootNode().activeElement === node),
    true,
  );
  assert.deepEqual(
    await page.evaluate(() => globalThis.__workspaceFixture.sessions),
    trafficBeforeFailure,
  );
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/import-error-wide.png") });
  await page.setViewportSize({ width: 760, height: 520 });
  const errorBounds = await operationError.boundingBox();
  assert.ok(errorBounds.width <= 760 && errorBounds.height <= 520);
  assert.equal(
    await operationError.getByRole("button", { name: "Close", exact: true }).evaluate((node) => {
      const rect = node.getBoundingClientRect();
      return (
        node.getRootNode().elementFromPoint(rect.x + rect.width / 2, rect.y + rect.height / 2) ===
        node
      );
    }),
    true,
  );
  await page.keyboard.press("Tab");
  assert.equal(
    await page
      .locator("#traffic")
      .getByRole("button", { name: "Files", exact: true })
      .evaluate((node) => node.getRootNode().activeElement === node),
    false,
  );
  await page.keyboard.press("Shift+Tab");
  assert.equal(
    await operationError
      .getByRole("button", { name: "Close", exact: true })
      .evaluate((node) => node.getRootNode().activeElement === node),
    true,
  );
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/import-error-compact.png"),
  });
  await page.keyboard.press("Escape");
  await operationError.waitFor({ state: "hidden" });
  await page.waitForFunction(() => {
    const traffic = document.querySelector("app-shell").traffic;
    return traffic.filesMenuButton.getRootNode().activeElement === traffic.filesMenuButton;
  });
  await page.getByRole("button", { name: "Dismiss import message", exact: true }).click();
  assert.equal(await page.locator(".trace-import-status").count(), 0);
  await page.evaluate(
    () => (globalThis.__workspaceFixture.importError = "Capture format is invalid."),
  );
  await page.locator("#traffic").getByRole("button", { name: "Files", exact: true }).click();
  await page.getByRole("button", { name: "Import traffic…", exact: true }).click();
  await operationError.waitFor();
  await operationError.getByRole("button", { name: "Close", exact: true }).click();
  await operationError.waitFor({ state: "hidden" });
  await page.evaluate(() => {
    delete globalThis.__workspaceFixture.importError;
    document.querySelector("app-shell").diagnosticText = "Fixture status message";
  });
  await page.getByRole("button", { name: "Dismiss status message", exact: true }).click();
  assert.equal(await page.evaluate(() => document.querySelector("app-shell").diagnosticText), "");
  await page.evaluate(() => {
    const traffic = document.querySelector("app-shell").traffic;
    globalThis.__errorQueue = Promise.all([
      traffic.showError("First failure", "First operation failed."),
      traffic.showError("Second failure", "Second operation failed."),
    ]);
  });
  await page
    .getByRole("alertdialog", { name: "First failure", exact: true })
    .getByRole("button", { name: "Close", exact: true })
    .click();
  await page
    .getByRole("alertdialog", { name: "Second failure", exact: true })
    .getByRole("button", { name: "Close", exact: true })
    .click();
  await page.evaluate(async () => {
    await globalThis.__errorQueue;
    delete globalThis.__errorQueue;
  });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.getByRole("button", { name: "Files", exact: true }).click();
  await page
    .locator("#trace-files-menu")
    .getByRole("button", { name: "View current imports…", exact: true })
    .click();
  await page
    .locator(".trace-metadata-dialog[open] h3")
    .getByText("support.saz", { exact: true })
    .waitFor();
  assert.match(
    await page.locator(".trace-metadata-dialog .trace-context").textContent(),
    /Captured machine IP configuration/,
  );
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/trace-metadata-wide.png"),
  });
  await page
    .locator(".trace-metadata-dialog")
    .getByRole("button", { name: "Close", exact: true })
    .click();
  assert.equal(
    await page
      .getByRole("button", { name: "Files", exact: true })
      .evaluate((node) => node === node.getRootNode().activeElement),
    true,
    "Metadata close did not restore Files options focus",
  );
  await page.evaluate(async () => {
    globalThis.__workspaceFixture.openedTraces = ["C:/captures/second.saz"];
    await document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace")
      .takeOpenedTraces();
  });
  await page.locator(".trace-open-dialog[open]").waitFor();
  await page
    .locator(".trace-open-dialog")
    .getByRole("button", { name: "Open separate viewer", exact: true })
    .click();
  assert.deepEqual(await page.evaluate(() => globalThis.__workspaceFixture.openedViewer), [
    "C:/captures/second.saz",
  ]);
  await page.evaluate(() => {
    globalThis.__workspaceFixture.deferImport = true;
  });
  await page.locator("#traffic").getByRole("button", { name: "Files", exact: true }).click();
  await page.getByRole("button", { name: "Import traffic…", exact: true }).click();
  await page.getByRole("button", { name: "Cancel import", exact: true }).click();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.releaseImport();
    globalThis.__workspaceFixture.deferImport = false;
  });
  await page
    .locator(".trace-import-status")
    .getByText("Import canceled. Traffic is unchanged.", { exact: true })
    .waitFor();
  assert.equal(await page.locator(".operation-error-dialog[open]").count(), 0);
  await checkLoopback(page, root);
  await checkClientCertificates(page, root);
  await checkExtensions(page, root);
  await checkCopy(page, root);
  await checkAuth(page, root);
  await checkVisualizers(page, root);
  await page.goto("https://workspace.test/?viewer");
  await page.getByText("Capture viewer", { exact: true }).waitFor();
  assert.equal(await page.getByRole("button", { name: "Start proxy", exact: true }).count(), 0);
  assert.equal(await page.locator("settings-workspace").count(), 0);
  assert.equal(
    await page.locator("#trace-files-menu button").filter({ hasText: "Save copy…" }).count(),
    1,
  );
  await page.locator("#traffic").getByRole("button", { name: "Files", exact: true }).waitFor();
  const viewerRows = await page.evaluate(async () => {
    const s = globalThis.__workspaceFixture,
      t = document.querySelector("app-shell").traffic,
      rows = s.sessions;
    s.sessions = [];
    await t.refreshSessions(undefined, true);
    return rows;
  });
  assert.match(await page.locator(".empty-cell").textContent(), /Import a saved capture/);
  await page.getByLabel("Filter or search traffic", { exact: true }).fill("not-present");
  await page.waitForFunction(() =>
    document.querySelector("app-shell").traffic.sessionText.includes("No matching saved traffic"),
  );
  assert.match(await page.locator(".empty-cell").textContent(), /No matching (saved )?traffic/);
  await page.getByLabel("Filter or search traffic", { exact: true }).fill("");
  await page.evaluate(async (rows) => {
    globalThis.__workspaceFixture.sessions = rows;
    await document.querySelector("app-shell").traffic.refreshSessions(undefined, true);
  }, viewerRows);

  await page.evaluate(async () => {
    globalThis.__workspaceFixture.openedTraces = ["C:/captures/viewer.saz"];
    await document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace")
      .takeOpenedTraces();
  });
  await page
    .locator(".trace-import-status")
    .getByText(/Imported 1 entry from viewer.saz/)
    .waitFor();
  assert.equal(await page.locator(".trace-open-dialog[open]").count(), 0);
  await page.getByRole("button", { name: "Open main window", exact: true }).click();
  assert.equal(await page.evaluate(() => globalThis.__workspaceFixture.calls.open_main_window), 1);
  await page.setViewportSize({ width: 800, height: 600 });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/capture-viewer-compact.png"),
  });
  const passwordRowsBefore = await page.evaluate(() =>
    structuredClone(globalThis.__workspaceFixture.sessions),
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.importPassword = "test-password";
    const traffic = document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace");
    void traffic.importTrace("C:/captures/protected.tmcap");
  });
  await page
    .locator(".trace-password-dialog[open]")
    .getByRole("button", { name: "Cancel", exact: true })
    .click();
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
        .importingTrace,
  );
  assert.deepEqual(
    await page.evaluate(() => globalThis.__workspaceFixture.sessions),
    passwordRowsBefore,
  );
  await page.evaluate(() => {
    const traffic = document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace");
    void traffic.importTrace("C:/captures/protected.tmcap");
  });
  await page
    .locator(".trace-password-dialog[open]")
    .getByLabel("Password", { exact: true })
    .fill("incorrect");
  await page
    .locator(".trace-password-dialog[open]")
    .getByRole("button", { name: "Continue", exact: true })
    .click();
  await page
    .locator(".trace-password-dialog[open]")
    .getByText("The capture password is incorrect", { exact: true })
    .waitFor();
  await page
    .locator(".trace-password-dialog[open]")
    .getByLabel("Password", { exact: true })
    .fill("test-password");
  assert.equal(
    await page
      .locator(".trace-password-dialog[open]")
      .getByLabel("Password", { exact: true })
      .getAttribute("type"),
    "password",
  );
  await page
    .locator(".trace-password-dialog[open]")
    .getByRole("button", { name: "Continue", exact: true })
    .click();
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
        .importingTrace,
  );
  assert.equal(
    (await page.evaluate(() => globalThis.__workspaceFixture.sessions)).length,
    passwordRowsBefore.length + 1,
  );
  await page.evaluate((rows) => {
    const state = globalThis.__workspaceFixture;
    state.sessions = rows;
    delete state.importPassword;
    state.traces.pop();
  }, passwordRowsBefore);
  await page.getByRole("button", { name: "Files", exact: true }).click();
  await page
    .locator("#trace-files-menu")
    .getByRole("button", { name: "Save copy…", exact: true })
    .click();
  const saveTrace = page.locator(".trace-save-dialog[open]");
  await saveTrace.getByLabel("Format", { exact: true }).selectOption("saz-strict");
  assert.equal(
    await saveTrace.getByLabel("Encrypt with a password (AES-256)", { exact: true }).isDisabled(),
    false,
  );
  await checkTraceSave(page, root, saveTrace);
  assert.equal(
    (await page.evaluate(() => globalThis.__workspaceFixture.savedTraceArgs)).options.format,
    "saz-strict",
  );
  await page.getByRole("button", { name: "Files", exact: true }).click();
  await page
    .locator("#trace-files-menu")
    .getByRole("button", { name: "Save copy…", exact: true })
    .click();
  await saveTrace.getByLabel("Format", { exact: true }).selectOption("har");
  assert.equal(
    await saveTrace.getByLabel("Encrypt with a password (AES-256)", { exact: true }).isDisabled(),
    true,
  );
  assert.equal(
    await saveTrace
      .getByLabel("Include this computer’s network configuration", { exact: true })
      .isDisabled(),
    true,
  );
  await saveTrace.getByLabel("Format", { exact: true }).selectOption("native");
  assert.equal(
    await saveTrace
      .getByLabel("Include this computer’s network configuration", { exact: true })
      .isChecked(),
    false,
  );
  assert.equal(
    await saveTrace
      .getByLabel("Remove authentication and cookie header values", { exact: true })
      .isChecked(),
    false,
  );
  await saveTrace
    .getByLabel("Remove authentication and cookie header values", { exact: true })
    .check();
  await saveTrace
    .getByLabel("Include this computer’s network configuration", { exact: true })
    .check();
  await page.evaluate(() => (globalThis.__workspaceFixture.saveTraceError = true));
  await saveTrace.getByRole("button", { name: "Save as…", exact: true }).click();
  await page
    .getByRole("alertdialog", { name: "Trace save failed", exact: true })
    .getByRole("button", { name: "Close", exact: true })
    .click();
  await saveTrace.getByText(/Trace could not be saved:/).waitFor();
  assert.equal(
    await saveTrace
      .getByLabel("Include this computer’s network configuration", { exact: true })
      .isChecked(),
    true,
  );
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/save-trace-compact.png") });
  await page.evaluate(() => {
    globalThis.__workspaceFixture.saveTraceError = false;
    globalThis.__workspaceFixture.cancelTraceSave = true;
  });
  await saveTrace.getByRole("button", { name: "Save as…", exact: true }).click();
  await saveTrace.getByText("Save canceled.", { exact: true }).waitFor();
  await page.evaluate(() => (globalThis.__workspaceFixture.cancelTraceSave = false));
  await saveTrace.getByLabel("Encrypt with a password (AES-256)", { exact: true }).check();
  await saveTrace.getByRole("button", { name: "Save as…", exact: true }).click();
  const passwordDialog = page.locator(".trace-password-dialog[open]");
  await passwordDialog.getByLabel("Password", { exact: true }).fill("test-password");
  await passwordDialog.getByLabel("Confirm password", { exact: true }).fill("different");
  await passwordDialog.getByRole("button", { name: "Continue", exact: true }).click();
  await passwordDialog.getByText("The passwords do not match.", { exact: true }).waitFor();
  await passwordDialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await saveTrace.getByText("Save canceled.", { exact: true }).waitFor();
  assert.equal(
    await page.locator('.trace-password-dialog input[name="password"]').inputValue(),
    "",
  );
  await saveTrace.getByLabel("Encrypt with a password (AES-256)", { exact: true }).uncheck();
  await saveTrace.getByRole("button", { name: "Save as…", exact: true }).click();
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
        .savingTrace,
  );
  assert.equal(await page.locator(".trace-save-dialog[open]").count(), 0);
  assert.deepEqual(
    await page.evaluate(() => {
      const { reviewId, ...options } = globalThis.__workspaceFixture.savedTraceArgs.options;
      return options;
    }),
    {
      includeOriginalContext: true,
      includeMarkers: true,
      format: "native",
      password: null,
      includeNetworkContext: true,
      redactSensitiveHeaders: true,
    },
  );
  await page.getByRole("button", { name: "Files", exact: true }).click();
  await page
    .locator("#trace-files-menu")
    .getByRole("button", { name: "Save copy…", exact: true })
    .click();
  await page
    .locator(".trace-save-dialog[open]")
    .getByLabel("Encrypt with a password (AES-256)", { exact: true })
    .check();
  const callbacksBeforeCancel = await page.evaluate(() =>
    globalThis.__workspaceFixture.callbackCount(),
  );
  for (let attempt = 0; attempt < 3; attempt++) {
    await page
      .locator(".trace-save-dialog[open]")
      .getByRole("button", { name: "Save as…", exact: true })
      .click();
    await page
      .locator(".trace-password-dialog[open]")
      .getByRole("button", { name: "Cancel", exact: true })
      .click();
    await page.waitForFunction(() => !document.querySelector("app-shell").traffic.savingTrace);
    assert.equal(
      await page.evaluate(() => globalThis.__workspaceFixture.callbackCount()),
      callbacksBeforeCancel,
      "Canceled password prompt leaked a progress channel",
    );
  }
  await page
    .locator(".trace-save-dialog[open]")
    .getByRole("button", { name: "Save as…", exact: true })
    .click();
  await page
    .locator(".trace-password-dialog[open]")
    .getByLabel("Password", { exact: true })
    .fill("test-password");
  await page
    .locator(".trace-password-dialog[open]")
    .getByLabel("Confirm password", { exact: true })
    .fill("test-password");
  await page
    .locator(".trace-password-dialog[open]")
    .getByRole("button", { name: "Continue", exact: true })
    .click();
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
        .savingTrace,
  );
  assert.equal(
    (await page.evaluate(() => globalThis.__workspaceFixture.savedTraceArgs)).options.password,
    "test-password",
  );
  assert.equal(
    await page.locator('.trace-password-dialog input[name="password"]').inputValue(),
    "",
  );
  await page.evaluate(() => {
    const state = globalThis.__workspaceFixture;
    state.performance = {
      work: [
        {
          kind: "hook",
          label: "Request headers · support script",
          beganOffsetMicros: 0,
          endedOffsetMicros: 1000,
          busyNanos: 1000000,
          calls: 1,
        },
        {
          kind: "pause",
          label: "Breakpoint decision",
          beganOffsetMicros: 500,
          endedOffsetMicros: 1500,
          busyNanos: 1000000,
          calls: 1,
        },
        {
          kind: "transform",
          label: "Response body · support script",
          beganOffsetMicros: 1800,
          endedOffsetMicros: 3300,
          busyNanos: 250000,
          calls: 3,
        },
      ],
      points: [
        { milestone: "client-connected", unixMillis: 1800000000000, offsetMicros: -2500 },
        { milestone: "request-headers", unixMillis: 1800000000002, offsetMicros: 0 },
        { milestone: "exchange-done", unixMillis: 1800000000005, offsetMicros: 3500 },
      ],
      protocols: [],
      transports: [
        {
          leg: "upstream",
          connectionId: "shared-h2-fixture",
          shared: true,
          outcome: "connected",
          sampledOffsetMicros: 3000,
          peer: "192.0.2.1:443",
          local: "192.0.2.2:54321",
          dnsMicros: null,
          tcpMicros: 0,
          tlsMicros: 2000,
          setupTimings: [
            {
              phase: "tcp",
              beganOffsetMicros: -20000,
              endedOffsetMicros: -18800,
              requestWaitMicros: 0,
            },
            {
              phase: "tls",
              beganOffsetMicros: -8000,
              endedOffsetMicros: 2000,
              requestWaitMicros: 2000,
            },
          ],
          tlsVersion: "TLSv1.3",
          tlsResumed: false,
          cipher: "TLS_AES_128_GCM_SHA256",
          alpn: "h2",
          bytesRead: 10240,
          bytesWritten: 1024,
          tcpSampledOffsetMicros: 2500,
          tcp: {
            rttMicros: 2400,
            minRttMicros: 1200,
            congestionWindow: 65536,
            sendWindow: 32768,
            receiveWindow: 131072,
            unacknowledgedBytes: 512,
            retransmittedBytes: 2048,
            retransmittedSegments: null,
            fastRetransmissions: 2,
            duplicateAcks: 3,
            timeoutEpisodes: 0,
            mss: 1460,
            connectionAgeMillis: 1300,
          },
          socketIo: {
            writeWaitMicros: 1750,
            writeWaits: 2,
            lastWriteOffsetMicros: 2800,
            lastFlushOffsetMicros: 2900,
          },
        },
      ],
    };
    state.savedTiming = { ClientBeginRequest: "00:00:00.000" };
  });
  await page.locator(".session-link").first().click();
  await page
    .locator(".selection-actions")
    .getByRole("button", { name: "Details…", exact: true })
    .click();
  const timing = page.locator(".timing-dialog[open]");
  await timing.locator(".waterfall-axis").getByText("3.5 ms", { exact: true }).waitFor();
  assert.match(await timing.textContent(), /Negative offsets/);
  await timing.getByText("Request waterfall", { exact: true }).waitFor({ state: "visible" });
  assert.equal(
    await timing
      .locator(".waterfall-bar")
      .first()
      .evaluate((node) => node.namespaceURI),
    "http://www.w3.org/2000/svg",
  );
  const minor = timing
    .locator("details")
    .filter({ has: page.getByText(/Proxy operations under 1 ms/) });
  assert.equal(await minor.evaluate((node) => node.open), false);
  await minor.locator("summary").click();
  assert.ok((await minor.locator(".waterfall-span[data-repeated]").count()) > 0);
  await minor.locator("summary").click();
  await page.evaluate(() =>
    globalThis.__workspaceFixture.performance.transports.push({
      leg: "upstream",
      connectionId: "failed-quic-fixture",
      shared: false,
      outcome: "tls-failed",
      sampledOffsetMicros: 3000,
      peer: null,
      local: null,
      dnsMicros: null,
      tcpMicros: null,
      tlsMicros: 1000,
      setupTimings: [
        {
          phase: "quic",
          beganOffsetMicros: 1000,
          endedOffsetMicros: 2000,
          requestWaitMicros: 1000,
        },
      ],
      tlsVersion: null,
      tlsResumed: null,
      cipher: null,
      alpn: null,
      bytesRead: null,
      bytesWritten: null,
      quic: null,
    }),
  );
  await timing.getByRole("button", { name: "Refresh measurements", exact: true }).click();
  await timing
    .getByText("QUIC / TLS wait for this request", { exact: true })
    .waitFor({ state: "attached" });
  assert.equal(await timing.getByText("TCP wait for this request", { exact: true }).count(), 1);
  await timing.getByRole("tab", { name: "Connection", exact: true }).click();
  await timing.getByText("Proxy ↔ upstream", { exact: true }).last().click();
  await timing
    .getByText("QUIC / TLS wait for this request", { exact: true })
    .waitFor({ state: "visible" });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/timings-failed-quic.png"),
  });
  await page.evaluate(() => globalThis.__workspaceFixture.performance.transports.pop());
  await timing.getByRole("button", { name: "Refresh measurements", exact: true }).click();
  await timing
    .getByText("QUIC / TLS wait for this request", { exact: true })
    .waitFor({ state: "detached" });
  const beforeTimingCopyExecutions = await page.evaluate(
    () => globalThis.__workspaceFixture.calls.execute_composer ?? 0,
  );
  await timing.getByRole("tab", { name: "Timings", exact: true }).click();
  await timing.getByRole("button", { name: "Copy report", exact: true }).click();
  await timing.getByText("Timing report copied.", { exact: true }).waitFor({ state: "visible" });
  const report = await page.evaluate(() => navigator.clipboard.readText());
  await timing.getByText("Report text", { exact: true }).click();
  assert.equal(
    await timing.getByLabel("Timing report", { exact: true }).inputValue(),
    report.replaceAll("\r\n", "\n"),
  );
  await timing.getByText("Report text", { exact: true }).click();
  assert.match(report, /GET .* → 200/);
  assert.match(report, /support script: 0.25 ms/);
  assert.match(report, /Breakpoint decision: 1 ms/);
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.calls.execute_composer ?? 0),
    beforeTimingCopyExecutions,
  );
  await page.setViewportSize({ width: 1280, height: 800 });
  await timing
    .getByText("Request waterfall", { exact: true })
    .evaluate((node) => node.scrollIntoView({ block: "start" }));
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/waterfall-wide.png") });
  await page.setViewportSize({ width: 800, height: 600 });
  await timing
    .getByText("Request waterfall", { exact: true })
    .evaluate((node) => node.scrollIntoView({ block: "start" }));
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/waterfall-compact.png") });
  await page.setViewportSize({ width: 1280, height: 800 });
  await timing.getByRole("tab", { name: "Connection", exact: true }).click();
  await timing.getByText("Proxy ↔ upstream", { exact: true }).click();
  assert.ok((await timing.textContent()).includes("Reused / shared connection"));
  const tcpWait = timing
    .locator(".timing-facts > div")
    .filter({ has: page.getByText("TCP wait for this request", { exact: true }) });
  assert.match(await tcpWait.textContent(), /0 ms.*Original connection phase 1.2 ms/);
  const tlsWait = timing
    .locator(".timing-facts > div")
    .filter({ has: page.getByText("TLS wait for this request", { exact: true }) });
  assert.match(await tlsWait.textContent(), /2 ms.*10 ms.*8 ms before.*2 ms after/);
  assert.match(await timing.textContent(), /Connection setup started20 ms before/);
  assert.match(await timing.textContent(), /TLSv1.3/);
  assert.match(await timing.textContent(), /TCP estimated round trip2.4 ms/);
  assert.match(await timing.textContent(), /Retransmitted TCP data2,048 bytes/);
  assert.match(await timing.textContent(), /Socket write \/ flush wait1.75 ms/);
  assert.match(report, /Unacknowledged sent data: 512 bytes/);
  await timing.getByRole("tab", { name: "Timings", exact: true }).click();
  await timing.getByText("Original imported timing and session evidence", { exact: true }).click();
  await timing.getByText("00:00:00.000", { exact: true }).waitFor();
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/timings-wide.png") });
  await timing.getByRole("tab", { name: "Connection", exact: true }).click();
  await timing
    .getByText("Proxy ↔ upstream", { exact: true })
    .evaluate((node) => node.scrollIntoView({ block: "start" }));
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/shared-setup-wide.png") });
  await page.setViewportSize({ width: 800, height: 600 });
  await timing.getByRole("tab", { name: "Connection", exact: true }).click();
  await timing
    .getByText("Proxy ↔ upstream", { exact: true })
    .evaluate((node) => node.scrollIntoView({ block: "start" }));
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/shared-setup-compact.png"),
  });
  await timing
    .getByText("TCP estimated round trip", { exact: true })
    .evaluate((node) => node.scrollIntoView({ block: "start" }));
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/tcp-statistics-compact.png"),
  });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/tcp-statistics-wide.png"),
  });
  await page.setViewportSize({ width: 800, height: 600 });
  const timingBounds = await timing.boundingBox();
  assert.ok(timingBounds.width <= 800 && timingBounds.height <= 600);
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/timings-compact.png") });
  await timing.getByRole("button", { name: "Close", exact: true }).click();
  assert.equal(
    await page
      .locator(".selection-actions")
      .getByRole("button", { name: "Details…", exact: true })
      .evaluate((node) => node === node.getRootNode().activeElement),
    true,
  );
  await checkCertificates(page, root);
  await checkConnectTunnels(page, root);
  await page.evaluate(() => {
    globalThis.__workspaceFixture.performance = undefined;
  });
  await page
    .locator(".selection-actions")
    .getByRole("button", { name: "Details…", exact: true })
    .click();
  await timing.getByText(/no measured proxy timeline/).waitFor();
  await page.keyboard.press("Escape");
  assert.equal(await page.locator(".timing-dialog[open]").count(), 0);
  await page.evaluate(() => {
    const workspace = document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace");
    const detail = structuredClone(workspace.selectedDetail);
    detail.storedBodies[0].mediaType = "text/html";
    workspace.applyDetail(detail);
  });
  await page
    .locator(".selection-actions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await openActionGroup(page.locator("#selected-request-actions"), "Preview");
  await page.getByRole("button", { name: "Preview page…", exact: true }).click();
  const pageWarning = page.locator(".captured-page-warning[open]");
  assert.equal(
    await pageWarning.getByLabel("Enable scripts for this preview", { exact: true }).isChecked(),
    false,
  );
  assert.match(await pageWarning.textContent(), /may be malicious/);
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/captured-page-warning-compact.png"),
  });
  await page.evaluate(() => {
    globalThis.__workspaceFixture.deferPagePreview = true;
    globalThis.__workspaceFixture.pagePreviewCanceled = false;
  });
  await pageWarning.getByRole("button", { name: "Open preview", exact: true }).click();
  await pageWarning.getByText("Preparing captured responses…", { exact: true }).waitFor();
  await page.keyboard.press("Escape");
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
        .previewPageBusy,
  );
  assert.equal(await page.locator(".captured-page-warning[open]").count(), 0);
  await page.evaluate(() => {
    globalThis.__workspaceFixture.deferPagePreview = false;
    globalThis.__workspaceFixture.pagePreviewCanceled = false;
  });
  await page
    .locator(".selection-actions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await openActionGroup(page.locator("#selected-request-actions"), "Preview");
  await page.getByRole("button", { name: "Preview page…", exact: true }).click();
  await pageWarning.getByLabel("Resource source", { exact: true }).selectOption("all-loaded");
  await pageWarning.getByText(/may mix content from different captures/).waitFor();
  await pageWarning.getByLabel("Enable scripts for this preview", { exact: true }).check();
  await pageWarning.getByRole("button", { name: "Open preview", exact: true }).click();
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
        .previewPageBusy,
  );
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.previewArgs.enableScripts),
    true,
  );
  assert.deepEqual(await page.evaluate(() => globalThis.__workspaceFixture.previewArgs.options), {
    scope: "all-loaded",
  });
  assert.equal(await page.locator(".captured-page-warning[open]").count(), 0);
  await page
    .locator(".selection-actions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await openActionGroup(page.locator("#selected-request-actions"), "Preview");
  await page.getByRole("button", { name: "Last preview diagnostics…", exact: true }).click();
  const previewReport = page.locator(".captured-preview-report[open]");
  await previewReport
    .getByText(/3 frozen variants.*1 skipped.*4 served.*2 network failures.*Scripts enabled/)
    .waitFor();
  await previewReport.getByText("Captured resource versions", { exact: true }).click();
  await previewReport.getByText("POST · fixture.saz", { exact: true }).waitFor();
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/preview-diagnostics-wide.png"),
  });
  await page.setViewportSize({ width: 760, height: 520 });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/preview-diagnostics-compact.png"),
  });
  assert.ok((await previewReport.boundingBox()).height <= 520);
  await previewReport.getByRole("button", { name: "Close", exact: true }).click();
  assert.equal(
    await page
      .locator(".selection-actions")
      .getByRole("button", { name: "Actions", exact: true })
      .evaluate((node) => node.getRootNode().activeElement === node),
    true,
  );
  await checkPagePreviewRules(page, root);
  await page.evaluate(() => {
    const workspace = document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace");
    workspace.metadataContext = "Placeholder metadata";
    workspace.metadataNetworkContext = "Placeholder network context";
  });
  await page.evaluate(async () => {
    await document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace")
      .clearTraffic();
  });
  assert.equal(
    await page.evaluate(
      () =>
        document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
          .metadataContext,
    ),
    "",
  );
  assert.equal(
    await page.evaluate(
      () =>
        document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
          .metadataNetworkContext,
    ),
    "",
  );
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace").sessions
        .length === 0,
  );
  await page.evaluate(async () => {
    await document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace")
      .undoTrafficRemoval();
  });
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace").sessions
        .length > 0,
  );
  await page.evaluate(async () => {
    globalThis.__workspaceFixture.clearBytes = 100000000;
    await document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace")
      .clearTraffic();
  });
  assert.ok(
    await page.evaluate(() => {
      const action = document
        .querySelector("app-shell")
        .shadowRoot.querySelector("traffic-workspace")
        .trafficUndo.at(-1);
      return action.expiresAt > Date.now() + 299000 && action.expiresAt <= Date.now() + 300000;
    }),
  );
  await page.evaluate(() => {
    const workspace = document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace");
    workspace.trafficUndo = workspace.trafficUndo.map((action) => ({
      ...action,
      expiresAt: Date.now() - 1,
    }));
    workspace.scheduleTrafficUndoExpiry();
  });
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace").trafficUndo
        .length === 0,
  );
  await page.evaluate(async () => {
    globalThis.__workspaceFixture.clearBytes = 1000000000;
    await document
      .querySelector("app-shell")
      .shadowRoot.querySelector("traffic-workspace")
      .clearTraffic();
  });
  assert.equal(
    await page.evaluate(
      () =>
        document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
          .trafficUndo.length,
    ),
    0,
  );
  assert.match(
    await page.evaluate(
      () =>
        document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
          .trafficUndoText,
    ),
    /1 GB or more/,
  );
  await page.goto("https://workspace.test/");
  await checkTrafficFilters(page, root);
  await checkTrafficAdvanced(page, root);
  await checkStreams(page, root);
  await checkAutomationContracts(page, root);
  await checkActionMenus(page, root);
  await checkWindowShortcuts(page, root);
  await checkTrafficPresentation(page, root);
  await checkTrafficClassifications(page, root);
  await checkBodyRefresh(page, root);
  await checkBodyDecoding(page);
  await checkXssiJson(page, root);
  await checkBodySource(page, root);
  await checkJsonFields(page);
  await checkBodySourceBounds(page);
  await checkRequestOrder(page, root);
  await checkTrafficNavigation(page);
  await page.goto("https://workspace.test/");
  await checkAppSelection(page, root);
  await checkTimingLabels(page, root);
  await checkTrafficTimeline(page, root);
  await page.goto("https://workspace.test/?empty");
  await page.waitForFunction(() => document.querySelector("app-shell").traffic.queryLoaded);
  const emptyDrop = page.locator("traffic-workspace trace-drop-target");
  await emptyDrop.getByRole("button", { name: /Drop traces here/ }).waitFor({ state: "visible" });
  assert.match(
    await emptyDrop.locator(".trace-drop-formats").textContent(),
    /NetLog \(\.netlog\/\.json\/\.jsonl\)/,
  );
  await page.setViewportSize({ width: 760, height: 520 });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/traffic-first-empty.png"),
  });
  await page.evaluate(async () => {
    const state = globalThis.__workspaceFixture;
    state.sessions = [state.emptyExample];
    await document.querySelector("app-shell").traffic.refreshSessions(undefined, true);
  });
  assert.equal(await emptyDrop.getByRole("button", { name: /Drop traces here/ }).count(), 0);
  await page.evaluate(async () => {
    await document.querySelector("app-shell").traffic.clearTraffic();
  });
  assert.equal(
    await emptyDrop.getByRole("button", { name: /Drop traces here/ }).count(),
    0,
    "Clear all restored the initial import helper",
  );
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/traffic-cleared-empty.png"),
  });
  await checkComparison(page, root);
  await checkMcp(page, root);
  await checkFormatConverter(page, root);
  await checkPortableCases(page, root);
  await checkIncidents(page, root);
  await checkResponseFiles(page, root);
  await checkResponseSetGuide(page, root);
  await checkPreviewPrerequisites(page, root);
  await checkResponseSetFollowups(page, root);
  await checkRequestControls(page);
  await checkTrafficShortcuts(page, root);
  await checkResponseSetWorkspaces(page, root);
  await checkResponseSetDraftRegressions(page);
  await checkResponseSetPendingDialogs(page, root);
  await checkResponseSetCollectionAdds(page, root);
  await checkResponseSetIntake(page, root);
  await checkResponseSetIntakeRefresh(page, root);
  await checkResponseSetIntakeMove(page, root);
  await checkResponseSetLifecycle(page, root);
  await checkSavedDemos(page, root);
  await checkResponseSetPreview(page, root);
  await checkHeaderRules(page, root);
  await checkTextEdits(page, root);
  assert.deepEqual(errors, []);
  process.stdout.write(
    JSON.stringify({
      startupRequests,
      coalescedQueries: coalesced,
      editorsBefore,
      editorsVisible,
      components: built.stats.componentCount,
      cspViolations: 0,
    }) + "\n",
  );
} catch (error) {
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/workspace-failure.png") });
  process.stderr.write(
    JSON.stringify(
      {
        errors,
        requests,
        state: await page.evaluate(() => ({
          calls: globalThis.__workspaceFixture?.calls,
          output: document.querySelector("app-shell")?.shadowRoot?.querySelector(".session-status")
            ?.textContent,
          diagnostics: document
            .querySelector("app-shell")
            ?.shadowRoot?.querySelector(".global-diagnostics")?.textContent,
          definitions: ["app-shell", "traffic-workspace", "settings-workspace"].map((tag) => [
            tag,
            Boolean(customElements.get(tag)),
          ]),
        })),
      },
      null,
      2,
    ) + "\n",
  );
  throw error;
} finally {
  await browser.close();
}
