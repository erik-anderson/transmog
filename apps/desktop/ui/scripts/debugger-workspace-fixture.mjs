export function installDebuggerWorkspaceFixture() {
  globalThis.__debuggerWorkspaceCommand = async (command, args = {}) => {
    const state = globalThis.__workspaceFixture;
    const debug = (state.workspaceDebug ??= {
      open: false,
      seen: false,
      points: [],
      watches: [],
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
      pauseTarget: "any",
      pausePending: false,
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
      ...(debug.extraFiles ?? []),
    ];
    const view = () => ({
      canOpenScripts: debug.canOpenScripts ?? true,
      pauseTarget: debug.pauseTarget,
      pausePending: debug.pausePending,
      scratch: debug.scratch ? { info: debug.scratch.info } : null,
      skipPauses: debug.skipPauses,
      open: debug.open,
      files,
      targets: debug.open && !debug.noWorkers ? [debug.info, ...(debug.extraTargets ?? [])] : [],
      breakpoints: debug.points,
      viewState: debug.viewState ?? null,
      layout: debug.layout,
      watches: debug.watches,
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
      debug.pausePending = false;
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
      if (args.skip) debug.pausePending = false;
      return;
    }
    if (command === "workspace_pause_next") {
      debug.pauseTarget = args.target;
      debug.pausePending = args.enabled;
      await globalThis.__debuggerWorkspaceCommand("workspace_debug_command", {
        session: debug.info.session,
        action: { kind: "pause-next", enabled: args.enabled },
      });
      return;
    }
    if (command === "scratch_console") {
      debug.scratch ??= {
        info: {
          session: `scratch-${(debug.scratchGeneration = (debug.scratchGeneration ?? 0) + 1)}`,
          label: "Scratch",
          mode: "Scratch",
          artifact: "scratch",
          source: "",
        },
        events: [],
        value: undefined,
      };
      const info = structuredClone(debug.scratch.info);
      if (debug.holdScratchStart) {
        debug.holdScratchStart = false;
        await new Promise((resolve) => {
          debug.finishScratchStart = resolve;
        });
        delete debug.finishScratchStart;
      }
      return { info };
    }
    if (command === "reset_scratch_console") {
      if (debug.rejectScratchReset) throw new Error("Scratch reset failed");
      if (args.session && args.session !== debug.scratch?.info.session)
        throw new Error("Scratch context changed");
      if (debug.holdScratchReset) {
        debug.holdScratchReset = false;
        await new Promise((resolve) => {
          debug.finishScratchReset = resolve;
        });
        delete debug.finishScratchReset;
      }
      debug.scratch = null;
      return;
    }
    if (command === "debugger_open_scripts") {
      state.emitNative("debugger-open-scripts", args.reveal ?? null);
      return;
    }
    if (command === "debugger_seen") return debug.seen;
    if (command === "debugger_view_state") {
      debug.viewState = structuredClone(args.viewState);
      return;
    }
    if (command === "debugger_layout") {
      if (args.layout) debug.layout = args.layout;
      return structuredClone(debug.layout);
    }
    if (command === "debugger_watches") {
      if (args.expressions) {
        if (debug.rejectWatchSave) throw new Error("Watch save failed");
        debug.watches = [...args.expressions];
        if (debug.holdWatchSave) {
          debug.holdWatchSave = false;
          await new Promise((resolve) => {
            debug.finishWatchSave = resolve;
          });
          delete debug.finishWatchSave;
        }
      }
      return [...debug.watches];
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
      debug.viewState = args.viewState ?? debug.viewState;
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
    if (command === "workspace_debug_session" && args.session === debug.scratch?.info.session) {
      return {
        info: debug.scratch.info,
        request: null,
        outcome: null,
        stream: {
          valueGeneration: 0,
          skipPauses: false,
          pauseNext: false,
          active: false,
          paused: null,
          closed: false,
          exceptions: "none",
          cursor: debug.scratch.events.length,
          omitted: false,
          logs: [],
          inventory: { sources: [], breakpoints: [] },
          events: debug.scratch.events.slice(args.after ?? 0),
          sessionRemainingMs: 0,
          pauseRemainingMs: null,
        },
      };
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
          omitted: args.after < (debug.eventFloor ?? 0),
          inventory:
            args.after === 0
              ? {
                  sources: [{ source: js, bytes: 80 }],
                  breakpoints: debug.points.filter((point) => point.enabled).map(binding),
                }
              : undefined,
          events: debug.events
            .slice(Math.max(args.after, debug.eventFloor ?? 0))
            .filter((item) => item.session === args.session),
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
      if (action.kind === "evaluate" && action.expression === "failNewCommand")
        throw new Error("New command failed");
      if (args.session === debug.scratch?.info.session) {
        debug.scratchActions ??= [];
        debug.scratchActions.push(structuredClone(action));
        if (action.kind === "evaluate") {
          if (action.expression === "globalThis.scratchValue = 41") debug.scratch.value = 41;
          const display =
            action.expression === "scratchValue + 1"
              ? String(debug.scratch.value + 1)
              : action.expression === "typeof scratchValue"
                ? typeof debug.scratch.value
                : String(debug.scratch.value ?? 7);
          debug.scratch.events.push({
            session: args.session,
            invocation: 0,
            request,
            valueGeneration: 0,
            event: {
              kind: "evaluated",
              pause: 0,
              thrown: false,
              value: {
                kind: "number",
                display,
                handle: null,
                truncated: false,
              },
            },
          });
          if (debug.holdScratchEvaluation) {
            debug.holdScratchEvaluation = false;
            await new Promise((resolve, reject) => {
              debug.finishScratchEvaluation = resolve;
              debug.rejectScratchEvaluation = () => reject(new Error("Retired Scratch reply"));
            });
            delete debug.finishScratchEvaluation;
          }
        }
        return request;
      }
      debug.actions.push(structuredClone(action));
      if (action.kind === "set-value") {
        const respond = (event) => {
          if (debug.holdValueReply) {
            debug.holdValueReply = false;
            debug.finishValueReply = () => {
              delete debug.finishValueReply;
              emit(event, request);
            };
          } else emit(event, request);
        };
        if (debug.holdValueEdit) {
          debug.holdValueEdit = false;
          await new Promise((resolve) => {
            debug.finishValueEdit = resolve;
          });
          delete debug.finishValueEdit;
        }
        if (debug.rejectValueEdit || action.expression === "invalid(") {
          respond({ kind: "rejected", message: "Invalid value expression" });
        } else {
          const property = debug.propertyInventory?.[action.handle]?.[action.index];
          if (property) {
            property.value = {
              kind: "number",
              display: action.expression,
              handle: null,
              truncated: false,
            };
            if (property.name === "value") debug.value = action.expression;
          } else debug.value = action.expression;
          respond({ kind: "acknowledged" });
        }
        return request;
      }
      if (action.kind === "evaluate" && debug.holdWorkerEvaluation) {
        debug.holdWorkerEvaluation = false;
        await new Promise((resolve) => {
          debug.finishWorkerEvaluation = resolve;
        });
        delete debug.finishWorkerEvaluation;
      }
      if (action.kind === "pause-next") {
        debug.pauseNext = action.enabled;
        emit(
          { kind: "control", skipPauses: debug.skipPauses, pauseNext: debug.pauseNext },
          request,
        );
        if (debug.omitPauseReply) {
          debug.omitPauseReply = false;
          debug.eventFloor = debug.events.length;
        }
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
      if (action.kind === "evaluate" && /^value = \d+$/u.test(action.expression)) {
        debug.value = action.expression.split(" = ")[1];
        const property = debug.propertyInventory?.[1]?.find(
          (property) => property.name === "value",
        );
        if (property) property.value.display = debug.value;
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
                      : String(debug.value ?? 42)
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
            scopes: debug.scopeInventory ?? [
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
                      display:
                        action.handle === 4
                          ? "21"
                          : action.handle === 1
                            ? String(debug.value ?? 42)
                            : "42",
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
            .map((property) => ({
              ...property,
              writable: property.writable ?? property.value.kind !== "unavailable",
            }))
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
