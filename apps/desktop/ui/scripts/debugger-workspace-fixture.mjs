export function installDebuggerWorkspaceFixture() {
  globalThis.__debuggerWorkspaceCommand = async (command, args = {}) => {
    const state = globalThis.__workspaceFixture;
    const debug = (state.workspaceDebug ??= {
      open: false,
      seen: false,
      points: [],
      events: [],
      request: 0,
      paused: null,
      epoch: 0,
      closed: false,
      layout: {
        sourcesSplit: 18,
        inspectorSplit: 22,
        consoleSplit: 70,
        sourcesVisible: true,
        inspectorVisible: true,
        consoleVisible: true,
      },
      policy: "none",
      skipPauses: false,
      pauseNext: false,
      actions: [],
      info: {
        session: "workspace-worker",
        mode: "Workspace script",
        label: "sample · revision 1",
        artifact: "sample:1:exact",
        source: "file:///transmog-script/main.js",
      },
    });
    const ts = "file:///transmog-scripts/sample/main.ts";
    const js = "file:///transmog-script/main.js";
    const files = [
      {
        id: "script/sample/main.ts",
        group: "User scripts",
        package: "sample",
        path: "main.ts",
        artifact: debug.info.artifact,
        source: ts,
        language: "typescript",
        state: "Loaded",
        counterpart: "script/sample/main.js",
      },
      {
        id: "script/sample/main.js",
        group: "User scripts",
        package: "sample",
        path: "main.js",
        artifact: debug.info.artifact,
        source: js,
        language: "javascript",
        state: "Loaded",
        counterpart: "script/sample/main.ts",
      },
      {
        id: "extension/example/src/view.ts",
        group: "Extensions",
        package: "example",
        path: "src/view.ts",
        artifact: "unloaded-package",
        source: "file:///transmog-extension/src/view.ts",
        language: "typescript",
        state: "Unloaded",
        counterpart: null,
      },
    ];
    const view = () => ({
      skipPauses: debug.skipPauses,
      open: debug.open,
      files,
      targets: debug.open ? [debug.info, ...(debug.extraTargets ?? [])] : [],
      breakpoints: debug.points,
      viewState: debug.viewState ?? null,
      layout: debug.layout,
    });
    const binding = (point) => ({
      id: point.breakpoint.id,
      requested: point.breakpoint.location,
      resolved: { source: js, line: point.breakpoint.location.line, column: 3 },
      issue: null,
    });
    const emit = (event, request = 0) =>
      debug.events.push({
        session: debug.info.session,
        invocation: 1,
        request,
        event,
        valueGeneration: debug.epoch,
      });
    debug.pause = (exception = null) => {
      if (debug.skipPauses) return;
      const event = {
        kind: "paused",
        pause: ++debug.epoch,
        reason: exception ? "exception" : debug.pauseNext ? "next callback" : "breakpoint",
        frames: [
          {
            index: 0,
            function: "run",
            location: { source: js, line: debug.pauseLine ?? 2, column: 3 },
            original: { source: ts, line: debug.pauseLine ?? 2, column: 3 },
            unavailable: false,
          },
          {
            index: 1,
            function: "outer",
            location: { source: js, line: 3, column: 3 },
            original: { source: ts, line: 3, column: 3 },
            unavailable: false,
          },
        ],
        exception,
        truncated: false,
      };
      debug.pauseNext = false;
      delete debug.pauseLine;
      emit(event);
      debug.paused = debug.events.at(-1);
    };
    debug.logpoint = (text) =>
      emit({
        kind: "logpoint",
        text,
        thrown: false,
        omitted: 0,
        timeUnixMs: Date.now(),
        location: { source: js, line: 3, column: 1 },
        original: { source: ts, line: 3, column: 1 },
      });
    if (command === "workspace_skip_pauses") {
      debug.skipPauses = args.skip;
      if (args.skip) debug.pauseNext = false;
      return;
    }
    if (command === "debugger_seen") return debug.seen;
    if (command === "debugger_layout") {
      if (args.layout) debug.layout = args.layout;
      return structuredClone(debug.layout);
    }
    if (command === "toggle_debugger") {
      await globalThis.__debuggerWorkspaceCommand("place_debugger", {
        placement: debug.open ? "closed" : "bottom",
      });
      if (debug.holdToggleReply) {
        debug.holdToggleReply = false;
        await new Promise((done) => {
          debug.finishToggle = done;
        });
        delete debug.finishToggle;
      }
      return;
    }
    if (command === "open_debugger") {
      if (!debug.open)
        await globalThis.__debuggerWorkspaceCommand("place_debugger", { placement: "bottom" });
      return;
    }
    if (command === "workspace_restart") {
      debug.closed = false;
      debug.pause();
      return;
    }
    if (command === "debugger_breakpoint_positions" && args.id.startsWith("extension/")) return [];
    if (command === "debugger_breakpoint_positions")
      return debug.points.map((point) => [
        point.breakpoint.id,
        { ...point.breakpoint.location, source: args.id.endsWith(".js") ? js : ts },
      ]);
    if (command === "place_debugger") {
      if (debug.holdMove) {
        debug.holdMove = false;
        state.emitNative("debugger-moving", true);
        await new Promise((resolve, reject) => {
          debug.failMove = () => reject(new Error("Debugger move failed"));
        });
      }
      debug.open = args.placement !== "closed";
      debug.viewState = args.viewState;
      const shell = document.querySelector("app-shell");
      if (debug.recreateSurface && debug.open) {
        shell.debuggerPlacement = "closed";
        shell.debuggerDocked = false;
        shell.$flushUpdates();
      }
      shell.debuggerPlacement = args.placement === "window" ? "bottom" : args.placement;
      shell.debuggerDocked = debug.open;
      if (!debug.recreateSurface) {
        state.emitNative("debugger-moving", false);
      }
      return;
    }
    if (command === "acknowledge_debugger") {
      debug.open = true;
      debug.seen = true;
      document.querySelector("app-shell").debuggerSeen = true;
      return view();
    }
    if (command === "debugger_workspace") return view();
    if (command === "debugger_file" && debug.holdSourceRead) {
      debug.holdSourceRead = false;
      await new Promise((done) => {
        debug.releaseSourceRead = done;
      });
      delete debug.releaseSourceRead;
    }
    if (command === "debugger_file")
      return args.id.endsWith(".js")
        ? "function run() {\n  const value = 42;\n  return value;\n}"
        : "export function run(): number {\n  const value: number = 42;\n  return value;\n}";
    if (command === "workspace_breakpoints") {
      debug.points = debug.points.filter(
        (point) => !args.removed.some((removed) => removed.breakpoint.id === point.breakpoint.id),
      );
      for (const point of args.points) {
        const index = debug.points.findIndex(
          (previous) => previous.breakpoint.id === point.breakpoint.id,
        );
        if (index < 0) debug.points.push(structuredClone(point));
        else debug.points[index] = structuredClone(point);
      }
      for (const point of debug.points.filter((point) => point.enabled)) {
        emit({ kind: "sources", sources: [], breakpoints: [binding(point)] });
      }
      return structuredClone(debug.points);
    }
    if (command === "workspace_debug_session")
      return {
        request:
          debug.paused && debug.requestContext !== false
            ? { id: "first", method: "GET", url: debug.requestUrl ?? "http://example.test/first" }
            : null,
        info:
          args.session === debug.info.session
            ? debug.info
            : debug.extraTargets.find((target) => target.session === args.session),
        outcome: null,
        stream: {
          operation: debug.paused
            ? {
                callback: "onRequestHead",
                invocation: 1,
                exchangeId: debug.requestContext !== false ? "first" : null,
              }
            : null,
          valueGeneration: debug.epoch,
          skipPauses: debug.skipPauses,
          pauseNext: debug.pauseNext,
          active: !!debug.paused,
          paused: args.session === debug.info.session ? debug.paused : null,
          closed: debug.closed,
          exceptions: debug.policy,
          cursor: debug.events.length,
          omitted: false,
          inventory:
            args.after === 0
              ? {
                  sources: [{ source: js, bytes: 80 }],
                  breakpoints: debug.points.filter((point) => point.enabled).map(binding),
                }
              : undefined,
          events: debug.events.slice(args.after).filter((item) => item.session === args.session),
          logs: debug.logs ?? [],
          sessionRemainingMs: 0,
          pauseRemainingMs: null,
        },
      };
    if (command === "workspace_debug_request") {
      if (
        !debug.paused ||
        debug.paused.event.pause !== args.pause ||
        debug.requestContext === false
      )
        throw new Error("This pause has ended");
      state.emitNative("debugger-request", "first");
      return;
    }
    if (command === "workspace_debug_command") {
      const request = ++debug.request;
      const action = args.action;
      debug.actions.push(structuredClone(action));
      if (action.kind === "pause-next") {
        debug.pauseNext = action.enabled;
        emit(
          { kind: "control", skipPauses: debug.skipPauses, pauseNext: debug.pauseNext },
          request,
        );
      }
      if (action.kind === "continue-to") {
        debug.paused = null;
        emit({ kind: "resumed" });
        debug.pauseLine = action.location.line;
        debug.pause();
      }
      if (action.kind === "exceptions") debug.policy = action.policy;
      if (action.kind === "stop") {
        debug.closed = true;
        debug.paused = null;
        emit({ kind: "stopped", reason: "Stopped" });
      }
      if (action.kind === "evaluate" && action.expression.includes("console.log"))
        emit({ kind: "output", level: "info", text: "<img src=x onerror=alert(1)>" }, request);
      if (action.kind === "complete") {
        const prefix = action.expression.split(".").at(-1);
        emit(
          {
            kind: "completions",
            pause: action.pause,
            matches: ["value", "sample", "nested", "answer"].filter((name) =>
              name.startsWith(prefix),
            ),
            omitted: false,
          },
          request,
        );
      }
      if (action.kind === "evaluate" || action.kind === "inspect")
        emit(
          {
            kind: "evaluated",
            pause: action.pause,
            value: {
              kind: action.expression === "sample" ? "object" : "number",
              display:
                action.expression === "sample"
                  ? "[Object]"
                  : action.expression === "value"
                    ? action.frame === 1
                      ? "21"
                      : "42"
                    : "7",
              handle: action.expression === "sample" ? 2 : null,
              truncated: false,
            },
            thrown: false,
          },
          request,
        );
      else if (action.kind === "scopes")
        emit(
          {
            kind: "scopes",
            pause: action.pause,
            frame: action.frame,
            scopes: [
              { kind: "local", name: "", handle: action.frame === 1 ? 4 : 1 },
              { kind: "closure", name: "outer", handle: 3 },
              { kind: "global", name: "", handle: 5 },
            ],
          },
          request,
        );
      else if (["properties", "preview-properties", "property"].includes(action.kind)) {
        if (
          action.kind === "preview-properties" &&
          action.handle === 2 &&
          action.offset === debug.rejectPropertyRead
        ) {
          emit({ kind: "rejected", message: "Property read failed" }, request);
          return request;
        }
        const all =
          debug.propertyInventory?.[action.handle] ??
          (action.handle === 2
            ? [
                {
                  name: "nested",
                  value: { kind: "object", display: "[Object]", handle: 6, truncated: false },
                },
                {
                  name: "secret",
                  value: {
                    kind: "unavailable",
                    display: "Accessor; not evaluated",
                    handle: null,
                    truncated: false,
                  },
                },
              ]
            : action.handle === 6
              ? [
                  {
                    name: "answer",
                    value: { kind: "number", display: "42", handle: null, truncated: false },
                  },
                ]
              : [
                  {
                    name: "value",
                    value: {
                      kind: "number",
                      display: action.handle === 4 ? "21" : "42",
                      handle: null,
                      truncated: false,
                    },
                  },
                ]);
        if (action.kind === "property") {
          emit(
            {
              kind: "evaluated",
              pause: action.pause,
              value: all[action.index]?.value ?? {
                kind: "unavailable",
                display: "Property is no longer available",
                handle: null,
                truncated: false,
              },
              thrown: false,
            },
            request,
          );
        } else {
          const preview = action.kind === "preview-properties";
          const properties = all
            .slice(action.offset, action.offset + (action.limit ?? 100))
            .map((property) =>
              preview ? { ...property, value: { ...property.value, handle: null } } : property,
            );
          const next = action.offset + properties.length;
          emit(
            {
              kind: preview ? "property-preview" : "properties",
              pause: action.pause,
              handle: action.handle,
              offset: action.offset,
              properties,
              total: all.length,
              next: next < all.length ? next : null,
            },
            request,
          );
        }
      } else if (action.kind === "resume") {
        debug.paused = null;
        emit({ kind: "resumed" });
        if (action.action !== "continue") debug.pause();
      }
      if (action.kind === "evaluate" && debug.holdEvaluation) {
        await new Promise((done) => {
          debug.finishEvaluation = done;
        });
        delete debug.finishEvaluation;
      }
      return request;
    }
    throw new Error("Unexpected debugger workspace command: " + command);
  };
}
