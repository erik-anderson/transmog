/** Install typed native-command fixtures without an Inspector or evaluation endpoint. */
export function installDebuggerFixture() {
  globalThis.__debuggerFixtureCommand = async (command, args) => {
    const state = globalThis.__workspaceFixture;
    const emit = (debug, event, request = 0) => {
      debug.events.push({
        session: debug.info.session,
        invocation: debug.invocation,
        request,
        event,
      });
    };
    const pause = (debug) => {
      debug.pause++;
      debug.paused = true;
      emit(debug, {
        kind: "paused",
        pause: debug.pause,
        reason: "breakpoint",
        frames: [
          {
            index: 0,
            function: "nested <img src=x onerror=alert(1)>",
            location: { source: "file:///transmog-script/main.js", line: 2, column: 3 },
            original: { source: debug.info.source, line: state.debugSourceLine ?? 2, column: 3 },
            unavailable: false,
          },
          { index: 1, function: "internal invocation", location: null, unavailable: true },
        ],
        exception: null,
        truncated: false,
      });
    };
    if (command === "start_debug_test" || command === "debug_next_script_exchange") {
      state.debugTestRequest = structuredClone(args.request);
      const live = command === "debug_next_script_exchange";
      if (live) state.liveDebugRequest = structuredClone(args.request);
      const session = String(
        ++state.debugSessionCounter || (state.debugSessionCounter = 1),
      ).padStart(32, "a");
      const info = {
        session,
        mode: live
          ? "Live exchange"
          : args.request.scenario
            ? "Sandbox scenario"
            : "Sandbox callback",
        label: "user-script · revision 1",
        artifact: "exact-debug-artifact",
        source: "file:///transmog-scripts/user-script/main.ts",
      };
      const debug = {
        info,
        events: [],
        pause: 0,
        paused: false,
        closed: false,
        invocation: 0,
        request: 0,
        outcome: null,
        armed: live,
        exchange: null,
      };
      state.debug = debug;
      state.debugCommands = [];
      emit(debug, { kind: "attached" });
      emit(debug, {
        kind: "sources",
        sources: [{ source: "file:///transmog-script/main.js", bytes: 100 }],
        breakpoints: args.request.configuration.breakpoints.map((value) => ({
          ...value,
          requested: value.location,
          resolved:
            value.location.line < 4
              ? { source: "file:///transmog-script/main.js", line: 2, column: 3 }
              : null,
          issue: value.location.line < 4 ? null : "No executable position in this module",
        })),
      });
      if (live)
        state.claimDebugExchange = () => {
          debug.armed = false;
          debug.exchange = "selected-live-exchange";
          pause(debug);
        };
      else pause(debug);
      return info;
    }
    const debug = state.debug;
    if (!debug || args.session !== debug.info.session) {
      throw new Error("Debugger session is unavailable in this window");
    }
    if (command === "debug_session") {
      if (state.deferDebugPoll) {
        await new Promise((resolve) => {
          state.finishDebugPoll = resolve;
        });
      }
      return {
        info: debug.info,
        stream: {
          active: !debug.closed && !debug.armed,
          armRemainingMs: debug.armed ? 60000 : null,
          exchangeId: debug.exchange,
          cursor: debug.events.length,
          omitted: false,
          closed: debug.closed,
          pauseRemainingMs: debug.paused ? 60000 : null,
          sessionRemainingMs: 590000,
          logs: state.scriptTestLogs ?? [],
          events: debug.events.slice(args.after),
        },
        outcome: debug.outcome,
      };
    }
    if (command === "close_debug_session") {
      debug.closed = true;
      debug.paused = false;
      state.debugClosedSession = args.session;
      return;
    }
    const action = args.action;
    state.debugCommands.push(structuredClone(action));
    const request = ++debug.request;
    switch (action.kind) {
      case "scopes":
        emit(
          debug,
          {
            kind: "scopes",
            pause: debug.pause,
            frame: action.frame,
            scopes:
              state.debugInitializerScope && debug.invocation === 0
                ? [{ kind: "module", name: "", handle: 2 }]
                : [
                    { kind: "local", name: "nested", handle: 1 },
                    { kind: "closure", name: "outer", handle: 2 },
                  ],
          },
          request,
        );
        break;
      case "properties": {
        const pageSize = state.debugPropertyPageSize ?? 100;
        const properties =
          action.handle === 1
            ? [
                {
                  name: "local",
                  value: { kind: "number", display: "7", handle: null, truncated: false },
                },
                {
                  name: "object",
                  value: { kind: "object", display: "[Object]", handle: 3, truncated: false },
                },
                {
                  name: "getter",
                  value: {
                    kind: "unavailable",
                    display: "Accessor; not evaluated",
                    handle: null,
                    truncated: false,
                  },
                },
              ]
            : action.handle === 2
              ? [
                  {
                    name: "closedOver",
                    value: { kind: "number", display: "41", handle: null, truncated: false },
                  },
                ]
              : Array.from({ length: 105 }, (_, index) => ({
                  name: `key-${index}`,
                  value: {
                    kind: "string",
                    display: `value-${index} <script>alert(1)</script>`,
                    handle: null,
                    truncated: false,
                  },
                }));
        emit(
          debug,
          {
            kind: "properties",
            pause: debug.pause,
            handle: action.handle,
            offset: action.offset,
            properties: properties.slice(action.offset, action.offset + pageSize),
            next: properties.length > action.offset + pageSize ? action.offset + pageSize : null,
          },
          request,
        );
        break;
      }
      case "resume":
        debug.paused = false;
        emit(debug, { kind: "resumed" });
        emit(debug, { kind: "acknowledged" }, request);
        if (action.action === "continue") {
          debug.outcome = {
            status: "completed",
            summary: "Callback completed",
            result: { decision: { action: "continue" } },
            truncated: false,
            exception: null,
            logs: [],
          };
          debug.closed = true;
          emit(debug, { kind: "stopped", reason: "Debug run completed; worker released" });
        } else {
          debug.invocation = 1;
          pause(debug);
        }
        break;
      case "breakpoints":
        emit(
          debug,
          {
            kind: "sources",
            sources: [],
            breakpoints: action.values.map((value) => ({
              id: value.id,
              requested: value.location,
              resolved: value.location,
              issue: null,
            })),
          },
          request,
        );
        emit(debug, { kind: "acknowledged" }, request);
        break;
      case "detach":
        debug.closed = true;
        debug.paused = false;
        emit(debug, { kind: "detached" }, request);
        break;
      case "stop":
        debug.closed = true;
        debug.paused = false;
        emit(debug, { kind: "stopped", reason: "Stopped by its owner" });
        break;
      default:
        emit(debug, { kind: "acknowledged" }, request);
        break;
    }
    if (state.debugAckDelay)
      await new Promise((resolve) => setTimeout(resolve, state.debugAckDelay));
    return request;
  };
}
